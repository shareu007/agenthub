//! Text-only handoff into NEW native sessions. Source session records stay unchanged.
use agenthub_core::{Agent, Message, ResumeCommand, Session};
use anyhow::{bail, Context, Result};
use chrono::{Duration, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};
use uuid::Uuid;

const MAX_TEXT_BYTES: usize = 64 * 1024 * 1024;
#[derive(Clone, Debug, Serialize)]
pub struct Turn {
    pub role: String,
    pub text: String,
}
struct Node {
    id: Option<String>,
    parent: Option<String>,
    turn: Option<Turn>,
}

/// Keeps only branch links and transferable text, not the full raw transcript.
pub struct Collector {
    agent: Agent,
    nodes: Vec<Node>,
    bytes: usize,
    records: usize,
    tree: bool,
    pub dropped: usize,
}
impl Collector {
    pub fn new(agent: Agent) -> Self {
        Self {
            agent,
            nodes: vec![],
            bytes: 0,
            records: 0,
            tree: false,
            dropped: 0,
        }
    }
    pub fn push(&mut self, m: Message) -> Result<()> {
        self.records += 1;
        if self.records > 1_000_000 {
            bail!("transfer transcript exceeds one million records");
        }
        if self.agent == Agent::Pi && m.kind == "session" {
            self.tree = m.raw["version"].as_u64().unwrap_or(1) >= 2;
            self.dropped += 1;
            return Ok(());
        }
        if self.agent == Agent::Claude && m.raw["isSidechain"] == true {
            self.dropped += 1;
            return Ok(());
        }
        let turn = if self.agent == Agent::Agy && m.role == "assistant" {
            agenthub_adapters::agy_assistant_reply(&m.raw)?
                .filter(|t| !t.trim().is_empty())
                .map(|text| Turn {
                    role: "assistant".into(),
                    text,
                })
        } else {
            dialogue(self.agent, &m)
        };
        if turn.is_none() {
            self.dropped += 1;
        }
        if let Some(t) = &turn {
            self.bytes += t.text.len();
        }
        if self.bytes > MAX_TEXT_BYTES {
            bail!("transfer transcript exceeds 64 MiB; export a smaller source session");
        }
        if self.agent == Agent::Claude && m.native_id.is_some() {
            self.tree = true;
        }
        if matches!(self.agent, Agent::Pi | Agent::Claude) {
            self.nodes.push(Node {
                id: m.native_id,
                parent: m.parent_id,
                turn,
            });
        } else if turn.is_some() {
            // Linear formats do not need IDs, parent links, or empty tool/event
            // nodes retained after normalization.
            self.nodes.push(Node {
                id: None,
                parent: None,
                turn,
            });
        }
        Ok(())
    }
    pub fn finish(mut self, last: Option<usize>) -> Result<(Vec<Turn>, usize)> {
        let total_turns = self.nodes.iter().filter(|n| n.turn.is_some()).count();
        let mut turns = Vec::new();
        if self.tree {
            let mut by_id = HashMap::new();
            for (index, node) in self.nodes.iter().enumerate() {
                if let Some(id) = &node.id {
                    if by_id.insert(id.as_str(), index).is_some() {
                        bail!("duplicate branch entry ID; cannot transfer safely");
                    }
                } else if node.turn.is_some() {
                    bail!("branch entry has no ID; cannot determine active history");
                }
            }
            let mut index = self
                .nodes
                .iter()
                .rposition(|n| n.id.is_some() && (self.agent != Agent::Claude || n.turn.is_some()));
            let mut seen = vec![false; self.nodes.len()];
            let mut chain = Vec::new();
            let mut active_turns = 0usize;
            while let Some(i) = index {
                if std::mem::replace(&mut seen[i], true) {
                    bail!("cycle in source branch history");
                }
                let node = &self.nodes[i];
                if node.turn.is_some() {
                    active_turns += 1;
                    if last.is_none_or(|n| chain.len() < n) {
                        chain.push(i);
                    }
                }
                index =
                    match node.parent.as_deref().filter(|p| !p.is_empty()) {
                        None => None,
                        Some(parent) => Some(*by_id.get(parent).context(
                            "source branch parent is missing; use a complete transcript",
                        )?),
                    };
            }
            // Resolve/validate the complete branch before moving text. Avoid
            // cloning a potentially 64 MiB history just to keep its last N rows.
            drop(by_id);
            for i in chain.into_iter().rev() {
                if let Some(turn) = self.nodes[i].turn.take() {
                    turns.push(turn);
                }
            }
            let omitted =
                total_turns.saturating_sub(active_turns) + active_turns.saturating_sub(turns.len());
            if turns.is_empty() {
                bail!("source contains no transferable user/assistant text");
            }
            return Ok((turns, self.dropped + omitted));
        } else {
            turns = self.nodes.into_iter().filter_map(|n| n.turn).collect();
        }
        let branch_omitted = total_turns.saturating_sub(turns.len());
        let omitted = last
            .map(|last| turns.len().saturating_sub(last))
            .unwrap_or(0);
        if omitted > 0 {
            turns.drain(..omitted);
        }
        if turns.is_empty() {
            bail!("source contains no transferable user/assistant text");
        }
        Ok((turns, self.dropped + branch_omitted + omitted))
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| {
                matches!(
                    p["type"].as_str(),
                    Some("text" | "input_text" | "output_text")
                )
            })
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}
fn dialogue(agent: Agent, m: &Message) -> Option<Turn> {
    if !matches!(m.role.as_str(), "user" | "assistant") {
        return None;
    }
    let value = match agent {
        Agent::Pi | Agent::Claude => text(&m.raw["message"]["content"]),
        Agent::Codex if m.kind == "response_item" && m.raw["payload"]["type"] == "message" => {
            text(&m.raw["payload"]["content"])
        }
        Agent::Opencode => {
            let mut parts = Vec::new();
            if let Some(t) = m.raw["info"]["text"].as_str() {
                parts.push(t.to_owned());
            }
            if let Some(ps) = m.raw["parts"].as_array() {
                for p in ps {
                    if p["type"] == "text" {
                        if let Some(t) = p["text"].as_str() {
                            parts.push(t.to_owned());
                        }
                    }
                }
            }
            if parts.is_empty() {
                text(&m.raw["info"]["content"])
            } else {
                parts.join("\n")
            }
        }
        Agent::Agy if m.role == "user" => m.text.clone(),
        _ => String::new(),
    };
    if value.trim().is_empty() {
        None
    } else {
        Some(Turn {
            role: m.role.clone(),
            text: value,
        })
    }
}

#[derive(Serialize)]
pub struct Plan {
    pub source: String,
    pub target: Agent,
    pub title: String,
    pub project: PathBuf,
    pub target_root: PathBuf,
    pub turn_count: usize,
    pub excluded_records: usize,
    pub warnings: Vec<String>,
    #[serde(skip)]
    turns: Vec<Turn>,
}
impl Plan {
    pub fn new(
        source: &Session,
        target: Agent,
        target_root: PathBuf,
        project: Option<PathBuf>,
        collector: Collector,
        last: Option<usize>,
    ) -> Result<Self> {
        if target == Agent::Agy {
            bail!("AGY has no verified native import contract; it can be a transfer source, not a destination");
        }
        if target == Agent::Claude && target_root.file_name().is_none_or(|n| n != "projects") {
            bail!("Claude transfer requires a projects directory under CLAUDE_CONFIG_DIR");
        }
        let project = project
            .or_else(|| source.project_path.clone())
            .context("source project is unknown; provide --cwd /absolute/project")?;
        if !project.is_absolute() || !project.is_dir() {
            bail!("transfer requires an existing absolute project directory");
        }
        project
            .to_str()
            .context("native transfer requires UTF-8 project paths")?;
        target_root
            .to_str()
            .context("native transfer requires a UTF-8 target root")?;
        if !target_root.is_absolute() {
            bail!("target store root must be absolute");
        }
        let (turns, excluded_records) = collector.finish(last)?;
        let mut warnings = source.warnings.clone();
        warnings.push("Only user/assistant text is copied. Tools, tool results, reasoning, attachments, approvals and runtime/model state are excluded. Project files are not copied or changed.".into());
        if source.agent == target {
            warnings.push("Same-agent transfer creates a new branch/copy with a new ID.".into());
        }
        Ok(Self {
            source: source.key.clone(),
            target,
            title: source.title.clone(),
            project,
            target_root,
            turn_count: turns.len(),
            excluded_records,
            warnings,
            turns,
        })
    }
    pub fn write(&self) -> Result<Receipt> {
        if !self.project.is_dir() {
            bail!("project directory disappeared since the transfer preview");
        }
        let id = Uuid::new_v4().to_string();
        if self.target == Agent::Opencode {
            return self.write_opencode(&id);
        }
        let now = Utc::now();
        let stamp = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let (directory, filename) = match self.target {
            Agent::Pi => (
                self.target_root.join(format!(
                    "--{}--",
                    self.project
                        .to_string_lossy()
                        .trim_start_matches('/')
                        .replace(['/', '\\', ':'], "-")
                )),
                format!("{}_{}.jsonl", now.format("%Y-%m-%dT%H-%M-%S-%3fZ"), id),
            ),
            Agent::Claude => (
                self.target_root.join(
                    self.project
                        .to_string_lossy()
                        .chars()
                        .map(|c| {
                            if c.is_ascii_alphanumeric() || c == '-' {
                                c
                            } else {
                                '-'
                            }
                        })
                        .collect::<String>(),
                ),
                format!("{id}.jsonl"),
            ),
            Agent::Codex => (
                self.target_root
                    .join(now.format("sessions/%Y/%m/%d").to_string()),
                format!("rollout-{}-{id}.jsonl", now.format("%Y-%m-%dT%H-%M-%S")),
            ),
            _ => bail!("unsupported transfer target"),
        };
        private_directory(&directory)?;
        let destination = directory.join(filename);
        let mut file = tempfile::NamedTempFile::new_in(&directory)?;
        {
            let mut file = BufWriter::new(file.as_file_mut());
            let write = |file: &mut dyn Write, value: Value| -> Result<()> {
                serde_json::to_writer(&mut *file, &value)?;
                writeln!(file)?;
                Ok(())
            };
            match self.target {
                Agent::Pi => write(
                    &mut file,
                    json!({"type":"session","version":3,"id":id,"timestamp":stamp,"cwd":self.project}),
                )?,
                Agent::Codex => write(
                    &mut file,
                    json!({"type":"session_meta","timestamp":stamp,"payload":{"id":id,"timestamp":stamp,"cwd":self.project,"originator":"agenthub","cli_version":env!("CARGO_PKG_VERSION"),"source":"cli","thread_source":"user","model_provider":"openai"}}),
                )?,
                Agent::Claude => {}
                _ => unreachable!(),
            }
            let mut parent: Option<String> = None;
            for (i, t) in self.turns.iter().enumerate() {
                let message_id = Uuid::new_v4().to_string();
                let at = now + Duration::milliseconds(i as i64 + 1);
                let timestamp = at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                match self.target {
                    Agent::Pi => {
                        let mut message = json!({"role":t.role,"content":[{"type":"text","text":t.text}],"timestamp":at.timestamp_millis()});
                        if t.role == "assistant" {
                            message["api"] = json!("openai-completions");
                            message["provider"] = json!("agenthub");
                            message["model"] = json!("transferred-history");
                            message["stopReason"] = json!("stop");
                            message["usage"] = json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}});
                        }
                        write(
                            &mut file,
                            json!({"type":"message","id":message_id,"parentId":parent,"timestamp":timestamp,"message":message}),
                        )?;
                    }
                    Agent::Claude => {
                        let message = if t.role == "user" {
                            json!({"role":"user","content":t.text})
                        } else {
                            json!({"id":format!("msg_{}",message_id.replace('-',"")),"type":"message","role":"assistant","model":"<synthetic>","content":[{"type":"text","text":t.text}],"stop_reason":"end_turn","stop_sequence":null,"usage":{}})
                        };
                        write(
                            &mut file,
                            json!({"type":t.role,"uuid":message_id,"parentUuid":parent,"isSidechain":false,"sessionId":id,"cwd":self.project,"timestamp":timestamp,"version":env!("CARGO_PKG_VERSION"),"userType":"external","message":message}),
                        )?;
                    }
                    Agent::Codex => {
                        write(
                            &mut file,
                            json!({"type":"response_item","timestamp":timestamp,"payload":{"type":"message","role":t.role,"content":[{"type":if t.role=="user" {"input_text"} else {"output_text"},"text":t.text}]}}),
                        )?;
                        write(
                            &mut file,
                            json!({"type":"event_msg","timestamp":timestamp,"payload":{"type":if t.role=="user" {"user_message"} else {"agent_message"},"message":t.text}}),
                        )?;
                    }
                    _ => unreachable!(),
                }
                parent = Some(message_id);
            }
            file.flush()?;
        }
        file.as_file().sync_all()?;
        file.persist_noclobber(&destination)
            .context("refusing to replace an existing native session")?;
        let mut receipt = self.receipt(&id, destination);
        if fs::File::open(&directory)
            .and_then(|dir| dir.sync_all())
            .is_err()
        {
            // Publication already happened. Return its identity with a durability
            // warning, rather than suggesting a retry that creates a second copy.
            receipt.warnings.push("New session was created, but directory durability could not be confirmed; inspect this session before repeating transfer.".into());
        }
        Ok(receipt)
    }
    fn receipt(&self, id: &str, path: PathBuf) -> Receipt {
        let mut session =
            Session::new(self.target, id.into(), path.clone()).expect("generated UUID");
        session.project_path = Some(self.project.clone());
        let mut resume = agenthub_core::resume_command(&session).expect("UTF-8 generated path");
        match self.target {
            Agent::Claude => {
                if let Some(root) = self.target_root.parent() {
                    resume.env.push(("CLAUDE_CONFIG_DIR".into(), root.into()));
                }
            }
            Agent::Codex => resume
                .env
                .push(("CODEX_HOME".into(), self.target_root.clone())),
            Agent::Opencode => {
                if let Some(root) = self.target_root.parent() {
                    resume.env.push(("XDG_DATA_HOME".into(), root.into()));
                }
            }
            _ => {}
        }
        Receipt {
            key: session.key,
            path,
            resume,
            warnings: self.warnings.clone(),
        }
    }
    fn write_opencode(&self, id: &str) -> Result<Receipt> {
        if self
            .target_root
            .file_name()
            .is_none_or(|name| name != "opencode")
            || self.target_root.is_file()
        {
            bail!("OpenCode transfer requires a directory named opencode under XDG_DATA_HOME");
        }
        let data_home = self
            .target_root
            .parent()
            .context("OpenCode data root has no parent")?;
        let version = run_native_command(
            Command::new("opencode")
                .arg("--version")
                .env("XDG_DATA_HOME", data_home)
                .current_dir(&self.project),
        )?;
        if !version.lines().any(|line| line.trim().starts_with("1.")) {
            bail!("OpenCode import currently supports native 1.x only; other formats need a verified importer");
        }
        let now = Utc::now().timestamp_millis();
        let session_id = format!("ses_{}", id.replace('-', ""));
        let mut file = tempfile::NamedTempFile::new()?;
        {
            let mut output = BufWriter::new(file.as_file_mut());
            let info = json!({"id":session_id,"slug":format!("agenthub-{id}"),"projectID":"agenthub","directory":self.project,"title":self.title,"version":env!("CARGO_PKG_VERSION"),"time":{"created":now,"updated":now+self.turns.len() as i64}});
            write!(output, "{{\"info\":")?;
            serde_json::to_writer(&mut output, &info)?;
            write!(output, ",\"messages\":[")?;
            let mut parent = String::new();
            for (i, t) in self.turns.iter().enumerate() {
                let mid = format!("msg_{}", Uuid::new_v4().simple());
                let mut info = json!({"id":mid,"sessionID":session_id,"role":t.role,"time":{"created":now+i as i64},"agent":"build"});
                if t.role == "user" {
                    info["model"] =
                        json!({"providerID":"agenthub","modelID":"transferred-history"});
                } else {
                    info["parentID"] = json!(if parent.is_empty() { &mid } else { &parent });
                    info["modelID"] = json!("transferred-history");
                    info["providerID"] = json!("agenthub");
                    info["mode"] = json!("build");
                    info["path"] = json!({"cwd":self.project,"root":self.project});
                    info["cost"] = json!(0);
                    info["tokens"] =
                        json!({"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}});
                }
                if i > 0 {
                    write!(output, ",")?;
                }
                let message = ImportMessage {
                    info,
                    parts: [ImportPart {
                        id: format!("prt_{}", Uuid::new_v4().simple()),
                        message_id: &mid,
                        session_id: &session_id,
                        kind: "text",
                        text: &t.text,
                    }],
                };
                serde_json::to_writer(&mut output, &message)?;
                parent = mid;
            }
            write!(output, "]}}")?;
            output.flush()?;
        }
        let output = run_native_command(
            Command::new("opencode")
                .arg("import")
                .arg(file.path())
                .env("XDG_DATA_HOME", data_home)
                .current_dir(&self.project),
        )?;
        if !output.contains(&format!("Imported session: {session_id}")) {
            bail!("OpenCode did not acknowledge the imported session; inspect native CLI compatibility");
        }
        Ok(self.receipt(&session_id, self.target_root.join("opencode.db")))
    }
}

#[derive(Serialize)]
struct ImportMessage<'a> {
    info: Value,
    parts: [ImportPart<'a>; 1],
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportPart<'a> {
    id: String,
    #[serde(rename = "messageID")]
    message_id: &'a str,
    #[serde(rename = "sessionID")]
    session_id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    text: &'a str,
}

#[derive(Serialize)]
pub struct Receipt {
    pub key: String,
    pub path: PathBuf,
    pub resume: ResumeCommand,
    pub warnings: Vec<String>,
}

fn private_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}
/// Run a native metadata command with bounded runtime/output and no stdin.
pub fn run_native_command(command: &mut Command) -> Result<String> {
    bounded_command_with_limits(command, std::time::Duration::from_secs(30), 4 * 1024 * 1024)
}
fn bounded_command_with_limits(
    command: &mut Command,
    timeout: std::time::Duration,
    log_limit: u64,
) -> Result<String> {
    // Import performs no model turn. Bound runtime and logs; never echo native
    // output (which can include transcript content) in errors.
    let log = tempfile::tempfile()?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log.try_clone()?))
        .spawn()
        .context("start native command")?;
    let mut child = NativeProcess { child };
    let start = Instant::now();
    let status = loop {
        if start.elapsed() >= timeout || log.metadata()?.len() > log_limit {
            bail!("native command timed out or exceeded log limit; check native sessions before retrying");
        }
        if let Some(status) = child.child.try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    if log.metadata()?.len() > log_limit {
        bail!("native command exceeded log limit; check native sessions before retrying");
    }
    if !status.success() {
        bail!(
            "native command failed; inspect native sessions before retrying"
        );
    }
    use std::io::{Read, Seek};
    let mut log = log;
    log.rewind()?;
    let mut output = String::new();
    BufReader::new(log)
        .take(log_limit)
        .read_to_string(&mut output)?;
    Ok(output)
}

// The process group is created by us, so timeout/error cleanup cannot signal
// a user's already-running native agent or unrelated terminal process group.
struct NativeProcess {
    child: std::process::Child,
}
impl Drop for NativeProcess {
    fn drop(&mut self) {
        // Metadata commands must not leave helpers running, even when their
        // direct parent exits successfully. This is our isolated process group.
        #[cfg(unix)]
        // SAFETY: spawn used process_group(0); this is our child's group.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(all(test, unix))]
mod process_tests {
    use super::*;
    #[test]
    fn timeout_reaps_the_native_child_and_zero_exit_does_not_bypass_log_limit() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let started = Instant::now();
        let result = bounded_command_with_limits(
            Command::new("/bin/sh")
                .args(["-c", "echo $$ > \"$AH_TEST_PID\"; exec /bin/sleep 5"])
                .env("AH_TEST_PID", &pid_file),
            std::time::Duration::from_millis(300),
            4096,
        );
        assert!(result.is_err());
        assert!(started.elapsed().as_secs() < 2);
        let pid: i32 = fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // The direct child was killed AND waited, so no live child or zombie remains.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        let result = bounded_command_with_limits(
            Command::new("/bin/sh").args(["-c", &format!("printf %s '{}'", "x".repeat(8192))]),
            std::time::Duration::from_secs(2),
            4096,
        );
        assert!(result.is_err());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod helper_cleanup_tests {
    use super::*;
    #[test]
    fn successful_parent_does_not_leave_its_helper_running() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("helper.pid");
        run_native_command(Command::new("/bin/sh")
            .args(["-c", "/bin/sleep 20 & echo $! > \"$AH_HELPER_PID\"; exit 0"])
            .env("AH_HELPER_PID", &pid_file)).unwrap();
        let pid = fs::read_to_string(pid_file).unwrap();
        let stat = format!("/proc/{}/stat", pid.trim());
        for _ in 0..50 {
            let running = fs::read_to_string(&stat).ok().is_some_and(|s| {
                !s.split_once(") ").is_some_and(|(_, tail)| tail.starts_with('Z'))
            });
            if !running { return; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("background helper survived successful parent exit");
    }
}

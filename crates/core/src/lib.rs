//! Platform-neutral model and synchronous, streaming adapter contract.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{fmt, fs, path::PathBuf, str::FromStr, time::UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Claude,
    Codex,
    Opencode,
    Pi,
    Agy,
}

impl Agent {
    pub const ALL: [Self; 5] = [Self::Claude, Self::Codex, Self::Opencode, Self::Pi, Self::Agy];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Pi => "pi",
            Self::Agy => "agy",
        }
    }
}

impl fmt::Display for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Agent {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == s)
            .with_context(|| format!("unknown agent {s:?}; expected claude, codex, opencode, pi, agy"))
    }
}

/// Native identity is independent of its location and of future archive membership.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub key: String,
    pub agent: Agent,
    pub native_id: String,
    pub title: String,
    pub project_path: Option<PathBuf>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub source_path: PathBuf,
    pub parent_id: Option<String>,
    /// Opaque records remain inspectable; unknown encodings are never silently dropped.
    pub warnings: Vec<String>,
}

impl Session {
    pub fn new(agent: Agent, native_id: String, source_path: PathBuf) -> Result<Self> {
        if native_id.is_empty() || native_id.starts_with('-') || native_id.chars().any(char::is_control) {
            bail!("invalid native session identifier");
        }
        Ok(Self {
            key: format!("{agent}:{native_id}"), agent, native_id,
            title: String::new(), project_path: None, created_at: None,
            updated_at: None, source_path, parent_id: None, warnings: Vec::new(),
        })
    }

    pub fn observe_time(&mut self, value: Option<i64>) {
        if let Some(t) = value {
            self.created_at = Some(self.created_at.map_or(t, |old| old.min(t)));
            self.updated_at = Some(self.updated_at.map_or(t, |old| old.max(t)));
        }
    }

    pub fn suggest_title(&mut self, text: &str) {
        if self.title.is_empty() {
            self.title = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub ordinal: i64,
    pub native_id: Option<String>,
    pub parent_id: Option<String>,
    pub role: String,
    pub kind: String,
    pub timestamp: Option<i64>,
    pub text: String,
    /// Lossless JSON record, or an explicitly tagged encoding of a binary record.
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct Source {
    pub agent: Agent,
    pub path: PathBuf,
    /// All files influencing this source, including WALs and auxiliary metadata.
    pub dependencies: Vec<PathBuf>,
}

impl Source {
    pub fn new(agent: Agent, path: PathBuf) -> Self {
        Self { agent, dependencies: vec![path.clone()], path }
    }
    pub fn id(&self) -> Result<String> {
        let absolute = fs::canonicalize(&self.path).with_context(|| format!("resolve {}", self.path.display()))?;
        let mut h = blake3::Hasher::new();
        h.update(self.agent.as_str().as_bytes());
        h.update(absolute.as_os_str().as_encoded_bytes());
        Ok(h.finalize().to_hex().to_string())
    }
    /// Fast change detection, not a content digest. `sync --force` repairs preserved-mtime rewrites.
    pub fn fingerprint(&self) -> Result<String> {
        let mut h = blake3::Hasher::new();
        for path in &self.dependencies {
            h.update(path.as_os_str().as_encoded_bytes());
            match fs::metadata(path) {
                Ok(meta) => {
                    h.update(&meta.len().to_le_bytes());
                    let t = meta.modified()?.duration_since(UNIX_EPOCH)?.as_nanos();
                    h.update(&t.to_le_bytes());
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        h.update(&meta.ino().to_le_bytes());
                        h.update(&meta.ctime().to_le_bytes());
                        h.update(&meta.ctime_nsec().to_le_bytes());
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => { h.update(b"missing"); }
                Err(e) => return Err(e).with_context(|| format!("stat {}", path.display())),
            }
        }
        Ok(h.finalize().to_hex().to_string())
    }
}

#[derive(Debug, Default)]
pub struct Discovery {
    pub sources: Vec<Source>,
    pub warnings: Vec<String>,
    /// Only a complete traversal can mark previously indexed sources unavailable.
    pub complete: bool,
}

/// Each source is ingested in one transaction. Only one record need be held in memory.
pub trait SessionSink {
    fn begin(&mut self, session: &Session) -> Result<()>;
    fn message(&mut self, message: &Message) -> Result<()>;
    fn finish(&mut self, session: &Session) -> Result<()>;
}

pub trait Adapter {
    fn agent(&self) -> Agent;
    fn root(&self) -> &std::path::Path;
    fn discover(&self) -> Result<Discovery>;
    fn read(&self, source: &Source, sink: &mut dyn SessionSink) -> Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, PathBuf)>,
}

/// Plan first; the CLI executes only on an explicit `resume --exec` invocation.
pub fn resume_command(session: &Session) -> Result<ResumeCommand> {
    let id = &session.native_id;
    let args = match session.agent {
        Agent::Claude => vec!["--resume".into(), id.clone()],
        Agent::Codex => vec!["resume".into(), id.clone()],
        Agent::Opencode => vec!["--session".into(), id.clone()],
        Agent::Pi => vec!["--session".into(), session.source_path.to_str().context("non UTF-8 session path")?.into()],
        Agent::Agy => vec!["--conversation".into(), id.clone()],
    };
    Ok(ResumeCommand { program: session.agent.as_str().into(), args, cwd: session.project_path.clone(), env: Vec::new() })
}

pub fn timestamp(value: &serde_json::Value) -> Option<i64> {
    if let Some(t) = value.as_i64() {
        // Agent numeric timestamps are milliseconds since the epoch.
        return Some(t);
    }
    value.as_str().and_then(|s| {
        chrono::DateTime::parse_from_rfc3339(s).ok().map(|v| v.timestamp_millis())
            .or_else(|| chrono::DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f%:z").ok().map(|v| v.timestamp_millis()))
            .or_else(|| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f").ok().map(|v| v.and_utc().timestamp_millis()))
    })
}

/// Reserved archive boundary. No implementation, schema, or CLI in phase one.
pub mod archive {
    use anyhow::Result;
    use serde::{Deserialize, Serialize};
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ArchiveDraft {
        pub session_key: String,
        pub project_name: String,
        pub folders: Vec<String>,
        pub title: Option<String>,
        pub note: Option<String>,
    }
    pub trait ArchiveRepository {
        fn add(&mut self, draft: ArchiveDraft) -> Result<String>;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_resume_commands_are_argument_arrays() {
        for (agent,flag) in [(Agent::Claude,"--resume"),(Agent::Codex,"resume"),(Agent::Opencode,"--session"),(Agent::Pi,"--session"),(Agent::Agy,"--conversation")] {
            let session = Session::new(agent,"native-id".into(),PathBuf::from("/tmp/source.jsonl")).unwrap();
            let command = resume_command(&session).unwrap();
            assert_eq!(command.program,agent.as_str());
            assert_eq!(command.args[0],flag);
            assert_eq!(command.args[1],if agent == Agent::Pi { "/tmp/source.jsonl" } else { "native-id" });
        }
        assert!(Session::new(Agent::Agy,"--flag".into(),PathBuf::new()).is_err());
        assert!(Session::new(Agent::Agy,"bad\nidentifier".into(),PathBuf::new()).is_err());
    }
    #[test]
    fn timestamp_formats_preserve_timezone_and_milliseconds() {
        let expected = timestamp(&serde_json::json!("2026-11-12T00:00:00Z"));
        assert_eq!(expected,timestamp(&serde_json::json!("2026-11-12 08:00:00+08:00")));
        assert_eq!(timestamp(&serde_json::json!(1234567)),Some(1234567));
        assert_eq!(timestamp(&serde_json::json!("not a timestamp")),None);
    }
}

use crate::{content, discover_files, string};
use agenthub_core::{timestamp, Adapter, Agent, Discovery, Message, Session, SessionSink, Source};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{fs::File, io::{BufRead, BufReader, Read}, path::{Path, PathBuf}};

/// Bound a single record to 32 MiB; the overall transcript is streamed.
const MAX_RECORD: u64 = 32 * 1024 * 1024;

pub struct JsonlAdapter { agent: Agent, root: PathBuf }
impl JsonlAdapter {
    pub fn new(agent: Agent, root: PathBuf) -> Self { Self { agent, root } }
}

impl Adapter for JsonlAdapter {
    fn agent(&self) -> Agent { self.agent }
    fn root(&self) -> &Path { &self.root }
    fn discover(&self) -> Result<Discovery> {
        if self.agent != Agent::Codex {
            // Claude's nested subagent transcripts share their parent's ID; don't overwrite the main conversation.
            return discover_files(self.agent, &self.root, "jsonl", if self.agent == Agent::Claude { 2 } else { usize::MAX });
        }
        let mut result = Discovery { complete: true, ..Discovery::default() };
        for folder in ["sessions", "archived_sessions"] {
            let mut found = discover_files(self.agent, &self.root.join(folder), "jsonl", usize::MAX)?;
            result.complete &= found.complete;
            result.sources.append(&mut found.sources);
            result.warnings.append(&mut found.warnings);
        }
        Ok(result)
    }

    fn read(&self, source: &Source, sink: &mut dyn SessionSink) -> Result<()> {
        let mut input = BufReader::new(File::open(&source.path)?);
        let mut session: Option<Session> = None;
        let mut line = Vec::new();
        let mut ordinal = 0_i64;
        let mut line_no = 0;
        loop {
            line.clear();
            let n = input.by_ref().take(MAX_RECORD + 1).read_until(b'\n', &mut line)?;
            if n == 0 { break; }
            line_no += 1;
            if n as u64 > MAX_RECORD { bail!("{}:{line_no}: record exceeds 32 MiB", source.path.display()); }
            if line.iter().all(u8::is_ascii_whitespace) { continue; }
            let value: Value = match serde_json::from_slice(&line) {
                Ok(v) => v,
                Err(_) if !line.ends_with(b"\n") && input.fill_buf()?.is_empty() => {
                    if let Some(s) = &mut session {
                        s.warnings.push(format!("incomplete final JSONL record at line {line_no}; retry after append"));
                        break;
                    }
                    bail!("incomplete session header");
                }
                Err(e) => return Err(e).with_context(|| format!("{}:{line_no}", source.path.display())),
            };
            if !value.is_object() { bail!("line {line_no}: expected a JSON object"); }
            let kind = value["type"].as_str().unwrap_or("unknown");
            if session.is_none() {
                let id = match self.agent {
                    Agent::Claude => string(&value, "sessionId").or_else(|| source.path.file_stem().and_then(|s| s.to_str()).map(str::to_owned)),
                    Agent::Codex if kind == "session_meta" => string(&value["payload"], "id"),
                    Agent::Pi if kind == "session" => string(&value, "id"),
                    _ => None,
                }.context("missing or unsupported session header")?;
                let mut s = Session::new(self.agent, id, source.path.clone())?;
                let header = if self.agent == Agent::Codex { &value["payload"] } else { &value };
                s.project_path = string(header, "cwd").map(PathBuf::from);
                s.parent_id = string(header, "parentSession").or_else(|| string(header, "forked_from_id"));
                s.observe_time(timestamp(&value["timestamp"]).or_else(|| timestamp(&header["timestamp"])));
                if self.agent == Agent::Pi && header["version"].as_u64().is_some_and(|v| v > 3) {
                    s.warnings.push("Pi session version newer than v3; raw events preserved".into());
                }
                sink.begin(&s)?;
                session = Some(s);
            }
            let s = session.as_mut().context("missing session")?;
            if self.agent == Agent::Claude {
                if let Some(id) = value["sessionId"].as_str() {
                    if id != s.native_id { bail!("mixed Claude session IDs in one transcript"); }
                }
                if let Some(cwd) = string(&value, "cwd") { s.project_path = Some(cwd.into()); }
                if let Some(title) = string(&value, "customTitle") { s.title = title; }
                if kind == "summary" && s.title.is_empty() { s.title = string(&value, "summary").unwrap_or_default(); }
            }
            if self.agent == Agent::Pi && kind == "session_info" {
                if let Some(title) = string(&value, "name") { s.title = title; }
            }
            if self.agent == Agent::Codex && kind == "turn_context" && s.project_path.is_none() {
                s.project_path = string(&value["payload"], "cwd").map(PathBuf::from);
            }
            let time = timestamp(&value["timestamp"]);
            s.observe_time(time);
            let (role, text) = match self.agent {
                Agent::Claude | Agent::Pi if value["message"].is_object() => {
                    let message = &value["message"];
                    let native_role = message["role"].as_str().unwrap_or("system");
                    let tool_result = message["content"].as_array().is_some_and(|xs| !xs.is_empty() && xs.iter().all(|v|v["type"] == "tool_result"));
                    let role = if tool_result || matches!(native_role,"toolResult"|"tool_result"|"bashExecution") { "tool" } else { native_role };
                    (role.to_owned(),content(message.get("content").unwrap_or(message)))
                }
                Agent::Codex if kind == "response_item" => {
                    let item = &value["payload"];
                    match item["type"].as_str().unwrap_or("") {
                        "message" => (item["role"].as_str().unwrap_or("system").into(), content(&item["content"])),
                        "function_call" | "custom_tool_call" | "local_shell_call" => ("tool_call".into(), item.to_string()),
                        "function_call_output" | "custom_tool_call_output" => ("tool".into(), content(&item["output"])),
                        "reasoning" => ("assistant".into(), content(&item["summary"])),
                        _ => ("system".into(), item.to_string()),
                    }
                }
                // event_msg mirrors response_item; retain it as an event without duplicating conversation text/search hits.
                Agent::Codex if kind == "event_msg" => ("event".into(), value["payload"].to_string()),
                _ => ("system".into(), content(value.get("summary").unwrap_or(&value))),
            };
            if role == "user" { s.suggest_title(&text); }
            sink.message(&Message {
                ordinal, native_id: string(&value, "uuid").or_else(|| string(&value, "id")),
                parent_id: string(&value, "parentUuid").or_else(|| string(&value, "parentId")),
                role, kind: kind.into(), timestamp: time, text, raw: value,
            })?;
            ordinal += 1;
        }
        let mut s = session.context("empty session file")?;
        if s.title.is_empty() { s.title = s.native_id.clone(); }
        sink.finish(&s)
    }
}

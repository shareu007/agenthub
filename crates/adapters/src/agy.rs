use crate::{discover_files, opencode::validate_component, protobuf, sqlite::{self, has_column, Snapshot}};
use agenthub_core::{timestamp, Adapter, Agent, Discovery, Message, Session, SessionSink, Source};
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{cell::RefCell, path::{Path, PathBuf}};

pub struct AgyAdapter { root: PathBuf, summaries: RefCell<Option<(String, Snapshot)>> }
impl AgyAdapter {
    pub fn new(root: PathBuf) -> Self { Self { root, summaries: RefCell::new(None) } }
}

impl Adapter for AgyAdapter {
    fn agent(&self) -> Agent { Agent::Agy }
    fn root(&self) -> &Path { &self.root }
    fn discover(&self) -> Result<Discovery> {
        let mut found = discover_files(Agent::Agy, &self.root.join("conversations"), "db", 1)?;
        let summary = self.root.join("conversation_summaries.db");
        for item in &mut found.sources {
            *item = sqlite::source(Agent::Agy, item.path.clone());
            item.dependencies.extend(sqlite::source(Agent::Agy, summary.clone()).dependencies);
        }
        // Old IDE .pb files are deliberately not interpreted as CLI conversations.
        Ok(found)
    }
    fn read(&self, source: &Source, sink: &mut dyn SessionSink) -> Result<()> {
        let id = source.path.file_stem().and_then(|s| s.to_str()).context("invalid AGY conversation filename")?;
        validate_component(id)?;
        let mut session = Session::new(Agent::Agy, id.into(), source.path.clone())?;
        let summaries = self.root.join("conversation_summaries.db");
        if summaries.try_exists()? {
            let fingerprint = sqlite::source(Agent::Agy,summaries.clone()).fingerprint()?;
            let mut cache = self.summaries.borrow_mut();
            if cache.as_ref().is_none_or(|(f,_)| f != &fingerprint) {
                *cache = Some((fingerprint,Snapshot::open(&summaries)?));
            }
            let summary = &cache.as_ref().context("missing AGY summary snapshot")?.1;
            let parent = if has_column(&summary.conn, "conversation_summaries", "parent_conversation_id")? { "parent_conversation_id" } else { "NULL" };
            let mut stmt = summary.conn.prepare(&format!("SELECT title,workspace_uris,last_modified_time,{parent} FROM conversation_summaries WHERE conversation_id=?1"))?;
            let mut rows = stmt.query([id])?;
            if let Some(row) = rows.next()? {
                session.title = row.get(0)?;
                let workspaces: Value = serde_json::from_str(&row.get::<_, String>(1)?).context("invalid AGY workspace_uris")?;
                session.project_path = workspaces.as_array().and_then(|xs| xs.iter().filter_map(Value::as_str).find_map(file_uri));
                let modified: String = row.get(2)?;
                session.updated_at = timestamp(&Value::String(modified));
                session.parent_id = row.get::<_, Option<String>>(3)?.filter(|s| !s.is_empty());
            }
        }
        let snapshot = Snapshot::open(&source.path)?;
        sink.begin(&session)?;
        // Rows stay in native trajectory order. Each original column is retained in raw.
        let mut stmt = snapshot.conn.prepare("SELECT idx,step_type,step_format,step_payload,metadata,error_details,permissions,task_details,render_info,status,has_subtrajectory FROM steps ORDER BY idx")?;
        let mut rows = stmt.query([])?;
        let mut opaque = false;
        while let Some(row) = rows.next()? {
            let ordinal: i64 = row.get(0)?;
            let kind: i64 = row.get(1)?;
            let format: i64 = row.get(2)?;
            let payload = row.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default();
            let mut raw = json!({"encoding":"agy-protobuf","step_type":kind,"step_format":format,
                "step_payload_base64":STANDARD.encode(&payload),"status":row.get::<_, i64>(9)?,
                "has_subtrajectory":row.get::<_, bool>(10)?});
            for (column, name) in [(4,"metadata"),(5,"error_details"),(6,"permissions"),(7,"task_details"),(8,"render_info")] {
                raw[name] = row.get::<_, Option<Vec<u8>>>(column)?.map(|v| Value::String(STANDARD.encode(v))).unwrap_or(Value::Null);
            }
            let (role, text, time) = if format == 0 && !payload.is_empty() {
                let decoded = (|| -> Result<_> {
                    let time = protobuf::integer_at(&payload, &[5,1,1])?.and_then(|v| i64::try_from(v).ok()).and_then(|v| v.checked_mul(1000));
                    let (role, text) = match kind {
                        14 => ("user", protobuf::text_at(&payload, &[19,2])?.or(protobuf::text_at(&payload, &[19,3,1])?).context("unknown AGY user input encoding")?),
                        15 => ("assistant", assistant_text(&payload)?),
                        _ => {
                            // Non-chat steps are a schema-neutral wire tree, including tool inputs/results.
                            let mut values = Vec::new();
                            for (field, value) in protobuf::fields(&payload)? {
                                if field > 5 {
                                    if let protobuf::Wire::Bytes(b) = value { values.push(json!({"field":field,"wire":protobuf::wire_json(b,0)})); }
                                }
                            }
                            ("tool", serde_json::to_string(&values)?)
                        }
                    };
                    Ok((role, text, time))
                })();
                match decoded {
                    Ok(v) => v,
                    Err(_) => { opaque = true; ("event", format!("[opaque AGY step {kind}; use show --raw]"), None) }
                }
            } else {
                opaque = true;
                ("event", format!("[unsupported AGY step format {format}; use show --raw]"), None)
            };
            session.observe_time(time);
            if role == "user" { session.suggest_title(&text); }
            sink.message(&Message { ordinal, native_id: Some(ordinal.to_string()), parent_id: None,
                role: role.into(), kind: format!("agy_step_{kind}"), timestamp: time, text, raw })?;
        }
        if opaque { session.warnings.push("some AGY steps have unsupported binary encodings; raw bytes retained".into()); }
        session.warnings.push("AGY tool steps use a protobuf wire tree; attachment/trajectory side tables are not decoded".into());
        if session.title.is_empty() { session.title = session.native_id.clone(); }
        sink.finish(&session)
    }
}

/// Extract only visible prose for handoff from the already retained raw bytes.
/// No second decoded-reply cache is added to the index; older indexed records
/// retain the same binary contract and remain usable.
pub fn agy_assistant_reply(raw: &Value) -> Result<Option<String>> {
    anyhow::ensure!(raw["encoding"] == "agy-protobuf" && raw["step_type"] == 15 && raw["step_format"] == 0,
        "unsupported AGY assistant record for transfer");
    let encoded = raw["step_payload_base64"].as_str().context("missing AGY payload for transfer")?;
    anyhow::ensure!(encoded.len() <= (32usize * 1024 * 1024).div_ceil(3) * 4, "AGY transfer payload exceeds 32 MiB");
    let payload = STANDARD.decode(encoded).context("invalid AGY transfer payload")?;
    if let Some(reply) = protobuf::text_at(&payload, &[20,8])? { return Ok(Some(reply)); }
    protobuf::text_at(&payload, &[20,1])
}

fn assistant_text(payload: &[u8]) -> Result<String> {
    let mut parts = Vec::new();
    if let Some(thinking) = protobuf::text_at(payload, &[20, 3])?.filter(|s| !s.is_empty()) {
        parts.push(thinking);
    }
    if let Some(reply) = protobuf::text_at(payload, &[20, 8])?.or(protobuf::text_at(payload, &[20, 1])?).filter(|s| !s.is_empty()) {
        parts.push(reply);
    }
    if let Some(body) = protobuf::bytes_at(payload, &[20])? {
        for (field, value) in protobuf::fields(body)? {
            if field == 7 {
                if let protobuf::Wire::Bytes(call) = value {
                    let name = protobuf::text_at(call, &[2])?.context("missing AGY tool call name")?;
                    parts.push(json!({"id": protobuf::text_at(call, &[1])?, "name": name, "arguments": protobuf::text_at(call, &[3])?}).to_string());
                }
            }
        }
    }
    anyhow::ensure!(!parts.is_empty(), "unknown AGY assistant encoding");
    Ok(parts.join("\n"))
}

/// Local file URI only, percent-decoded without normalizing case or symlinks.
fn file_uri(uri: &str) -> Option<PathBuf> {
    let value = uri.strip_prefix("file://")?;
    let value = value.strip_prefix("localhost").unwrap_or(value);
    if !value.starts_with('/') { return None; }
    let mut bytes = Vec::new();
    let mut it = value.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let hi = (it.next()? as char).to_digit(16)?;
            let lo = (it.next()? as char).to_digit(16)?;
            bytes.push((hi * 16 + lo) as u8);
        } else { bytes.push(b); }
    }
    if bytes.contains(&0) { return None; }
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uri_decode() {
        assert_eq!(file_uri("file:///tmp/my%20project"), Some(PathBuf::from("/tmp/my project")));
        assert_eq!(file_uri("file://localhost/tmp/a"), Some(PathBuf::from("/tmp/a")));
        assert_eq!(file_uri("file://remote/tmp/a"), None);
        assert_eq!(file_uri("file:///tmp/%00"), None);
    }
}

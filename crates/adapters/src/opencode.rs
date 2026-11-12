use crate::{content, discover_files, sqlite::{self, has_column, table_exists, Snapshot}, string};
use agenthub_core::{timestamp, Adapter, Agent, Discovery, Message, Session, SessionSink, Source};
use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{fs, path::{Path, PathBuf}};

pub struct OpenCodeAdapter { root: PathBuf }
impl OpenCodeAdapter {
    pub fn new(root: PathBuf) -> Self { Self { root } }

    fn read_database(&self, source: &Source, sink: &mut dyn SessionSink) -> Result<()> {
        let snapshot = Snapshot::open(&source.path)?;
        let c = &snapshot.conn;
        let v2 = table_exists(c, "session_v2")?;
        let table = if v2 { "session_v2" } else { "session" };
        if !table_exists(c, table)? { bail!("unsupported OpenCode schema: no session/session_v2 table"); }
        let parent = if has_column(c, table, "parent_id")? { "parent_id" } else { "NULL" };
        let sql = format!("SELECT id,COALESCE(title,''),directory,time_created,time_updated,{parent} FROM {table} ORDER BY time_created,id");
        let mut stmt = c.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let mut session = Session::new(Agent::Opencode, row.get(0)?, source.path.clone())?;
            session.title = row.get(1)?;
            session.project_path = Some(PathBuf::from(row.get::<_, String>(2)?));
            session.created_at = row.get(3)?;
            session.updated_at = row.get(4)?;
            session.parent_id = row.get(5)?;
            sink.begin(&session)?;
            self.read_messages(c, &mut session, v2, sink)?;
            if session.title.is_empty() { session.title = session.native_id.clone(); }
            sink.finish(&session)?;
        }
        Ok(())
    }

    fn read_messages(&self, c: &Connection, session: &mut Session, v2: bool, sink: &mut dyn SessionSink) -> Result<()> {
        let modern = v2 || !table_exists(c, "message")?;
        let table = if modern { "session_message" } else { "message" };
        if !table_exists(c, table)? { bail!("unsupported OpenCode schema: no message table"); }
        let role_column = if modern { "type" } else { "'message'" };
        let order = if modern && has_column(c, table, "seq")? { "seq,id" } else { "time_created,id" };
        let sql = if !modern && table_exists(c, "session_message")? {
            "SELECT id,'message' AS kind,time_created,data FROM message WHERE session_id=?1 UNION ALL SELECT id,type AS kind,time_created,data FROM session_message WHERE session_id=?1 ORDER BY time_created,id".to_owned()
        } else {
            format!("SELECT id,{role_column},time_created,data FROM {table} WHERE session_id=?1 ORDER BY {order}")
        };
        let mut stmt = c.prepare(&sql)?;
        let mut rows = stmt.query([&session.native_id])?;
        let mut ordinal = 0;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let kind: String = row.get(1)?;
            let data: Value = serde_json::from_str(&row.get::<_, String>(3)?).context("invalid OpenCode message JSON")?;
            let role = string(&data, "role").unwrap_or_else(|| match kind.as_str() {
                "user" => "user".into(), "assistant" => "assistant".into(), "shell" => "tool".into(), _ => "system".into(),
            });
            let mut text = content(&data["text"]);
            let mut parts = Vec::new();
            let part_table = if v2 && table_exists(c, "session_part")? { Some("session_part") }
                else if kind == "message" && table_exists(c, "part")? { Some("part") } else { None };
            if let Some(pt) = part_table {
                let part_order = if pt == "session_part" { "ordinal" } else { "time_created,id" };
                let mut ps = c.prepare(&format!("SELECT data FROM {pt} WHERE message_id=?1 ORDER BY {part_order}"))?;
                for p in ps.query_map([&id], |r| r.get::<_, String>(0))? {
                    let part: Value = serde_json::from_str(&p?).context("invalid OpenCode part JSON")?;
                    if !text.is_empty() { text.push('\n'); }
                    text.push_str(&content(&part));
                    parts.push(part);
                }
            }
            if parts.is_empty() && text.is_empty() {
                text = content(data.get("content").or_else(|| data.get("parts")).unwrap_or(&data));
            }
            if role == "user" { session.suggest_title(&text); }
            sink.message(&Message { ordinal, native_id: Some(id), parent_id: string(&data, "parentID"),
                role, kind, timestamp: row.get(2)?, text, raw: json!({"info": data, "parts": parts}) })?;
            ordinal += 1;
        }
        Ok(())
    }

    fn legacy_related(&self, session_path: &Path) -> Result<Vec<PathBuf>> {
        let info: Value = serde_json::from_slice(&fs::read(session_path)?)?;
        let id = string(&info, "id").context("missing legacy OpenCode id")?;
        validate_component(&id)?;
        let storage = self.root.join("storage");
        let mut paths = vec![session_path.to_path_buf()];
        for path in json_files(&storage.join("message").join(&id))? {
            let mid = path.file_stem().and_then(|s| s.to_str()).context("non UTF-8 message id")?;
            paths.extend(json_files(&storage.join("part").join(mid))?);
            paths.extend(json_files(&storage.join("part").join(&id).join(mid))?);
            paths.push(path);
        }
        // Directory metadata detects newly created/deleted messages and parts.
        paths.push(storage.join("message").join(&id));
        paths.push(storage.join("part").join(&id));
        for p in paths.clone() {
            if p.extension().is_some_and(|e| e == "json") {
                if let Some(mid) = p.file_stem() { paths.push(storage.join("part").join(mid)); }
            }
        }
        paths.sort(); paths.dedup();
        Ok(paths)
    }

    fn read_legacy(&self, source: &Source, sink: &mut dyn SessionSink) -> Result<()> {
        let info: Value = serde_json::from_slice(&fs::read(&source.path)?)?;
        let id = string(&info, "id").context("missing legacy session id")?;
        validate_component(&id)?;
        let mut s = Session::new(Agent::Opencode, id, source.path.clone())?;
        s.title = string(&info, "title").unwrap_or_default();
        s.project_path = string(&info, "directory").map(PathBuf::from);
        s.created_at = timestamp(&info["time"]["created"]);
        s.updated_at = timestamp(&info["time"]["updated"]);
        s.parent_id = string(&info, "parentID");
        sink.begin(&s)?;
        let storage = self.root.join("storage");
        for (ordinal, path) in json_files(&storage.join("message").join(&s.native_id))?.iter().enumerate() {
            let msg: Value = serde_json::from_slice(&fs::read(path)?)?;
            let mid = string(&msg, "id").context("missing legacy message id")?;
            validate_component(&mid)?;
            let mut paths = json_files(&storage.join("part").join(&mid))?;
            paths.extend(json_files(&storage.join("part").join(&s.native_id).join(&mid))?);
            paths.sort(); paths.dedup();
            let mut parts = Vec::new();
            for path in paths { parts.push(serde_json::from_slice::<Value>(&fs::read(path)?)?); }
            let text = if parts.is_empty() { content(&msg["parts"]) } else { content(&Value::Array(parts.clone())) };
            let role = string(&msg,"role").unwrap_or_else(||"system".into());
            if role == "user" { s.suggest_title(&text); }
            sink.message(&Message { ordinal: ordinal as i64, native_id: Some(mid), parent_id: string(&msg, "parentID"),
                role, kind: "message".into(),
                timestamp: timestamp(&msg["time"]["created"]), text, raw: json!({"info":msg,"parts":parts}) })?;
        }
        if s.title.is_empty() { s.title = s.native_id.clone(); }
        sink.finish(&s)
    }
}

impl Adapter for OpenCodeAdapter {
    fn agent(&self) -> Agent { Agent::Opencode }
    fn root(&self) -> &Path { &self.root }
    fn discover(&self) -> Result<Discovery> {
        let db = if self.root.is_file() { self.root.clone() } else { self.root.join("opencode.db") };
        if db.try_exists()? {
            return Ok(Discovery { sources: vec![sqlite::source(Agent::Opencode, db)], complete: true, warnings: vec![] });
        }
        let mut result = discover_files(Agent::Opencode, &self.root.join("storage/session"), "json", 2)?;
        for source in &mut result.sources { source.dependencies = self.legacy_related(&source.path)?; }
        Ok(result)
    }
    fn read(&self, source: &Source, sink: &mut dyn SessionSink) -> Result<()> {
        if source.path.extension().is_some_and(|e| e == "json") { self.read_legacy(source, sink) }
        else { self.read_database(source, sink) }
    }
}

pub(crate) fn validate_component(id: &str) -> Result<()> {
    if id.is_empty() || id == "." || id == ".." || id.contains(['/', '\\']) { bail!("unsafe native identifier"); }
    Ok(())
}

fn json_files(path: &Path) -> Result<Vec<PathBuf>> {
    if !path.try_exists()? { return Ok(vec![]); }
    let mut paths = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|e| e == "json") { paths.push(entry.path()); }
    }
    paths.sort();
    Ok(paths)
}

/// Read-only native deletion preflight/verification for the verified v1 store.
/// Reuse the WAL-aware private snapshot so previews never create source SHM files.
pub fn deletion_status(path: &Path, id: &str) -> Result<(bool, i64)> {
    if !path.try_exists()? { return Ok((false, 0)); }
    let snapshot = Snapshot::open(path)?;
    let c = &snapshot.conn;
    if !table_exists(c, "session")? || table_exists(c, "session_v2")? {
        bail!("native deletion requires a verified OpenCode v1 store; index retained");
    }
    let exists = c.query_row("SELECT 1 FROM session WHERE id=?1", [id], |_| Ok(())).optional()?.is_some();
    let children = c.query_row("SELECT count(*) FROM session WHERE parent_id=?1", [id], |r| r.get(0))?;
    Ok((exists, children))
}

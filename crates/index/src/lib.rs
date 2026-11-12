//! Local derived index. No agent-specific parsing and no source-store writes.
use agenthub_core::{Adapter, Agent, Message, Session, SessionSink, Source};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::{Path, PathBuf}, time::Duration};

const APP_ID: i64 = 0x41485542;
const SCHEMA_VERSION: i64 = 1;
const PARSER_VERSION: i64 = 1;
const SESSION_COLUMNS: &str = "s.key,s.native_id,s.agent,s.title,s.project_path,s.created_at,s.updated_at,s.source_path,s.parent_id,s.warnings,s.available,s.message_count";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    #[serde(flatten)]
    pub session: Session,
    pub available: bool,
    pub message_count: i64,
}

#[derive(Debug, Clone)]
pub struct Filter {
    pub agent: Option<Agent>,
    pub project: Option<String>,
    /// Updated timestamp in Unix milliseconds; since inclusive, until exclusive.
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub include_missing: bool,
    pub limit: u32,
    pub offset: u32,
}

impl Default for Filter {
    fn default() -> Self {
        Self { agent: None, project: None, since: None, until: None, include_missing: false, limit: 50, offset: 0 }
    }
}

impl Filter {
    pub fn recent() -> Self { Self::default() }
    fn args(&self) -> [rusqlite::types::Value; 7] {
        use rusqlite::types::Value;
        [self.agent.map(|a| Value::Text(a.to_string())).unwrap_or(Value::Null),
         self.project.clone().map(Value::Text).unwrap_or(Value::Null),
         self.since.map(Value::Integer).unwrap_or(Value::Null),
         self.until.map(Value::Integer).unwrap_or(Value::Null),
         Value::Integer(i64::from(self.include_missing)),
         Value::Integer(i64::from(self.limit)), Value::Integer(i64::from(self.offset))]
    }
}

const WHERE_FILTER: &str = "(?1 IS NULL OR s.agent=?1) AND (?2 IS NULL OR s.project_path=?2) AND (?3 IS NULL OR s.updated_at>=?3) AND (?4 IS NULL OR s.updated_at<?4) AND (?5 OR s.available=1)";

#[derive(Debug, Serialize)]
pub struct SearchHit {
    pub key: String,
    pub agent: Agent,
    pub title: String,
    /// -1 is the title/project metadata document, otherwise the native record ordinal.
    pub ordinal: i64,
    pub excerpt: String,
}

#[derive(Debug, Default, Serialize)]
pub struct SyncReport {
    pub discovered: usize,
    pub indexed: usize,
    pub unchanged: usize,
    pub missing: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

pub struct Store { conn: Connection }

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) { fs::create_dir_all(parent)?; }
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; opts.mode(0o600); }
        match opts.open(path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).context("create index database"),
        }
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(3))?;
        let id: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
        if id != APP_ID && (id != 0 || tables != 0 || version != 0) {
            bail!("refusing to use a non-AgentHub database as the index: {}", path.display());
        }
        if version > SCHEMA_VERSION { bail!("index schema {version} is newer than supported {SCHEMA_VERSION}"); }
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA cache_size=-4096;")?;
        if version == 0 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(include_str!("schema.sql"))?;
            tx.pragma_update(None, "application_id", APP_ID)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            tx.commit()?;
        }
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        Ok(Self { conn })
    }

    pub fn sync(&mut self, adapter: &dyn Adapter, force: bool) -> Result<SyncReport> {
        let discovery = adapter.discover()?;
        let mut report = SyncReport { discovered: discovery.sources.len(), warnings: discovery.warnings, ..SyncReport::default() };
        if !discovery.complete { report.errors.push("source discovery was incomplete; previous availability is preserved".into()); }
        let mut seen = HashSet::new();
        let root = path_text(adapter.root())?;
        for source in discovery.sources {
            let result = (|| -> Result<()> {
                let id = source.id()?;
                seen.insert(id.clone());
                let fingerprint = source.fingerprint()?;
                let previous: Option<(String, i64, bool)> = self.conn.query_row(
                    "SELECT fingerprint,parser_version,present FROM sources WHERE id=?1", [&id],
                    |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
                ).optional()?;
                if !force && previous.is_some_and(|(f,v,p)| f == fingerprint && v == PARSER_VERSION && p) {
                    report.unchanged += 1;
                    return Ok(());
                }
                self.ingest(adapter, &source, &id, &fingerprint, &root)?;
                report.indexed += 1;
                Ok(())
            })();
            if let Err(error) = result { report.errors.push(format!("{}: {error:#}", source.path.display())); }
        }
        // Traversal failures preserve previous availability rather than hiding data.
        if discovery.complete && report.errors.is_empty() {
            let mut stmt = self.conn.prepare("SELECT id FROM sources WHERE agent=?1 AND root=?2 AND present=1")?;
            let ids: Vec<String> = stmt.query_map(params![adapter.agent().as_str(),root], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for id in ids {
                if !seen.contains(&id) {
                    tx.execute("UPDATE sources SET present=0 WHERE id=?1", [&id])?;
                    tx.execute("UPDATE sessions SET available=0 WHERE source_id=?1", [&id])?;
                    report.missing += 1;
                }
            }
            tx.commit()?;
        }
        Ok(report)
    }

    fn ingest(&mut self, adapter: &dyn Adapter, source: &Source, id: &str, fingerprint: &str, root: &str) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO sources(id,agent,root,path,fingerprint,parser_version,present,synced_at) VALUES(?1,?2,?3,?4,?5,?6,1,?7) ON CONFLICT(id) DO UPDATE SET root=excluded.root,fingerprint=excluded.fingerprint,parser_version=excluded.parser_version,present=1,synced_at=excluded.synced_at",
            params![id,source.agent.as_str(),root,path_text(&source.path)?,fingerprint,PARSER_VERSION,chrono::Utc::now().timestamp_millis()])?;
        tx.execute("UPDATE sessions SET available=0 WHERE source_id=?1", [id])?;
        {
            let mut writer = Writer { tx: &tx, source_id: id, current: None, count: 0 };
            adapter.read(source, &mut writer)?;
            if writer.current.is_some() { bail!("adapter did not finish the current session"); }
        }
        if fingerprint != source.fingerprint()? { bail!("source changed during indexing; transaction rolled back, retry sync"); }
        tx.commit()?;
        Ok(())
    }

    pub fn list(&self, filter: &Filter) -> Result<Vec<SessionRecord>> {
        let sql = format!("SELECT {SESSION_COLUMNS} FROM sessions s WHERE {WHERE_FILTER} ORDER BY s.updated_at DESC,s.key LIMIT ?6 OFFSET ?7");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(filter.args()), session_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn find(&self, key: &str) -> Result<SessionRecord> {
        let exact = self.conn.query_row(&format!("SELECT {SESSION_COLUMNS} FROM sessions s WHERE s.key=?1"), [key], session_row).optional()?;
        if let Some(record) = exact { return Ok(record); }
        let mut stmt = self.conn.prepare(&format!("SELECT {SESSION_COLUMNS} FROM sessions s WHERE s.native_id=?1 LIMIT 2"))?;
        let matches = stmt.query_map([key], session_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        match matches.len() {
            0 => bail!("session {key:?} not found; run agenthub sync first"),
            1 => matches.into_iter().next().context("missing match"),
            _ => bail!("ambiguous session id; use the full agent:id key from list"),
        }
    }

    pub fn visit_messages(&self, key: &str, offset: u32, limit: Option<u32>, mut visit: impl FnMut(Message) -> Result<()>) -> Result<()> {
        let mut stmt = self.conn.prepare("SELECT ordinal,native_id,parent_id,role,kind,timestamp,body,raw FROM messages WHERE session_key=?1 ORDER BY ordinal LIMIT ?2 OFFSET ?3")?;
        let mut rows = stmt.query(params![key,limit.map(i64::from).unwrap_or(-1),i64::from(offset)])?;
        while let Some(row) = rows.next()? {
            visit(Message { ordinal:row.get(0)?,native_id:row.get(1)?,parent_id:row.get(2)?,role:row.get(3)?,kind:row.get(4)?,timestamp:row.get(5)?,text:row.get(6)?,raw:serde_json::from_str(&row.get::<_,String>(7)?)? })?;
        }
        Ok(())
    }

    pub fn search(&self, query: &str, filter: &Filter) -> Result<Vec<SearchHit>> {
        if query.trim().is_empty() { bail!("search query must not be empty"); }
        let long = query.chars().count() >= 3;
        let (from, condition, excerpt, order) = if long {
            ("documents_fts JOIN documents d ON d.id=documents_fts.rowid JOIN sessions s ON s.key=d.session_key", "documents_fts MATCH ?8", "snippet(documents_fts,0,'[',']',' … ',32)", "bm25(documents_fts),s.updated_at DESC,d.id")
        } else {
            ("documents d JOIN sessions s ON s.key=d.session_key", "instr(lower(d.body),lower(?8))>0", "substr(d.body,max(1,instr(lower(d.body),lower(?8))-60),180)", "s.updated_at DESC,d.id")
        };
        let sql = format!("SELECT s.key,s.agent,s.title,d.ordinal,{excerpt} FROM {from} WHERE {WHERE_FILTER} AND {condition} ORDER BY {order} LIMIT ?6 OFFSET ?7");
        let mut args = filter.args().to_vec();
        args.push(rusqlite::types::Value::Text(if long { format!("\"{}\"",query.replace('"',"\"\"")) } else { query.into() }));
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(args))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(SearchHit { key:row.get(0)?,agent:row.get::<_,String>(1)?.parse()?,title:row.get(2)?,ordinal:row.get(3)?,excerpt:row.get(4)? });
        }
        Ok(out)
    }

    pub fn source_root(&self, key: &str) -> Result<PathBuf> {
        Ok(PathBuf::from(self.conn.query_row("SELECT root FROM sources JOIN sessions ON sources.id=sessions.source_id WHERE sessions.key=?1", [key], |r| r.get::<_,String>(0))?))
    }
}

fn path_text(path: &Path) -> Result<String> { path.to_str().map(str::to_owned).context("non UTF-8 paths are not supported by the SQLite index") }

fn session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    let agent: String = row.get(2)?;
    let warnings: String = row.get(9)?;
    let convert = |e: Box<dyn std::error::Error + Send + Sync>| rusqlite::Error::FromSqlConversionFailure(0,rusqlite::types::Type::Text,e);
    Ok(SessionRecord { session: Session {
        key:row.get(0)?,native_id:row.get(1)?,agent:agent.parse().map_err(|e: anyhow::Error| convert(e.into()))?,title:row.get(3)?,
        project_path:row.get::<_,Option<String>>(4)?.map(PathBuf::from),created_at:row.get(5)?,updated_at:row.get(6)?,source_path:PathBuf::from(row.get::<_,String>(7)?),parent_id:row.get(8)?,
        warnings:serde_json::from_str(&warnings).map_err(|e| convert(Box::new(e)))?,
    },available:row.get(10)?,message_count:row.get(11)? })
}

struct Writer<'a, 'conn> { tx: &'a Transaction<'conn>, source_id: &'a str, current: Option<String>, count: i64 }
impl Writer<'_, '_> {
    fn put_session(&self, s: &Session) -> Result<()> {
        self.tx.execute("INSERT INTO sessions(key,native_id,agent,source_id,title,project_path,created_at,updated_at,source_path,parent_id,warnings,available,message_count) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,1,?12) ON CONFLICT(key) DO UPDATE SET source_id=excluded.source_id,title=excluded.title,project_path=excluded.project_path,created_at=excluded.created_at,updated_at=excluded.updated_at,source_path=excluded.source_path,parent_id=excluded.parent_id,warnings=excluded.warnings,available=1,message_count=excluded.message_count",
            params![s.key,s.native_id,s.agent.as_str(),self.source_id,s.title,s.project_path.as_deref().map(path_text).transpose()?,s.created_at,s.updated_at,path_text(&s.source_path)?,s.parent_id,serde_json::to_string(&s.warnings)?,self.count])?;
        Ok(())
    }
}
impl SessionSink for Writer<'_, '_> {
    fn begin(&mut self, session: &Session) -> Result<()> {
        if self.current.is_some() { bail!("nested session begin"); }
        self.count = 0;
        self.put_session(session)?;
        self.tx.execute("DELETE FROM documents WHERE session_key=?1", [&session.key])?;
        self.tx.execute("DELETE FROM messages WHERE session_key=?1", [&session.key])?;
        self.current = Some(session.key.clone());
        Ok(())
    }
    fn message(&mut self, m: &Message) -> Result<()> {
        let key = self.current.as_deref().context("message outside session")?;
        self.tx.prepare_cached("INSERT INTO messages(session_key,ordinal,native_id,parent_id,role,kind,timestamp,body,raw) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)")?.execute(params![key,m.ordinal,m.native_id,m.parent_id,m.role,m.kind,m.timestamp,m.text,serde_json::to_string(&m.raw)?])?;
        if !m.text.is_empty() && m.role != "event" {
            self.tx.prepare_cached("INSERT INTO documents(session_key,ordinal,body) VALUES(?1,?2,?3)")?.execute(params![key,m.ordinal,m.text])?;
        }
        self.count += 1;
        Ok(())
    }
    fn finish(&mut self, session: &Session) -> Result<()> {
        if self.current.as_deref() != Some(session.key.as_str()) { bail!("session finish mismatch"); }
        self.put_session(session)?;
        let project = session.project_path.as_deref().map(path_text).transpose()?.unwrap_or_default();
        self.tx.execute("INSERT INTO documents(session_key,ordinal,body) VALUES(?1,-1,?2)",params![session.key,format!("{}\n{}",session.title,project)])?;
        self.current = None;
        Ok(())
    }
}

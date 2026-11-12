use agenthub_core::{Agent, Source};
use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::{fs, path::{Path, PathBuf}, time::Duration};

pub(crate) fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

pub(crate) fn source(agent: Agent, path: PathBuf) -> Source {
    let mut source = Source::new(agent, path.clone());
    source.dependencies.push(sibling(&path, "-wal"));
    source.dependencies.push(sibling(&path, "-journal"));
    source
}

/// Never ask SQLite to open the original path: even mode=ro may create a -shm file.
/// Copy a stable DB+WAL pair into a private temporary directory first.
pub(crate) struct Snapshot {
    pub conn: Connection,
    _dir: tempfile::TempDir,
}

impl Snapshot {
    pub fn open(path: &Path) -> Result<Self> {
        let tracked = source(Agent::Agy, path.to_path_buf());
        for _ in 0..3 {
            let before = tracked.fingerprint()?;
            let journal = sibling(path, "-journal");
            if journal.try_exists()? && fs::metadata(&journal)?.len() > 0 {
                bail!("{} has an active rollback journal; retry after the agent commits", path.display());
            }
            let dir = tempfile::Builder::new().prefix("agenthub-read-").tempdir()?;
            let copy = dir.path().join("source.db");
            fs::copy(path, &copy).with_context(|| format!("snapshot {}", path.display()))?;
            let wal = sibling(path, "-wal");
            match fs::copy(&wal, sibling(&copy, "-wal")) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).context("copy source WAL"),
            }
            if before != tracked.fingerprint()? { continue; }
            let conn = Connection::open_with_flags(copy, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
            conn.busy_timeout(Duration::from_millis(250))?;
            conn.execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-2048; BEGIN;")?;
            return Ok(Self { conn, _dir: dir });
        }
        bail!("source {} changed while copying; retry sync when the agent is idle", path.display())
    }
}

pub(crate) fn table_exists(c: &Connection, name: &str) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)", [name], |r| r.get(0))?)
}

pub(crate) fn has_column(c: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = c.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let columns = stmt.query_map([table], |r| r.get::<_, String>(0))?;
    for col in columns { if col? == column { return Ok(true); } }
    Ok(false)
}


//! Explicit native deletion, separate from read-only discovery and display metadata.
use crate::launch;
use agenthub_core::{Agent, Message, Session, SessionSink, Source};
use agenthub_index::{SessionRecord, Store};
use anyhow::{bail, Context, Result};
use std::{fs, path::PathBuf, process::Command};

pub fn check_delete(store: &Store, key: &str) -> Result<()> {
    let record = store.find(key)?;
    match record.session.agent {
        Agent::Agy => bail!("AGY has no verified native deletion API. Delete this conversation in agy's /resume picker, then sync. AgentHub will not remove only its index or guess writes to AGY's private databases"),
        Agent::Opencode => { opencode_check(store, &record)?; }
        _ => { checked_file(store, &record)?; }
    }
    Ok(())
}

pub fn delete(store: &mut Store, key: &str) -> Result<()> {
    let record = store.find(key)?;
    if record.session.agent == Agent::Opencode {
        let (data_root, exists) = opencode_check(store, &record)?;
        if exists {
            let path = launch::absolute_path()?;
            let executable = launch::resolve("opencode", &path)?;
            agenthub_transfer::run_native_command(
                Command::new(executable)
                    .args(["session", "delete", &record.session.native_id, "--pure"])
                    .env("PATH", path)
                    .env("XDG_DATA_HOME", data_root),
            )?;
            // Some native CLI failures exit zero. Verify actual state, never trust
            // an acknowledgement alone, and never purge the index on a no-op.
            if opencode_exists(&record)?.0 {
                bail!(
                    "OpenCode still contains this session after its delete command; index retained"
                );
            }
        }
    } else {
        if record.session.agent == Agent::Agy {
            return check_delete(store, key);
        }
        if let Some((path, fingerprint)) = checked_file(store, &record)? {
            if Source::new(record.session.agent, path.clone()).fingerprint()? != fingerprint {
                bail!("session changed before deletion; stop the native session and retry");
            }
            fs::remove_file(&path).context("remove native session file; index retained")?;
        }
    }
    store.purge(&record.session.key).context("native session was deleted but index cleanup failed; run delete again to clean the cached transcript")
}

fn checked_file(store: &Store, record: &SessionRecord) -> Result<Option<(PathBuf, String)>> {
    let s = &record.session;
    if !s.source_path.try_exists()? {
        return Ok(None);
    }
    let path = fs::canonicalize(&s.source_path)?;
    let root = fs::canonicalize(store.source_root(&s.key)?)?;
    if !path.starts_with(&root)
        || path != s.source_path
        || path.extension().is_none_or(|e| e != "jsonl")
    {
        bail!("session path changed or is outside its indexed native root; sync before deleting");
    }
    let source = Source::new(s.agent, path.clone());
    let fingerprint = source.fingerprint()?;
    let mut sink = Identity {
        expected: &s.key,
        count: 0,
    };
    agenthub_adapters::adapter(s.agent, root).read(&source, &mut sink)?;
    if sink.count != 1 || fingerprint != source.fingerprint()? {
        bail!("session identity changed, source is shared, or session is active; sync before deleting");
    }
    Ok(Some((path, fingerprint)))
}

struct Identity<'a> {
    expected: &'a str,
    count: usize,
}
impl SessionSink for Identity<'_> {
    fn begin(&mut self, session: &Session) -> Result<()> {
        if session.key != self.expected {
            bail!("native file now belongs to a different session; index retained");
        }
        self.count += 1;
        Ok(())
    }
    fn message(&mut self, _: &Message) -> Result<()> {
        Ok(())
    }
    fn finish(&mut self, _: &Session) -> Result<()> {
        Ok(())
    }
}

fn opencode_check(store: &Store, record: &SessionRecord) -> Result<(PathBuf, bool)> {
    let root = store.source_root(&record.session.key)?;
    let directory = if root.file_name().is_some_and(|n| n == "opencode.db") {
        root.parent().context("OpenCode root has no parent")?
    } else {
        root.as_path()
    };
    if directory.file_name().is_none_or(|s| s != "opencode")
        || record.session.source_path != directory.join("opencode.db")
    {
        bail!("native deletion supports the OpenCode SQLite store at <XDG_DATA_HOME>/opencode/opencode.db; legacy JSON stores must be deleted with the matching native installation");
    }
    if record.session.source_path.try_exists()?
        && fs::canonicalize(&record.session.source_path)? != record.session.source_path
    {
        bail!("OpenCode database path changed through a symlink; sync before deleting");
    }
    let (exists, children) = opencode_exists(record)?;
    if children > 0 {
        bail!("OpenCode session has child sessions; delete them first to avoid recursive native deletion");
    }
    if exists {
        launch::resolve("opencode", &launch::absolute_path()?)?;
    }
    Ok((
        directory
            .parent()
            .context("OpenCode data root has no parent")?
            .into(),
        exists,
    ))
}

fn opencode_exists(record: &SessionRecord) -> Result<(bool, i64)> {
    agenthub_adapters::opencode_deletion_status(
        &record.session.source_path,
        &record.session.native_id,
    )
}

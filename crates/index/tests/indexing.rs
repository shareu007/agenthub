use agenthub_adapters::adapter;
use agenthub_core::Agent;
use agenthub_index::{Filter, Store};
use std::{fs, path::Path};

fn transcript(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let header = serde_json::json!({"type":"session","version":3,"id":"session1","cwd":"/tmp/project","timestamp":"2026-01-01T00:00:00Z"});
    let msg = serde_json::json!({"type":"message","id":"m1","parentId":null,"timestamp":"2026-01-02T00:00:00Z","message":{"role":"user","content":text}});
    fs::write(path,format!("{header}\n{msg}\n")).unwrap();
}

#[test]
fn incremental_update_removes_stale_fts_and_supports_chinese() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let path = root.join("project/a.jsonl");
    transcript(&path,"修复渲染问题 old keyword");
    let a = adapter(Agent::Pi,root);
    let mut store = Store::open(&temp.path().join("index.db")).unwrap();
    assert_eq!(store.sync(a.as_ref(),false).unwrap().indexed,1);
    assert_eq!(store.sync(a.as_ref(),false).unwrap().unchanged,1);
    for keyword in ["渲染问题","渲染","old","keyword"] {
        assert!(!store.search(keyword,&Filter::recent()).unwrap().is_empty(),"{keyword}");
    }
    assert!(store.search("' OR 1=1; --",&Filter::recent()).unwrap().is_empty());
    transcript(&path,"new content changed length");
    assert_eq!(store.sync(a.as_ref(),false).unwrap().indexed,1);
    assert!(store.search("keyword",&Filter::recent()).unwrap().is_empty());
    assert!(!store.search("changed",&Filter::recent()).unwrap().is_empty());
    assert_eq!(store.find("pi:session1").unwrap().message_count,2);
    assert_eq!(store.sync(a.as_ref(),true).unwrap().indexed,1);
}

#[test]
fn corrupt_source_rolls_back_metadata_messages_and_fts_together() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let path = root.join("a.jsonl");
    transcript(&path,"known good text");
    let a = adapter(Agent::Pi,root);
    let mut store = Store::open(&temp.path().join("index.db")).unwrap();
    store.sync(a.as_ref(),false).unwrap();
    fs::write(&path,"{\"type\":\"session\",\"id\":\"session1\"}\ncorrupt\n").unwrap();
    let report = store.sync(a.as_ref(),false).unwrap();
    assert_eq!(report.errors.len(),1);
    let record = store.find("session1").unwrap();
    assert!(record.available);
    assert_eq!(record.message_count,2);
    assert_eq!(record.session.title,"known good text");
    assert!(!store.search("known good",&Filter::recent()).unwrap().is_empty());
    // Failure did not advance the fingerprint: the same failure is retried.
    assert_eq!(store.sync(a.as_ref(),false).unwrap().errors.len(),1);
}

#[test]
fn disappeared_source_keeps_cached_history_and_native_identity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let path = root.join("a.jsonl");
    transcript(&path,"cached text");
    let a = adapter(Agent::Pi,root);
    let mut store = Store::open(&temp.path().join("index.db")).unwrap();
    store.sync(a.as_ref(),false).unwrap();
    fs::remove_file(&path).unwrap();
    assert_eq!(store.sync(a.as_ref(),false).unwrap().missing,1);
    assert!(store.list(&Filter::recent()).unwrap().is_empty());
    assert_eq!(store.list(&Filter {include_missing:true,..Filter::recent()}).unwrap().len(),1);
    let record = store.find("session1").unwrap();
    assert!(!record.available);
    let mut count = 0;
    store.visit_messages(&record.session.key,0,None,|_| {count+=1;Ok(())}).unwrap();
    assert_eq!(count,2);
    transcript(&path,"cached text");
    store.sync(a.as_ref(),false).unwrap();
    assert!(store.find("session1").unwrap().available);
}

#[test]
fn codex_move_to_archived_sessions_does_not_change_key() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("codex");
    fs::create_dir_all(root.join("sessions")).unwrap();
    fs::create_dir_all(root.join("archived_sessions")).unwrap();
    let path = root.join("sessions/rollout-1.jsonl");
    fs::write(&path,"{\"type\":\"session_meta\",\"payload\":{\"id\":\"native1\",\"cwd\":\"/tmp/project\"}}\n").unwrap();
    let a = adapter(Agent::Codex,root.clone());
    let mut store = Store::open(&temp.path().join("index.db")).unwrap();
    store.sync(a.as_ref(),false).unwrap();
    fs::rename(&path,root.join("archived_sessions/rollout-1.jsonl")).unwrap();
    store.sync(a.as_ref(),false).unwrap();
    let records = store.list(&Filter::recent()).unwrap();
    assert_eq!(records.len(),1);
    assert_eq!(records[0].session.key,"codex:native1");
    assert!(records[0].available);
    assert!(records[0].session.source_path.starts_with(root.join("archived_sessions")));
}

#[test]
fn project_agent_and_time_filters_are_composable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    transcript(&root.join("a.jsonl"),"filter me");
    let a = adapter(Agent::Pi,root);
    let mut store = Store::open(&temp.path().join("index.db")).unwrap();
    store.sync(a.as_ref(),false).unwrap();
    let record = store.find("session1").unwrap();
    let updated = record.session.updated_at.unwrap();
    let filter = Filter {agent:Some(Agent::Pi),project:Some("/tmp/project".into()),since:Some(updated),until:Some(updated+1),..Filter::recent()};
    assert_eq!(store.list(&filter).unwrap().len(),1);
    assert!(!store.search("filter",&filter).unwrap().is_empty());
    assert!(store.list(&Filter{until:Some(updated),..filter.clone()}).unwrap().is_empty());
    assert!(store.list(&Filter{agent:Some(Agent::Agy),..filter.clone()}).unwrap().is_empty());
    assert!(store.list(&Filter{project:Some("/tmp/other".into()),..filter}).unwrap().is_empty());
}

#[test]
fn source_database_cannot_accidentally_become_the_index() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("original.db");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("CREATE TABLE agent_data(body TEXT); INSERT INTO agent_data VALUES('original');").unwrap();
    drop(c);
    let before = fs::read(&path).unwrap();
    assert!(Store::open(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(),before);
}

#[cfg(unix)]
#[test]
fn index_is_created_with_private_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("index.db");
    let _store = Store::open(&path).unwrap();
    assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777,0o600);
}

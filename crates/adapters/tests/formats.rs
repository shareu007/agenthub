use agenthub_adapters::{adapter, JsonlAdapter};
use agenthub_core::{Adapter, Agent, Message, Session, SessionSink};
use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::json;
use std::{fs, path::Path};

#[derive(Default)]
struct Capture { sessions: Vec<Session>, messages: Vec<Message> }
impl SessionSink for Capture {
    fn begin(&mut self, _: &Session) -> Result<()> { Ok(()) }
    fn message(&mut self, m: &Message) -> Result<()> { self.messages.push(m.clone()); Ok(()) }
    fn finish(&mut self, s: &Session) -> Result<()> { self.sessions.push(s.clone()); Ok(()) }
}

fn write(path: &Path, value: &str) { fs::create_dir_all(path.parent().unwrap()).unwrap(); fs::write(path,value).unwrap(); }

#[test]
fn jsonl_families_preserve_tools_branches_and_unknown_events() {
    let temp = tempfile::tempdir().unwrap();
    let cases = [
        (Agent::Claude,"project/c1.jsonl",concat!(
            "{\"type\":\"user\",\"sessionId\":\"c1\",\"cwd\":\"/tmp/project\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"修复渲染问题\"}}\n",
            "{\"type\":\"assistant\",\"sessionId\":\"c1\",\"uuid\":\"m1\",\"parentUuid\":\"m0\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"name\":\"read\",\"input\":{\"path\":\"x.rs\"}}]}}\n",
            "{\"type\":\"future_event\",\"custom\":42}\n")),
        (Agent::Codex,"sessions/2026/a.jsonl",concat!(
            "{\"type\":\"session_meta\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"payload\":{\"id\":\"cx1\",\"cwd\":\"/tmp/project\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"fix renderer\"}]}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"output\":\"tool output\"}}\n")),
        (Agent::Pi,"project/pi1.jsonl",concat!(
            "{\"type\":\"session\",\"version\":3,\"id\":\"p1\",\"cwd\":\"/tmp/project\",\"timestamp\":\"2026-01-01T00:00:00Z\"}\n",
            "{\"type\":\"message\",\"id\":\"m2\",\"parentId\":\"m0\",\"message\":{\"role\":\"user\",\"content\":\"first branch\"}}\n",
            "{\"type\":\"message\",\"id\":\"m3\",\"parentId\":\"m0\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"second branch\"}]}}\n",
            "{\"type\":\"session_info\",\"name\":\"Custom title\"}\n")),
    ];
    for (agent,file,data) in cases {
        let root = temp.path().join(agent.as_str());
        write(&root.join(file),data);
        let a = JsonlAdapter::new(agent,root);
        let sources = a.discover().unwrap();
        assert_eq!(sources.sources.len(),1);
        let before = fs::read(&sources.sources[0].path).unwrap();
        let mut capture = Capture::default();
        a.read(&sources.sources[0],&mut capture).unwrap();
        assert_eq!(capture.sessions.len(),1);
        assert!(capture.messages.len() >= 3);
        assert_eq!(fs::read(&sources.sources[0].path).unwrap(),before);
        if agent == Agent::Pi {
            assert_eq!(capture.sessions[0].title,"Custom title");
            assert_eq!(capture.messages[2].parent_id.as_deref(),Some("m0"));
        }
        if agent == Agent::Claude { assert_eq!(capture.messages[2].raw["custom"],42); }
        if agent == Agent::Codex { assert_eq!(capture.messages[2].text,"tool output"); }
    }
}

#[test]
fn opencode_transitional_schema_merges_messages_and_events_with_wal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let c = Connection::open(&path).unwrap();
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
        CREATE TABLE session(id TEXT,title TEXT,directory TEXT,time_created INTEGER,time_updated INTEGER,parent_id TEXT);
        CREATE TABLE message(id TEXT,session_id TEXT,time_created INTEGER,data TEXT);
        CREATE TABLE part(id TEXT,message_id TEXT,time_created INTEGER,data TEXT);
        CREATE TABLE session_message(id TEXT,session_id TEXT,type TEXT,time_created INTEGER,data TEXT);
        INSERT INTO session VALUES('ses_a','Title','/tmp/project',100,200,NULL);
        INSERT INTO message VALUES('msg_1','ses_a',101,'{\"role\":\"user\"}');
        INSERT INTO part VALUES('prt_1','msg_1',101,'{\"type\":\"text\",\"text\":\"legacy text\"}');
        INSERT INTO session_message VALUES('msg_2','ses_a','agent-switched',102,'{\"agent\":\"build\"}');").unwrap();
    // Keep the writer connection open: the new data is in WAL, not the main file.
    let db_before = fs::read(&path).unwrap();
    let wal_path = path.with_file_name("opencode.db-wal");
    let wal_before = fs::read(&wal_path).unwrap();
    let a = adapter(Agent::Opencode,dir.path().into());
    let source = a.discover().unwrap().sources.remove(0);
    let mut capture = Capture::default();
    a.read(&source,&mut capture).unwrap();
    assert_eq!(capture.messages.len(),2);
    assert_eq!(capture.messages[0].text,"legacy text");
    assert_eq!(capture.messages[1].kind,"agent-switched");
    assert_eq!(fs::read(&path).unwrap(),db_before);
    assert_eq!(fs::read(&wal_path).unwrap(),wal_before);
    drop(c);
}

#[test]
fn opencode_v2_orders_message_sequence_and_part_ordinals() {
    let dir = tempfile::tempdir().unwrap();
    let c = Connection::open(dir.path().join("opencode.db")).unwrap();
    c.execute_batch("CREATE TABLE session_v2(id TEXT,title TEXT,directory TEXT,time_created INTEGER,time_updated INTEGER,parent_id TEXT);
        CREATE TABLE session_message(id TEXT,session_id TEXT,type TEXT,seq INTEGER,time_created INTEGER,data TEXT);
        CREATE TABLE session_part(message_id TEXT,ordinal INTEGER,data TEXT);
        INSERT INTO session_v2 VALUES('ses_v2','V2','/tmp/project',100,200,NULL);
        INSERT INTO session_message VALUES('msg_z','ses_v2','user',1,101,'{\"text\":\"question\"}');
        INSERT INTO session_message VALUES('msg_a','ses_v2','assistant',2,101,'{}');
        INSERT INTO session_part VALUES('msg_a',1,'{\"type\":\"text\",\"text\":\"second\"}');
        INSERT INTO session_part VALUES('msg_a',0,'{\"type\":\"text\",\"text\":\"first\"}');").unwrap();
    drop(c);
    let a = adapter(Agent::Opencode,dir.path().into());
    let source = a.discover().unwrap().sources.remove(0);
    assert!(!dir.path().join("opencode.db-shm").exists());
    let mut capture = Capture::default();
    a.read(&source,&mut capture).unwrap();
    assert_eq!(capture.messages[0].role,"user");
    assert_eq!(capture.messages[1].text,"first\nsecond");
    assert!(!dir.path().join("opencode.db-shm").exists());
}

#[test]
fn opencode_legacy_json_tree_and_dependency_changes() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    write(&storage.join("session/project/ses_old.json"),r#"{"id":"ses_old","directory":"/tmp/old","time":{"created":100,"updated":200}}"#);
    write(&storage.join("message/ses_old/msg_1.json"),r#"{"id":"msg_1","role":"user","time":{"created":101}}"#);
    write(&storage.join("part/msg_1/prt_1.json"),r#"{"type":"text","text":"legacy question"}"#);
    let a = adapter(Agent::Opencode,dir.path().into());
    let source = a.discover().unwrap().sources.remove(0);
    let before = source.fingerprint().unwrap();
    let mut capture = Capture::default();
    a.read(&source,&mut capture).unwrap();
    assert_eq!(capture.sessions[0].title,"legacy question");
    assert_eq!(capture.messages[0].text,"legacy question");
    write(&storage.join("part/msg_1/prt_2.json"),r#"{"type":"text","text":"new part"}"#);
    let newer = a.discover().unwrap().sources.remove(0);
    assert_ne!(before,newer.fingerprint().unwrap());
}

fn varint(mut n: u64) -> Vec<u8> {
    let mut bytes = vec![];
    while n >= 128 { bytes.push((n as u8 & 127)|128); n >>= 7; }
    bytes.push(n as u8); bytes
}
fn field(n: u64, bytes: &[u8]) -> Vec<u8> {
    let mut out = varint(n*8+2); out.extend(varint(bytes.len() as u64)); out.extend(bytes); out
}

#[test]
fn agy_native_sqlite_protobuf_and_unknown_formats_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("conversations")).unwrap();
    let c = Connection::open(dir.path().join("conversation_summaries.db")).unwrap();
    c.execute_batch("CREATE TABLE conversation_summaries(conversation_id TEXT,title TEXT,workspace_uris TEXT,last_modified_time TEXT,parent_conversation_id TEXT);").unwrap();
    c.execute("INSERT INTO conversation_summaries VALUES(?1,?2,?3,?4,NULL)",params!["agy1","Native AGY",r#"["file:///tmp/my%20project"]"#,"2026-01-01T00:00:00Z"]).unwrap();
    drop(c);
    let c = Connection::open(dir.path().join("conversations/agy1.db")).unwrap();
    c.execute_batch("CREATE TABLE steps(idx INTEGER,step_type INTEGER,step_format INTEGER,step_payload BLOB,metadata BLOB,error_details BLOB,permissions BLOB,task_details BLOB,render_info BLOB,status INTEGER,has_subtrajectory INTEGER);").unwrap();
    let user = field(19,&field(2,"修复渲染问题".as_bytes()));
    let assistant = field(20,&field(8,b"fixed"));
    for (i,kind,fmt,bytes) in [(0,14,0,user),(1,15,0,assistant),(2,99,7,vec![1,2,3])] {
        c.execute("INSERT INTO steps VALUES(?1,?2,?3,?4,NULL,NULL,NULL,NULL,NULL,3,0)",params![i,kind,fmt,bytes]).unwrap();
    }
    drop(c);
    let a = adapter(Agent::Agy,dir.path().into());
    let source = a.discover().unwrap().sources.remove(0);
    let mut capture = Capture::default();
    a.read(&source,&mut capture).unwrap();
    assert_eq!(capture.sessions[0].project_path.as_deref(),Some(Path::new("/tmp/my project")));
    assert_eq!(capture.messages[0].text,"修复渲染问题");
    assert_eq!(capture.messages[1].text,"fixed");
    assert!(capture.messages[2].text.contains("unsupported"));
    assert!(capture.sessions[0].warnings.iter().any(|s|s.contains("unsupported")));
    assert_eq!(capture.messages[2].raw["step_payload_base64"],json!("AQID"));
}

#[test]
fn active_partial_tail_is_deferred_but_internal_corruption_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.jsonl");
    let header = "{\"type\":\"session\",\"id\":\"p\",\"cwd\":\"/tmp\"}\n";
    write(&path,&format!("{header}{{\"type\":"));
    let a = JsonlAdapter::new(Agent::Pi,dir.path().into());
    let source = a.discover().unwrap().sources.remove(0);
    let mut capture = Capture::default();
    a.read(&source,&mut capture).unwrap();
    assert!(capture.sessions[0].warnings[0].contains("incomplete"));
    write(&path,&format!("{header}broken\n{{}}\n"));
    assert!(a.read(&source,&mut Capture::default()).is_err());
}

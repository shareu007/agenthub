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

#[test]
fn pi_1_5_2_native_writer_fixture_preserves_identity_and_dialogue() {
    let dir = tempfile::tempdir().unwrap();
    let data = include_str!("../../../tests/fixtures/pi/1.5.2.jsonl");
    write(&dir.path().join("native.jsonl"), data);
    let a = adapter(Agent::Pi, dir.path().into());
    let source = a.discover().unwrap().sources.remove(0);
    let mut capture = Capture::default();
    a.read(&source, &mut capture).unwrap();
    assert_eq!(capture.sessions[0].native_id, "01a1cd90-1c83-7753-bc2e-2d0c9bac8cf4");
    assert_eq!(capture.messages.len(), 4);
    assert_eq!(capture.messages[1].role, "user");
    assert!(capture.messages[1].text.contains("AH_FIXTURE_20261112"));
    assert_eq!(capture.messages[2].role, "assistant");
    assert_eq!(capture.messages[2].parent_id, capture.messages[1].native_id);
    assert_eq!(capture.messages[3].kind, "thinking_level_change");
    assert_eq!(fs::read_to_string(&source.path).unwrap(), data);
}

#[test]
fn opencode_1_19_5_native_api_fixture_keeps_stored_user_message() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let c = Connection::open(&path).unwrap();
    c.execute_batch(include_str!("../../../tests/fixtures/opencode/1.19.5.sql")).unwrap();
    drop(c);
    let before = fs::read(&path).unwrap();
    let a = adapter(Agent::Opencode, dir.path().into());
    let source = a.discover().unwrap().sources.remove(0);
    let mut capture = Capture::default();
    a.read(&source, &mut capture).unwrap();
    assert_eq!(capture.sessions[0].native_id, "ses_e326ec748ffePgLiZVEam9royd");
    assert_eq!(capture.messages.len(), 1);
    assert_eq!(capture.messages[0].role, "user");
    assert!(capture.messages[0].text.contains("AH_FIXTURE_20261112"));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(!dir.path().join("opencode.db-shm").exists());
}

#[test]
fn agy_1_5_6_native_cli_fixture_preserves_steps_and_tool_call() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("conversations")).unwrap();
    let (summary_sql, conv_sql) = include_str!("../../../tests/fixtures/agy/1.5.6.sql")
        .split_once("-- CONVERSATION_DB: 93748d3b-ae91-447e-a184-c16dc21090c9.db\n").unwrap();
    let summary_path = dir.path().join("conversation_summaries.db");
    let conv_path = dir.path().join("conversations/93748d3b-ae91-447e-a184-c16dc21090c9.db");
    Connection::open(&summary_path).unwrap().execute_batch(summary_sql).unwrap();
    Connection::open(&conv_path).unwrap().execute_batch(conv_sql).unwrap();
    let summary_before = fs::read(&summary_path).unwrap();
    let conv_before = fs::read(&conv_path).unwrap();
    let a = adapter(Agent::Agy, dir.path().into());
    let source = a.discover().unwrap().sources.remove(0);
    let mut capture = Capture::default();
    a.read(&source, &mut capture).unwrap();
    assert_eq!(capture.sessions[0].native_id, "93748d3b-ae91-447e-a184-c16dc21090c9");
    assert_eq!(capture.sessions[0].project_path.as_deref(), Some(Path::new("/tmp/agenthub-fixture-project")));
    assert!(!capture.sessions[0].warnings.iter().any(|w| w.contains("unsupported")));
    assert_eq!(capture.messages.len(), 4);
    assert_eq!(capture.messages[0].role, "user");
    assert!(capture.messages[0].text.contains("AH_FIXTURE_20261112"));
    assert_eq!(capture.messages[1].role, "assistant");
    assert!(capture.messages[1].text.contains("Inspecting fixture file") && capture.messages[1].text.contains("read_file"));
    assert_eq!(capture.messages[2].role, "tool");
    assert_eq!(capture.messages[2].kind, "agy_step_132");
    assert!(capture.messages[2].text.contains("AgentHub AGY fixture file"));
    assert_eq!(capture.messages[3].role, "assistant");
    assert!(capture.messages[3].text.contains("AH_FIXTURE_20261112"));
    assert_eq!(fs::read(&summary_path).unwrap(), summary_before);
    assert_eq!(fs::read(&conv_path).unwrap(), conv_before);
    assert!(!dir.path().join("conversation_summaries.db-shm").exists());
    assert!(!dir.path().join("conversations/93748d3b-ae91-447e-a184-c16dc21090c9.db-shm").exists());
}

#[test]
fn dsh_0_2_2_rc_1_native_cli_fixture_preserves_zstd_frames_tools_and_title() {
    let dir = tempfile::tempdir().unwrap();
    let sess_dir = dir.path().join("--tmp-agenthub-fixture-project--/session-fixture");
    fs::create_dir_all(&sess_dir).unwrap();
    let zstd_bytes = include_bytes!("../../../tests/fixtures/dsh/0.2.2-rc.1.jsonl.zstd");
    let v4_path = sess_dir.join("session.v4.jsonl.zstd");
    let v3_path = sess_dir.join("session.v3.jsonl");
    let lock_path = sess_dir.join("session.lock");
    fs::write(&v4_path, zstd_bytes).unwrap();
    fs::write(&v3_path, "{\"type\":\"session\",\"version\":3,\"id\":\"old\",\"createdAt\":100}\n").unwrap();
    fs::write(&lock_path, "").unwrap();

    let a = adapter(Agent::Dsh, dir.path().into());
    let sources = a.discover().unwrap().sources;
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].path, v4_path);
    assert_eq!(sources[0].dependencies, vec![v3_path.clone(), v4_path.clone()]);

    let mut capture = Capture::default();
    a.read(&sources[0], &mut capture).unwrap();
    assert_eq!(capture.sessions.len(), 1);
    let s = &capture.sessions[0];
    assert!(s.native_id.starts_with("session-"));
    assert_eq!(s.project_path.as_deref(), Some(Path::new("/tmp/agenthub-fixture-project")));
    assert_eq!(s.title, "Fixture session AH_FIXTURE_20261112");
    assert!(s.warnings.is_empty());

    assert!(capture.messages.iter().any(|m| m.role == "user" && m.text.contains("Inspect AH_FIXTURE_20261112")));
    assert!(capture.messages.iter().any(|m| m.kind == "user/message" && m.role == "system" && m.text.contains("Current runtime context")));
    assert!(capture.messages.iter().any(|m| m.role == "tool_call" && m.native_id.as_deref() == Some("call_fixture_1") && m.text.contains("glob")));
    assert!(capture.messages.iter().any(|m| m.role == "tool" && m.parent_id.as_deref() == Some("16") && m.text.contains("README.md")));
    assert!(capture.messages.iter().any(|m| m.role == "assistant" && m.text.contains("AH_FIXTURE_20261112 verified on DeepSeek Harness.")));
    assert_eq!(fs::read(&v4_path).unwrap(), zstd_bytes);

    // Torn final Zstandard frame preserves earlier committed frames with a warning.
    fs::write(&v4_path, &zstd_bytes[..zstd_bytes.len() - 5]).unwrap();
    let mut torn_capture = Capture::default();
    a.read(&sources[0], &mut torn_capture).unwrap();
    assert!(torn_capture.sessions[0].warnings.iter().any(|w| w.contains("incomplete final Zstandard frame")));
    assert!(torn_capture.messages.iter().any(|m| m.role == "user" && m.text.contains("Inspect AH_FIXTURE_20261112")));
}


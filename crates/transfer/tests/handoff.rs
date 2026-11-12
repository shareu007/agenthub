use agenthub_adapters::adapter;
use agenthub_core::{Agent, Message, Session, SessionSink};
use agenthub_transfer::{Collector, Plan};
use anyhow::Result;
use serde_json::json;
use std::path::PathBuf;

fn message(
    id: Option<&str>,
    parent: Option<&str>,
    role: &str,
    kind: &str,
    raw: serde_json::Value,
) -> Message {
    Message {
        ordinal: 0,
        native_id: id.map(str::to_owned),
        parent_id: parent.map(str::to_owned),
        role: role.into(),
        kind: kind.into(),
        timestamp: None,
        text: "must not copy normalized reasoning/tool text".into(),
        raw,
    }
}
fn codex(text: &str, role: &str) -> Message {
    message(
        None,
        None,
        role,
        "response_item",
        json!({"type":"response_item","payload":{"type":"message","role":role,"content":[{"type":if role=="user" {"input_text"} else {"output_text"},"text":text}]}}),
    )
}
fn conversation() -> Collector {
    let mut c = Collector::new(Agent::Codex);
    c.push(codex("transfer-marker 渲染问题", "user")).unwrap();
    c.push(message(None,None,"assistant","response_item",json!({"payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"private reasoning"}]}}))).unwrap();
    c.push(message(
        None,
        None,
        "tool",
        "response_item",
        json!({"payload":{"type":"function_call_output","output":"tool secret"}}),
    ))
    .unwrap();
    c.push(codex("answer", "assistant")).unwrap();
    c
}

#[derive(Default)]
struct Capture(Vec<Message>);
impl SessionSink for Capture {
    fn begin(&mut self, _: &Session) -> Result<()> {
        Ok(())
    }
    fn finish(&mut self, _: &Session) -> Result<()> {
        Ok(())
    }
    fn message(&mut self, m: &Message) -> Result<()> {
        self.0.push(m.clone());
        Ok(())
    }
}

#[test]
fn file_targets_roundtrip_only_dialogue_into_new_private_files() {
    let dir = tempfile::tempdir().unwrap();
    let source_file = dir.path().join("source.jsonl");
    std::fs::write(&source_file, "original must remain unchanged").unwrap();
    let mut source = Session::new(Agent::Codex, "source".into(), source_file.clone()).unwrap();
    source.project_path = Some(dir.path().into());
    source.title = "Transfer test".into();
    for target in [Agent::Claude, Agent::Codex, Agent::Pi] {
        let root = if target == Agent::Claude {
            dir.path().join("claude/projects")
        } else {
            dir.path().join(target.as_str())
        };
        let plan = Plan::new(&source, target, root.clone(), None, conversation(), None).unwrap();
        assert_eq!(plan.turn_count, 2);
        assert!(!root.exists()); // Preview builds no destination directories/files.
        let first = plan.write().unwrap();
        let second = plan.write().unwrap();
        assert_ne!(first.key, second.key);
        assert_ne!(first.path, second.path);
        let a = adapter(target, root);
        let discovered = a.discover().unwrap();
        assert_eq!(discovered.sources.len(), 2);
        let mut capture = Capture::default();
        a.read(&discovered.sources[0], &mut capture).unwrap();
        let dialogue = capture
            .0
            .iter()
            .filter(|m| matches!(m.role.as_str(), "user" | "assistant"))
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(dialogue, ["transfer-marker 渲染问题", "answer"]);
        assert!(!std::fs::read_to_string(&first.path)
            .unwrap()
            .contains("private reasoning"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&first.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    assert_eq!(
        std::fs::read_to_string(source_file).unwrap(),
        "original must remain unchanged"
    );
}

#[test]
fn pi_active_leaf_excludes_abandoned_branches_and_scope_is_explicit() {
    let mut c = Collector::new(Agent::Pi);
    c.push(message(
        None,
        None,
        "system",
        "session",
        json!({"type":"session","version":3}),
    ))
    .unwrap();
    for (id, parent, role, value) in [
        ("u", None, "user", "question"),
        ("old", Some("u"), "assistant", "abandoned"),
        ("current", Some("u"), "assistant", "active answer"),
    ] {
        c.push(message(Some(id),parent,role,"message",json!({"message":{"content":[{"type":"text","text":value},{"type":"thinking","thinking":"reasoning excluded"}]}}))).unwrap();
    }
    let (turns, dropped) = c.finish(Some(1)).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].text, "active answer");
    assert!(dropped >= 2);
}

#[test]
fn broken_branch_links_and_unsupported_targets_fail_before_writing() {
    let mut c = Collector::new(Agent::Pi);
    c.push(message(
        None,
        None,
        "system",
        "session",
        json!({"type":"session","version":3}),
    ))
    .unwrap();
    c.push(message(
        Some("u"),
        Some("missing"),
        "user",
        "message",
        json!({"message":{"content":"question"}}),
    ))
    .unwrap();
    assert!(c.finish(None).is_err());
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new(Agent::Pi, "s".into(), PathBuf::from("source")).unwrap();
    s.project_path = Some(dir.path().into());
    let root = dir.path().join("agy");
    assert!(Plan::new(&s, Agent::Agy, root.clone(), None, conversation(), None).is_err());
    assert!(!root.exists());
}

#[test]
fn agy_native_fixture_transfers_reply_without_thinking_or_tools() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("conversations")).unwrap();
    let (summary, conversation) = include_str!("../../../tests/fixtures/agy/1.5.6.sql")
        .split_once("-- CONVERSATION_DB: 93748d3b-ae91-447e-a184-c16dc21090c9.db\n")
        .unwrap();
    rusqlite::Connection::open(dir.path().join("conversation_summaries.db"))
        .unwrap()
        .execute_batch(summary)
        .unwrap();
    rusqlite::Connection::open(
        dir.path()
            .join("conversations/93748d3b-ae91-447e-a184-c16dc21090c9.db"),
    )
    .unwrap()
    .execute_batch(conversation)
    .unwrap();
    let a = adapter(Agent::Agy, dir.path().into());
    let native = a.discover().unwrap().sources.remove(0);
    let mut captured = Capture::default();
    a.read(&native, &mut captured).unwrap();
    assert!(captured.0[1].text.contains("read_file")); // Still visible in show/search.
    assert!(agenthub_adapters::agy_assistant_reply(&captured.0[1].raw)
        .unwrap()
        .is_none());
    assert!(captured.0[3].raw.get("assistant_reply").is_none()); // No decoded cache added.
    let mut collector = Collector::new(Agent::Agy);
    for m in captured.0 {
        collector.push(m).unwrap();
    }
    let source = Session::new(Agent::Agy, "fixture".into(), native.path).unwrap();
    let root = dir.path().join("pi");
    let plan = Plan::new(
        &source,
        Agent::Pi,
        root.clone(),
        Some(dir.path().into()),
        collector,
        None,
    )
    .unwrap();
    assert_eq!(plan.turn_count, 2);
    let receipt = plan.write().unwrap();
    let text = std::fs::read_to_string(receipt.path).unwrap();
    assert!(!text.contains("Inspecting fixture file"));
    assert!(!text.contains("tool secret"));
    assert!(text.contains("Offline AGY fixture response"));
    let mut stale = Collector::new(Agent::Agy);
    assert!(stale
        .push(message(
            None,
            None,
            "assistant",
            "agy_step_15",
            json!({"encoding":"agy-protobuf"})
        ))
        .is_err());
}

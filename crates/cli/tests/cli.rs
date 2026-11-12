use std::{fs, process::{Command, Output}};

fn run(config: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agenthub")).arg("--config").arg(config).args(args).output().unwrap()
}

#[test]
fn isolated_cli_sync_search_show_and_resume_plan() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let sources = home.join("pi");
    fs::create_dir(&sources).unwrap();
    fs::write(sources.join("sample.jsonl"),concat!(
        "{\"type\":\"session\",\"version\":3,\"id\":\"example\",\"cwd\":\"/tmp\"}\n",
        "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"unicode 渲染问题\"}}\n"
    )).unwrap();
    let config = home.join("config.toml");
    let mut config_text = format!("database = {:?}\n[sources]\n",home.join("index.db").to_str().unwrap());
    for agent in ["claude","codex","opencode","pi","agy"] {
        let path = if agent == "pi" {sources.clone()} else {home.join(agent)};
        config_text.push_str(&format!("{agent} = {:?}\n",path.to_str().unwrap()));
    }
    fs::write(&config,config_text).unwrap();
    let result = run(&config,&["sync","--include-unavailable","--json"]);
    assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["pi"]["indexed"],1);
    let result = run(&config,&["search","渲染","--json"]);
    assert!(result.status.success());
    assert!(!serde_json::from_slice::<Vec<serde_json::Value>>(&result.stdout).unwrap().is_empty());
    let result = run(&config,&["show","pi:example","--json"]);
    assert!(result.status.success());
    for line in String::from_utf8(result.stdout).unwrap().lines() { let _:serde_json::Value=serde_json::from_str(line).unwrap(); }
    let result = run(&config,&["resume","pi:example","--json"]);
    assert!(result.status.success());
    let plan:serde_json::Value=serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(plan["program"],"pi");
    assert_eq!(plan["args"][0],"--session");
    // Doctor never creates a database, even in a new installation.
    fs::remove_file(home.join("index.db")).unwrap();
    assert!(run(&config,&["doctor"]).status.success());
    assert!(!home.join("index.db").exists());
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("pi")).unwrap();
    fs::write(root.join("pi/session.jsonl"), concat!(
        "{\"type\":\"session\",\"id\":\"ui\",\"cwd\":\"/tmp\",\"timestamp\":\"2026-01-01T00:00:00Z\"}\n",
        "{\"type\":\"message\",\"timestamp\":\"2026-01-02T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"修复渲染问题\\u001b[31m\"}}\n"
    )).unwrap();
    let config = root.join("config.toml");
    let mut text = format!("database = {:?}\n[sources]\n", root.join("index.db"));
    for agent in ["claude", "codex", "opencode", "pi", "agy"] {
        text.push_str(&format!("{agent} = {:?}\n", root.join(agent)));
    }
    fs::write(&config, text).unwrap();
    (dir, config)
}

#[test]
fn human_output_is_readable_and_json_stays_machine_readable() {
    let (_dir, config) = fixture();
    let empty = run(&config, &["list"]);
    assert!(String::from_utf8(empty.stdout).unwrap().contains("No sessions match"));
    assert!(run(&config, &["sync","--include-unavailable"]).status.success());
    let listing = run(&config, &["list", "--limit", "1"]);
    let listing = String::from_utf8(listing.stdout).unwrap();
    assert!(listing.contains("pi:ui") && listing.contains("2026-01-02 00:00:00 UTC"));
    assert!(listing.contains("--offset 1 --limit 1"));
    assert!(!listing.contains('\u{1b}'));
    let colored = run(&config, &["list", "--color", "always"]);
    assert!(colored.stdout.contains(&0x1b));
    let json = run(&config, &["list", "--color", "always", "--json"]);
    assert!(!json.stdout.contains(&0x1b));
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(rows[0]["key"], "pi:ui");
    let show = run(&config, &["show", "pi:ui", "--color", "never"]);
    let show = String::from_utf8(show.stdout).unwrap();
    assert!(show.contains("USER · #1") && show.contains("2 records"));
    assert!(!show.contains('\u{1b}'));
    let search = run(&config, &["search", "no-match"]);
    assert!(String::from_utf8(search.stdout).unwrap().contains("No matches"));
    let raw = run(&config, &["show", "pi:ui", "--raw"]);
    for line in String::from_utf8(raw.stdout).unwrap().lines() {
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(record.get("type").is_some());
    }
}

#[test]
fn doctor_reports_discovery_failures_in_text_and_json_with_failure_exit() {
    let (dir, config) = fixture();
    let path = dir.path().join("opencode/storage/session/project");
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("broken.json"), "not JSON").unwrap();
    let result = run(&config, &["doctor"]);
    assert!(!result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("ERROR") && text.contains("Error:"));
    let result = run(&config, &["doctor", "--json"]);
    assert!(!result.status.success());
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&result.stdout).unwrap();
    assert!(rows.iter().any(|r| r["agent"] == "opencode" && !r["error"].is_null()));
    assert!(!dir.path().join("index.db").exists());
}

#[test]
fn help_gives_a_workflow_without_accessing_configuration() {
    let output = Command::new(env!("CARGO_BIN_EXE_agenthub"))
        .args(["--config", "/missing/agenthub/config.toml", "--help"]).output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("QUICK START") && help.contains("--json") && help.contains("--exec"));
}

#[test]
fn both_names_default_to_list_and_help_stays_explicit() {
    let (_dir, config) = fixture();
    assert!(run(&config, &["sync","--include-unavailable"]).status.success());
    let explicit = run(&config, &["list", "--json"]);
    for binary in [env!("CARGO_BIN_EXE_agenthub"), env!("CARGO_BIN_EXE_ah")] {
        let result = Command::new(binary).arg("--config").arg(&config).arg("--json").output().unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, explicit.stdout);
        let help = Command::new(binary).args(["help"]).output().unwrap();
        assert!(help.status.success());
        assert!(String::from_utf8(help.stdout).unwrap().contains("QUICK START"));
    }
    let text = run(&config, &[]);
    assert!(String::from_utf8(text.stdout).unwrap().contains("AgentHub · Sessions"));
    let forced = run(&config, &["list", "--interactive"]);
    assert!(!forced.status.success());
    assert!(String::from_utf8(forced.stderr).unwrap().contains("requires terminal"));
    let json = run(&config, &["list", "--interactive", "--json"]);
    assert!(!json.status.success());
}

#[test]
fn grouping_and_sorting_happen_before_list_pagination() {
    let (dir, config) = fixture();
    for (id, project, timestamp) in [("old", "/z", "2025-01-01T00:00:00Z"), ("new", "/a", "2027-01-01T00:00:00Z")] {
        fs::write(dir.path().join(format!("pi/{id}.jsonl")), format!("{{\"type\":\"session\",\"id\":\"{id}\",\"cwd\":\"{project}\",\"timestamp\":\"{timestamp}\"}}\n")).unwrap();
    }
    assert!(run(&config, &["sync","--include-unavailable"]).status.success());
    let result = run(&config, &["list", "--group-by", "none", "--sort", "time", "--limit", "1", "--offset", "1", "--json"]);
    let records: Vec<serde_json::Value> = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(records[0]["key"], "pi:ui");
    let result = run(&config, &["list", "--group-by", "none", "--sort", "time", "--reverse", "--limit", "1", "--json"]);
    let records: Vec<serde_json::Value> = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(records[0]["key"], "pi:old");
    let result = run(&config, &["list", "--group-by", "project", "--limit", "1", "--json"]);
    let records: Vec<serde_json::Value> = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(records[0]["key"], "pi:new");
    let result = run(&config, &["list", "--group-by", "project", "--no-interactive"]);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.find("── /a ──").unwrap() < text.find("── /tmp ──").unwrap());
    assert!(text.find("── /tmp ──").unwrap() < text.find("── /z ──").unwrap());
    assert!(!run(&config, &["list", "--sort", "arbitrary-sql"]).status.success());
}

#[test]
fn transfer_preview_is_read_only_and_written_session_is_immediately_indexed() {
    let (dir, config) = fixture();
    let source=dir.path().join("pi/session.jsonl");
    let original=fs::read(&source).unwrap();
    assert!(run(&config,&["sync","--include-unavailable"]).status.success());
    let preview=run(&config,&["transfer","pi:ui","--to","codex","--dry-run","--json"]);
    assert!(preview.status.success(),"{}",String::from_utf8_lossy(&preview.stderr));
    let preview:serde_json::Value=serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(preview["dry_run"],true);
    assert_eq!(preview["preview"]["turn_count"],1);
    assert!(!dir.path().join("codex").exists());
    let result=run(&config,&["transfer","pi:ui","--to","codex","--json"]);
    assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));
    let receipt:serde_json::Value=serde_json::from_slice(&result.stdout).unwrap();
    let key=receipt["key"].as_str().unwrap();
    assert!(key.starts_with("codex:") && std::path::Path::new(receipt["path"].as_str().unwrap()).exists());
    let indexed=run(&config,&["list","--agent","codex","--json"]);
    let rows:Vec<serde_json::Value>=serde_json::from_slice(&indexed.stdout).unwrap();
    assert_eq!(rows[0]["key"],key);
    assert_eq!(fs::read(source).unwrap(),original);
    let unsupported=run(&config,&["transfer","pi:ui","--to","agy","--dry-run"]);
    assert!(!unsupported.status.success());
    assert!(String::from_utf8(unsupported.stderr).unwrap().contains("no verified native import"));
    assert!(!dir.path().join("agy").exists());
    assert!(!run(&config,&["transfer","pi:ui","--to","codex","--last","0"]).status.success());
}

#[cfg(unix)]
#[test]
fn opencode_import_stream_uses_native_keys_and_requires_positive_acknowledgement() {
    use std::os::unix::fs::PermissionsExt;
    let (dir,config)=fixture();
    assert!(run(&config,&["sync","--include-unavailable"]).status.success());
    let bin=dir.path().join("bin");fs::create_dir(&bin).unwrap();
    let program=bin.join("opencode");
    fs::write(&program, r#"#!/usr/bin/env python3
import sys,os,json,stat
if sys.argv[1]=='--version':
 print('1.19.5')
else:
 assert stat.S_IMODE(os.stat(sys.argv[2]).st_mode)==0o600
 with open(sys.argv[2]) as f: payload=json.load(f)
 with open(os.environ['AH_TEST_PAYLOAD'],'w') as f: json.dump(payload,f)
 if os.environ['AH_TEST_ACK']=='yes': print('Imported session: '+payload['info']['id'])
 else: print('Import failed')
"#).unwrap();
    fs::set_permissions(&program,fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths=vec![bin];paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    let path=std::env::join_paths(paths).unwrap();
    let payload=dir.path().join("payload.json");
    let invoke=|ack| Command::new(env!("CARGO_BIN_EXE_agenthub")).arg("--config").arg(&config)
        .args(["transfer","pi:ui","--to","opencode","--json"])
        .env("PATH",&path).env("AH_TEST_PAYLOAD",&payload).env("AH_TEST_ACK",ack).output().unwrap();
    assert!(!invoke("no").status.success());
    let result=invoke("yes");assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));
    let body:serde_json::Value=serde_json::from_slice(&fs::read(payload).unwrap()).unwrap();
    let part=&body["messages"][0]["parts"][0];
    assert_eq!(part["sessionID"],body["info"]["id"]);
    assert_eq!(part["messageID"],body["messages"][0]["info"]["id"]);
    assert_eq!(part["type"],"text");assert!(part["text"].as_str().unwrap().contains("渲染"));
    let receipt:serde_json::Value=serde_json::from_slice(&result.stdout).unwrap();
    assert!(receipt["warnings"].as_array().unwrap().iter().any(|w|w.as_str().unwrap().contains("not visible")));
}

#[cfg(unix)]
#[test]
fn index_cannot_alias_into_source_through_multiple_missing_directories() {
    let (dir,config)=fixture();
    let alias=dir.path().join("source-alias");
    std::os::unix::fs::symlink(dir.path().join("pi"),&alias).unwrap();
    let result=Command::new(env!("CARGO_BIN_EXE_agenthub")).arg("--config").arg(config)
        .arg("--db").arg(alias.join("missing/deep/index.db")).arg("list").output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8(result.stderr).unwrap().contains("index path overlaps"));
    assert!(!dir.path().join("pi/missing").exists());
    let dangling=dir.path().join("dangling-index");
    std::os::unix::fs::symlink(dir.path().join("pi/new-index.db"),&dangling).unwrap();
    let result=Command::new(env!("CARGO_BIN_EXE_agenthub")).arg("--config").arg(dir.path().join("config.toml"))
        .arg("--db").arg(dangling).arg("list").output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8(result.stderr).unwrap().contains("dangling symlink"));
    assert!(!dir.path().join("pi/new-index.db").exists());
}

#[cfg(unix)]
fn executable(path: &std::path::Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn default_sync_only_indexes_installed_agents_without_executing_them() {
    let (dir, config) = fixture();
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let call = |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_agenthub")).arg("--config").arg(&config).args(args).env("PATH", &bin).output().unwrap();
    let report: serde_json::Value = serde_json::from_slice(&call(&["sync", "--json"]).stdout).unwrap();
    assert!(report["pi"]["skipped"].as_str().unwrap().contains("native"));
    assert_eq!(report["pi"]["indexed"], 0);
    assert!(!call(&["sync", "--agent", "pi"]).status.success());
    assert_eq!(serde_json::from_slice::<Vec<serde_json::Value>>(&call(&["list", "--json"]).stdout).unwrap().len(), 0);
    let marker = dir.path().join("executed");
    executable(&bin.join("pi"), &format!("#!/bin/sh\nprintf invoked > '{}'\n", marker.display()));
    let result = call(&["sync", "--json"]);
    assert!(result.status.success());
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["pi"]["indexed"], 1);
    assert!(!marker.exists(), "sync must not start a native client");
    fs::remove_file(bin.join("pi")).unwrap();
    assert!(call(&["sync"]).status.success());
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&call(&["list", "--json"]).stdout).unwrap();
    assert_eq!(rows.len(), 1, "skipping a missing client preserves previously indexed history");
    assert!(rows[0]["size_bytes"].as_i64().unwrap() > 0);
    assert!(call(&["sync", "--include-unavailable", "--agent", "pi"]).status.success());
}

#[cfg(unix)]
#[test]
fn resume_freezes_relative_path_and_distinguishes_missing_cwd_and_interpreter() {
    let (dir, config) = fixture();
    let root = dir.path();
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    let native = root.join("opencode/storage/session/p");
    fs::create_dir_all(&native).unwrap();
    fs::write(native.join("ses_launch.json"), serde_json::json!({"id":"ses_launch","title":"launch","directory":project,"time":{"created":1,"updated":2}}).to_string()).unwrap();
    assert!(run(&config, &["sync", "--include-unavailable"]).status.success());
    let bin = root.join("tools/bin");
    fs::create_dir_all(&bin).unwrap();
    executable(&bin.join("opencode"), "#!/bin/sh\nexec fake-runtime \"$@\"\n");
    executable(&bin.join("fake-runtime"), "#!/bin/sh\npwd\nprintf '%s\\n' \"$@\"\nexit 17\n");
    let call = |cwd: Option<&str>| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_agenthub"));
        cmd.arg("--config").arg(&config).args(["resume", "opencode:ses_launch", "--exec"]).env("PATH", "tools/bin").current_dir(root);
        if let Some(cwd) = cwd { cmd.args(["--cwd", cwd]); }
        cmd.output().unwrap()
    };
    let result = call(None);
    assert_eq!(result.status.code(), Some(17), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(String::from_utf8_lossy(&result.stdout).contains("--session\nses_launch"));
    fs::remove_dir(&project).unwrap();
    assert!(String::from_utf8_lossy(&call(None).stderr).contains("working directory does not exist"));
    assert_eq!(call(Some(root.to_str().unwrap())).status.code(), Some(17));
    executable(&bin.join("opencode"), "#!/missing/agenthub-interpreter\n");
    assert!(String::from_utf8_lossy(&call(Some(root.to_str().unwrap())).stderr).contains("interpreter"));
    fs::remove_file(bin.join("opencode")).unwrap();
    assert!(String::from_utf8_lossy(&call(Some(root.to_str().unwrap())).stderr).contains("native executable 'opencode'"));
}

#[test]
fn rename_persists_without_native_writes_and_delete_requires_explicit_confirmation() {
    let (dir, config) = fixture();
    let path = dir.path().join("pi/session.jsonl");
    let before = fs::read(&path).unwrap();
    assert!(run(&config, &["sync", "--include-unavailable"]).status.success());
    assert!(run(&config, &["rename", "pi:ui", "My custom title"]).status.success());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(run(&config, &["sync", "--include-unavailable", "--force"]).status.success());
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&run(&config, &["list", "--json", "--sort", "size"]).stdout).unwrap();
    assert_eq!(rows[0]["title"], "My custom title");
    assert!(!serde_json::from_slice::<Vec<serde_json::Value>>(&run(&config, &["search", "custom", "--json"]).stdout).unwrap().is_empty());
    assert!(run(&config, &["rename", "pi:ui", "--reset"]).status.success());
    let preview = run(&config, &["delete", "pi:ui"]);
    assert!(preview.status.success() && String::from_utf8_lossy(&preview.stdout).contains("PERMANENT DELETE"));
    assert!(path.exists());
    assert!(run(&config, &["show", "pi:ui"]).status.success());
    let deleted = run(&config, &["delete", "pi:ui", "--yes", "--json"]);
    assert!(deleted.status.success(), "{}", String::from_utf8_lossy(&deleted.stderr));
    assert!(!path.exists());
    assert!(!run(&config, &["show", "pi:ui"]).status.success());
    assert!(run(&config, &["sync", "--include-unavailable"]).status.success());
    assert!(serde_json::from_slice::<Vec<serde_json::Value>>(&run(&config, &["list", "--include-missing", "--json"]).stdout).unwrap().is_empty());
}

#[test]
fn delete_refuses_a_replaced_file_and_preserves_index() {
    let (dir, config) = fixture();
    assert!(run(&config, &["sync", "--include-unavailable"]).status.success());
    let path = dir.path().join("pi/session.jsonl");
    fs::write(&path, "{\"type\":\"session\",\"id\":\"other\"}\n").unwrap();
    assert!(!run(&config, &["delete", "pi:ui", "--yes"]).status.success());
    assert!(path.exists());
    assert!(run(&config, &["show", "pi:ui"]).status.success());
    #[cfg(unix)]
    {
        let outside = dir.path().join("outside.jsonl");
        fs::rename(&path, &outside).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        let result = run(&config, &["delete", "pi:ui", "--yes"]);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("outside its indexed native root"));
        assert!(outside.exists());
    }
}

#[cfg(unix)]
#[test]
fn opencode_delete_verifies_native_state_and_refuses_recursive_delete() {
    let (dir, config) = fixture();
    let root = dir.path();
    let native = root.join("opencode/opencode.db");
    fs::create_dir(root.join("opencode")).unwrap();
    let conn = rusqlite::Connection::open(&native).unwrap();
    conn.execute_batch("CREATE TABLE session(id TEXT,title TEXT,directory TEXT,time_created INTEGER,time_updated INTEGER,parent_id TEXT);
        CREATE TABLE message(id TEXT,session_id TEXT,time_created INTEGER,data TEXT);
        INSERT INTO session VALUES('ses_delete','delete','/tmp',1,2,NULL);
        INSERT INTO session VALUES('ses_keep','keep','/tmp',1,2,NULL);").unwrap();
    assert!(run(&config, &["sync", "--include-unavailable"]).status.success());
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    executable(&bin.join("opencode"), "#!/bin/sh\nexit 0\n");
    let call = || Command::new(env!("CARGO_BIN_EXE_agenthub")).arg("--config").arg(&config).args(["delete", "opencode:ses_delete", "--yes"]).env("PATH", &bin).output().unwrap();
    let result = call();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("still contains"));
    assert!(run(&config, &["show", "opencode:ses_delete"]).status.success());
    conn.execute("UPDATE session SET parent_id='ses_delete' WHERE id='ses_keep'", []).unwrap();
    assert!(String::from_utf8_lossy(&call().stderr).contains("child sessions"));
    conn.execute("UPDATE session SET parent_id=NULL WHERE id='ses_keep'", []).unwrap();
    // Simulate a successful native command with a controlled SQLite helper.
    let python = Command::new("python3").args(["-c", "import sys; print(sys.executable)"]).output().unwrap();
    let python = String::from_utf8(python.stdout).unwrap();
    executable(&bin.join("opencode"), &format!("#!{}\nimport os,sys,sqlite3\nassert sys.argv[1:3]==['session','delete']\nassert sys.argv[-1]=='--pure'\nc=sqlite3.connect(os.path.join(os.environ['XDG_DATA_HOME'],'opencode','opencode.db'))\nc.execute('DELETE FROM session WHERE id=?',(sys.argv[3],))\nc.commit()\n", python.trim()));
    assert!(call().status.success());
    assert!(!run(&config, &["show", "opencode:ses_delete"]).status.success());
    assert!(run(&config, &["show", "opencode:ses_keep"]).status.success());
    assert_eq!(conn.query_row("SELECT count(*) FROM session", [], |r|r.get::<_,i64>(0)).unwrap(), 1);
}

#[test]
fn claude_and_codex_native_files_delete_independently() {
    let (dir, config) = fixture();
    let root = dir.path();
    for (agent, subdir, header) in [
        ("claude", "project", "{\"sessionId\":\"delete-claude\",\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"delete fixture\"}}"),
        ("codex", "sessions", "{\"type\":\"session_meta\",\"payload\":{\"id\":\"delete-codex\",\"cwd\":\"/tmp\"}}"),
    ] {
        let source = root.join(agent).join(subdir);
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("delete.jsonl"), format!("{header}\n")).unwrap();
        let other = header.replace(&format!("delete-{agent}"), &format!("keep-{agent}"));
        fs::write(source.join("keep.jsonl"), format!("{other}\n")).unwrap();
        assert!(run(&config, &["sync", "--include-unavailable", "--agent", agent]).status.success());
        let result = run(&config, &["delete", &format!("{agent}:delete-{agent}"), "--yes"]);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        assert!(!source.join("delete.jsonl").exists());
        assert!(source.join("keep.jsonl").exists());
        assert!(run(&config, &["show", &format!("{agent}:keep-{agent}")]).status.success());
    }
}

#[test]
fn agy_delete_refuses_unverified_native_writes_without_hiding_history() {
    let (dir, config) = fixture();
    let root = dir.path().join("agy");
    let conversations = root.join("conversations");
    fs::create_dir_all(&conversations).unwrap();
    let sql = include_str!("../../../tests/fixtures/agy/1.5.6.sql");
    let (summary, conversation) = sql.split_once("-- CONVERSATION_DB: 93748d3b-ae91-447e-a184-c16dc21090c9.db\n").unwrap();
    let source = conversations.join("93748d3b-ae91-447e-a184-c16dc21090c9.db");
    let summary_path = root.join("conversation_summaries.db");
    rusqlite::Connection::open(&source).unwrap().execute_batch(conversation).unwrap();
    rusqlite::Connection::open(&summary_path).unwrap().execute_batch(summary).unwrap();
    assert!(run(&config, &["sync", "--include-unavailable", "--agent", "agy"]).status.success());
    let before = (fs::read(&source).unwrap(), fs::read(&summary_path).unwrap());
    let key = "agy:93748d3b-ae91-447e-a184-c16dc21090c9";
    let result = run(&config, &["delete", key, "--yes"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("no verified native deletion API"));
    assert_eq!((fs::read(&source).unwrap(), fs::read(&summary_path).unwrap()), before);
    assert!(run(&config, &["show", key]).status.success());
}

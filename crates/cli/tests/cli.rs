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
    let dsh_root = home.join("dsh/sessions");
    fs::create_dir_all(dsh_root.join("--tmp--/session-dsh-1")).unwrap();
    fs::write(dsh_root.join("--tmp--/session-dsh-1/session.v4.jsonl"), concat!(
        "{\"type\":\"session\",\"version\":4,\"id\":\"session-dsh-1\",\"createdAt\":1794450000000,\"cwd\":\"/tmp\"}\n",
        "{\"type\":\"user/message\",\"seq\":0,\"time\":1794450001000,\"data\":{\"id\":\"u1\",\"role\":\"user\",\"source\":{\"kind\":\"user\"},\"content\":[{\"type\":\"text\",\"text\":\"dsh prompt\"}]}}\n"
    )).unwrap();
    let config = home.join("config.toml");
    let mut config_text = format!("database = {:?}\n[sources]\n",home.join("index.db").to_str().unwrap());
    for agent in ["claude","codex","opencode","pi","agy","dsh"] {
        let path = if agent == "pi" {
            sources.clone()
        } else if agent == "dsh" {
            dsh_root.clone()
        } else {
            home.join(agent)
        };
        config_text.push_str(&format!("{agent} = {:?}\n",path.to_str().unwrap()));
    }
    fs::write(&config,config_text).unwrap();
    let result = run(&config,&["sync","--json"]);
    assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["pi"]["indexed"],1);
    assert_eq!(report["dsh"]["indexed"],1);
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
    let dsh_resume = run(&config,&["resume","dsh:session-dsh-1","--json"]);
    assert!(dsh_resume.status.success());
    let dsh_plan: serde_json::Value = serde_json::from_slice(&dsh_resume.stdout).unwrap();
    assert_eq!(dsh_plan["program"],"dsh");
    assert_eq!(dsh_plan["args"],serde_json::json!(["--profile","headless","--session-id","session-dsh-1"]));
    assert_eq!(dsh_plan["env"][0][0],"DSH_HOME");
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
    for agent in ["claude", "codex", "opencode", "pi", "agy", "dsh"] {
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
    assert!(run(&config, &["sync"]).status.success());
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
    assert!(run(&config, &["sync"]).status.success());
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
    assert!(run(&config, &["sync"]).status.success());
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

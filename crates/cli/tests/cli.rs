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
    let result = run(&config,&["sync","--json"]);
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

use agenthub_adapters::{adapter, default_root};
use agenthub_core::{resume_command, Agent, ResumeCommand};
use agenthub_index::{Filter, Store};
use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use serde::Deserialize;
use std::{collections::BTreeMap, env, fs, io::{self, Write}, path::{Path, PathBuf}, process::{Command, ExitCode}};

#[derive(Parser)]
#[command(name="agenthub", version, about="Read-only coding-agent session index (Linux / macOS)")]
struct Cli {
    /// Index path; never point this at an agent database.
    #[arg(long, global=true, env="AGENTHUB_DB")]
    db: Option<PathBuf>,
    /// TOML configuration (default: $XDG_CONFIG_HOME/agenthub/config.toml).
    #[arg(long, global=true, env="AGENTHUB_CONFIG")]
    config: Option<PathBuf>,
    /// Machine-readable output. `show` streams JSON Lines.
    #[arg(long, global=true)]
    json: bool,
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Discover sessions and incrementally refresh the local index.
    Sync {
        #[arg(long)] agent: Option<Agent>,
        /// Ignore source fingerprints and reparse all selected sources.
        #[arg(long)] force: bool,
    },
    /// List indexed sessions, newest update first. Does not scan source files.
    List(Selection),
    /// Show an entire native transcript in record order.
    Show {
        id: String,
        /// Show original JSON or tagged binary payloads.
        #[arg(long)] raw: bool,
        #[arg(long, default_value_t=0)] offset: u32,
        #[arg(long)] limit: Option<u32>,
    },
    /// Literal substring search across titles, project paths, and conversation text.
    Search { query: String, #[command(flatten)] selection: Selection },
    /// Print the native resume command, or run it with --exec.
    Resume { id: String, #[arg(long="exec")] execute: bool },
    /// Inspect source paths, read capabilities, and installed native executables.
    Doctor,
}

#[derive(Args)]
struct Selection {
    #[arg(long)] agent: Option<Agent>,
    /// Exact project working directory.
    #[arg(long)] project: Option<String>,
    /// Updated since RFC3339 timestamp, date (UTC), or epoch milliseconds (inclusive).
    #[arg(long)] since: Option<String>,
    /// Updated before timestamp/date (exclusive).
    #[arg(long)] until: Option<String>,
    #[arg(long)] include_missing: bool,
    #[arg(long, default_value_t=50, value_parser=clap::value_parser!(u32).range(1..=10000))] limit: u32,
    #[arg(long, default_value_t=0)] offset: u32,
}
impl Selection {
    fn filter(&self) -> Result<Filter> {
        let filter = Filter { agent:self.agent,project:self.project.clone(),since:self.since.as_deref().map(parse_time).transpose()?,until:self.until.as_deref().map(parse_time).transpose()?,include_missing:self.include_missing,limit:self.limit,offset:self.offset };
        if matches!((filter.since,filter.until),(Some(a),Some(b)) if a >= b) { bail!("--since must be earlier than --until"); }
        Ok(filter)
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config { database: Option<PathBuf>, #[serde(default)] sources: BTreeMap<String, PathBuf> }

struct Settings { db: PathBuf, roots: BTreeMap<String, PathBuf> }
impl Settings {
    fn load(cli: &Cli) -> Result<Self> {
        let home = env::var_os("HOME").map(PathBuf::from).context("HOME must be set")?;
        let config_home = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        let path = cli.config.clone().unwrap_or_else(|| config_home.join("agenthub/config.toml"));
        let config: Config = match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("read config {}", path.display()))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound && cli.config.is_none() => Config::default(),
            Err(e) => return Err(e).context("read requested config"),
        };
        let mut roots = BTreeMap::new();
        for name in config.sources.keys() { let _: Agent = name.parse()?; }
        for agent in Agent::ALL {
            let root = config.sources.get(agent.as_str()).cloned().unwrap_or_else(|| default_root(agent,&home));
            roots.insert(agent.to_string(),absolute(&root,&home)?);
        }
        let data = env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
        let db = absolute(&cli.db.clone().or(config.database).unwrap_or_else(|| data.join("agenthub/index.db")),&home)?;
        // Reject accidental overlap with known source databases before opening the index.
        for (name, root) in &roots {
            if db == *root || db.starts_with(root) {
                bail!("index path overlaps the {name} source root; choose an AgentHub-only location");
            }
        }
        Ok(Self { db,roots })
    }
    fn root(&self, agent: Agent) -> Result<PathBuf> { self.roots.get(agent.as_str()).cloned().context("missing source root") }
}

fn absolute(path: &Path, home: &Path) -> Result<PathBuf> {
    let path = if path == Path::new("~") { home.to_path_buf() }
        else if let Ok(rest) = path.strip_prefix("~/") { home.join(rest) } else { path.to_path_buf() };
    let path = if path.is_absolute() { path } else { env::current_dir()?.join(path) };
    // Canonicalize existing paths so a symlink cannot bypass source/index overlap checks.
    if path.try_exists()? { return Ok(fs::canonicalize(path)?); }
    if let Some(parent) = path.parent() {
        if parent.try_exists()? {
            return Ok(fs::canonicalize(parent)?.join(path.file_name().context("missing filename")?));
        }
    }
    Ok(path)
}

fn parse_time(s: &str) -> Result<i64> {
    if let Ok(t) = s.parse::<i64>() { return Ok(t); }
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) { return Ok(t.timestamp_millis()); }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s,"%Y-%m-%d") {
        return Ok(date.and_hms_opt(0,0,0).context("invalid midnight")?.and_utc().timestamp_millis());
    }
    bail!("invalid time {s:?}: use RFC3339, YYYY-MM-DD (UTC), or epoch milliseconds")
}

fn safe(text: &str) -> String {
    // Sessions are untrusted terminal content; preserve only printable text and line breaks.
    text.chars().filter(|c| !c.is_control() || matches!(c,'\n'|'\t')).collect()
}
fn cell(text: &str) -> String { safe(text).replace(['\n','\t']," ") }
fn quote(s: &str) -> String { format!("'{}'",s.replace('\'',"'\\''")) }
fn display_plan(plan: &ResumeCommand) -> String {
    let mut s = String::new();
    if let Some(cwd) = &plan.cwd { s.push_str(&format!("cd {} && ",quote(&cwd.to_string_lossy()))); }
    for (key,val) in &plan.env { s.push_str(&format!("{key}={} ",quote(&val.to_string_lossy()))); }
    s.push_str(&quote(&plan.program));
    for arg in &plan.args { s.push(' '); s.push_str(&quote(arg)); }
    s
}

fn plan_for(store: &Store, record: &agenthub_index::SessionRecord) -> Result<ResumeCommand> {
    if !record.available { bail!("source is unavailable; locate the original session and run sync before resuming"); }
    let s = &record.session;
    if !s.source_path.try_exists()? { bail!("original session no longer exists; run sync to refresh availability"); }
    let mut plan = resume_command(s)?;
    let root = store.source_root(&s.key)?;
    // Honor the indexed agent installation, even when the CLI's current environment differs.
    match s.agent {
        Agent::Claude => if let Some(p) = root.parent() { plan.env.push(("CLAUDE_CONFIG_DIR".into(),p.into())); },
        Agent::Codex => plan.env.push(("CODEX_HOME".into(),root)),
        Agent::Pi => {}
        Agent::Opencode => {
            let directory = if root.is_file() {
                if root.file_name().is_none_or(|s|s != "opencode.db") { bail!("OpenCode custom database filename cannot be resumed with a verified native flag"); }
                root.parent().context("missing OpenCode directory")?
            } else { root.as_path() };
            if directory.file_name().is_none_or(|s|s != "opencode") { bail!("OpenCode resume requires the <XDG_DATA_HOME>/opencode directory layout"); }
            let data_root = if root.is_file() { root.parent().and_then(Path::parent) } else { root.parent() };
            if let Some(p) = data_root { plan.env.push(("XDG_DATA_HOME".into(),p.into())); }
        }
        Agent::Agy => {
            let home = env::var_os("HOME").map(PathBuf::from).context("HOME must be set")?;
            if root != fs::canonicalize(home.join(".gemini/antigravity-cli")).unwrap_or_else(|_| home.join(".gemini/antigravity-cli")) {
                bail!("AGY does not expose a verified custom data-root flag; launch the matching AGY installation manually");
            }
        }
    }
    Ok(plan)
}

fn run(cli: Cli) -> Result<bool> {
    let settings = Settings::load(&cli)?;
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    if matches!(&cli.command,Action::Doctor) {
        let paths = env::var_os("PATH").map(|v| env::split_paths(&v).collect::<Vec<_>>()).unwrap_or_default();
        let mut results = Vec::new();
        for agent in Agent::ALL {
            let root = settings.root(agent)?;
            let a = adapter(agent,root.clone());
            let discovery = a.discover();
            let (count,warnings,error) = match discovery { Ok(d) => (d.sources.len(),d.warnings,None),Err(e) => (0,vec![],Some(format!("{e:#}"))) };
            let binary = paths.iter().map(|p| p.join(agent.as_str())).find(|p| p.is_file());
            results.push(serde_json::json!({"agent":agent,"root":root,"exists":root.try_exists()?,"sources":count,"binary":binary,"warnings":warnings,"error":error}));
        }
        if cli.json { writeln!(out,"{}",serde_json::to_string_pretty(&results)?)?; }
        else {
            writeln!(out,"Index: {}",cell(&settings.db.to_string_lossy()))?;
            for r in results { writeln!(out,"{}\t{}\tsources={}\tbinary={}\terror={}",r["agent"].as_str().unwrap_or(""),cell(r["root"].as_str().unwrap_or("")),r["sources"],r["binary"],r["error"])?; }
        }
        return Ok(true);
    }
    let mut store = Store::open(&settings.db)?;
    match cli.command {
        Action::Sync { agent,force } => {
            let mut reports = BTreeMap::new();
            let mut success = true;
            for selected in Agent::ALL.into_iter().filter(|a| agent.is_none_or(|f| f == *a)) {
                let a = adapter(selected,settings.root(selected)?);
                let report = match store.sync(a.as_ref(),force) {
                    Ok(r) => r,
                    Err(e) => agenthub_index::SyncReport { errors:vec![format!("{e:#}")],..Default::default() },
                };
                success &= report.errors.is_empty();
                if !cli.json {
                    writeln!(out,"{selected}: discovered={} indexed={} unchanged={} missing={}",report.discovered,report.indexed,report.unchanged,report.missing)?;
                    for warning in &report.warnings { eprintln!("warning: {}",cell(warning)); }
                    for error in &report.errors { eprintln!("error: {}",cell(error)); }
                }
                reports.insert(selected.to_string(),report);
            }
            if cli.json { writeln!(out,"{}",serde_json::to_string_pretty(&reports)?)?; }
            out.flush()?;
            return Ok(success);
        }
        Action::List(selection) => {
            let records = store.list(&selection.filter()?)?;
            if cli.json { writeln!(out,"{}",serde_json::to_string_pretty(&records)?)?; }
            else {
                writeln!(out,"KEY\tUPDATED(ms)\tRECORDS\tAVAILABLE\tPROJECT\tTITLE")?;
                for record in records { let s=record.session; writeln!(out,"{}\t{}\t{}\t{}\t{}\t{}",cell(&s.key),s.updated_at.map(|v|v.to_string()).unwrap_or_default(),record.message_count,record.available,cell(&s.project_path.unwrap_or_default().to_string_lossy()),cell(&s.title))?; }
            }
        }
        Action::Show { id,raw,offset,limit } => {
            let record = store.find(&id)?;
            if cli.json { writeln!(out,"{}",serde_json::json!({"type":"session","session":record}))?; }
            else {
                writeln!(out,"{} — {}",cell(&record.session.key),cell(&record.session.title))?;
                if !record.available { writeln!(out,"[original source unavailable; showing cached history]")?; }
                for warning in &record.session.warnings { writeln!(out,"[{}]",cell(warning))?; }
            }
            store.visit_messages(&record.session.key,offset,limit,|message| {
                if cli.json { writeln!(out,"{}",serde_json::json!({"type":"message","message":message}))?; }
                else if raw { writeln!(out,"{}",serde_json::to_string(&message.raw)?)?; }
                else {
                    writeln!(out,"\n[{}] {} ({})",message.ordinal,cell(&message.role),cell(&message.kind))?;
                    writeln!(out,"{}",safe(&message.text))?;
                }
                Ok(())
            })?;
        }
        Action::Search { query,selection } => {
            let hits = store.search(&query,&selection.filter()?)?;
            if cli.json { writeln!(out,"{}",serde_json::to_string_pretty(&hits)?)?; }
            else { for h in hits { writeln!(out,"{}\t#{}\t{}\n{}\n",cell(&h.key),h.ordinal,cell(&h.title),safe(&h.excerpt))?; } }
        }
        Action::Resume { id,execute } => {
            let record = store.find(&id)?;
            let plan = plan_for(&store,&record)?;
            if !execute {
                if cli.json { writeln!(out,"{}",serde_json::to_string_pretty(&plan)?)?; }
                else { writeln!(out,"{}",display_plan(&plan))?; }
            } else {
                out.flush()?;
                drop(out);
                drop(store);
                let mut command = Command::new(&plan.program);
                command.args(&plan.args);
                for (key,value) in &plan.env { command.env(key,value); }
                if let Some(cwd) = &plan.cwd { command.current_dir(cwd); }
                // exec preserves terminal signals and the native exit status on both supported platforms.
                #[cfg(unix)]
                { use std::os::unix::process::CommandExt; return Err(command.exec()).with_context(|| format!("launch {}",plan.program)); }
                #[cfg(not(unix))]
                { return Ok(command.status()?.success()); }
            }
        }
        Action::Doctor => unreachable!(),
    }
    out.flush()?;
    Ok(true)
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            if error.downcast_ref::<io::Error>().is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe) { return ExitCode::SUCCESS; }
            eprintln!("agenthub: {}",cell(&format!("{error:#}")));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dates_and_shell_quoting() {
        assert_eq!(parse_time("2026-11-12").unwrap(),parse_time("2026-11-12T00:00:00Z").unwrap());
        assert!(parse_time("tomorrow").is_err());
        assert_eq!(quote("a'b;$(x)"),"'a'\\''b;$(x)'");
        assert_eq!(safe("hello\u{1b}[31m\u{7}world"),"hello[31mworld");
    }
    #[test]
    fn cli_definition_is_valid() { use clap::CommandFactory; Cli::command().debug_assert(); }
}

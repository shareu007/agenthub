use agenthub_adapters::{adapter, default_root};
use agenthub_core::{resume_command, Agent, ResumeCommand};
use agenthub_index::{Filter, Store};
use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use serde::Deserialize;
use std::{collections::BTreeMap, env, fs, io::{self, Write}, path::{Path, PathBuf}, process::{Command, ExitCode}};
mod ui;
mod listing;
use ui::{cell, safe, Ui};

#[derive(Parser)]
#[command(name="agenthub", version, about="Find, read, and resume your coding-agent sessions",
    long_about="A local, read-only session index for Claude Code, Codex, OpenCode, Pi, and AGY.\nRuns on demand on Linux and macOS. Run without a subcommand to browse sessions. Press Enter to resume, or use resume --exec.",
    after_help="QUICK START:\n  agenthub doctor\n  agenthub sync\n  ah\n  agenthub list --agent codex --group-by project --sort time\n  agenthub search \"rendering\" --project /path/to/project\n  agenthub show codex:SESSION_ID\n  agenthub resume codex:SESSION_ID --exec\n\nUse --json for scripts. Date filters and displayed times use UTC.")]
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
    /// Color for human output; auto respects NO_COLOR, TERM=dumb, and redirected stdout.
    #[arg(long, global=true, value_enum, default_value="auto")]
    color: ui::Color,
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Discover sessions and incrementally refresh the local index.
    Sync {
        #[arg(long)] agent: Option<Agent>,
        /// Ignore source fingerprints and reparse all selected sources.
        #[arg(long)] force: bool,
    },
    /// Browse indexed sessions. In a terminal, select and press Enter to resume.
    List(listing::ListArgs),
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

#[derive(Args, Clone)]
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
impl Default for Selection {
    fn default() -> Self {
        Self { agent: None, project: None, since: None, until: None, include_missing: false, limit: 50, offset: 0 }
    }
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
        Agent::Dsh => {
            if root.file_name().is_none_or(|s| s != "sessions") {
                bail!("DeepSeek Harness resume requires the <DSH_HOME>/sessions directory layout");
            }
            if let Some(p) = root.parent() {
                plan.env.push(("DSH_HOME".into(), p.into()));
            }
        }
    }
    Ok(plan)
}

fn run(cli: Cli) -> Result<bool> {
    let settings = Settings::load(&cli)?;
    let ui = Ui::new(cli.color, cli.json);
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    if matches!(&cli.command,Some(Action::Doctor)) {
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
            writeln!(out,"{}\nIndex: {}\n",ui.heading("AgentHub · Source health"),cell(&settings.db.to_string_lossy()))?;
            for r in &results {
                let status = if !r["error"].is_null() { "ERROR" } else if !r["warnings"].as_array().is_none_or(Vec::is_empty) { "WARNING" } else if r["exists"] == false { "NOT FOUND" } else { "READY" };
                writeln!(out,"{}  {status} · {} source(s)",ui.heading(r["agent"].as_str().unwrap_or("")),r["sources"])?;
                writeln!(out,"  Source: {}\n  Native CLI: {}",cell(r["root"].as_str().unwrap_or("")),cell(r["binary"].as_str().unwrap_or("not installed (needed only for resume --exec)")))?;
                if let Some(e) = r["error"].as_str() { writeln!(out,"  Error: {}",cell(e))?; }
                if let Some(warnings) = r["warnings"].as_array() { for w in warnings { writeln!(out,"  Warning: {}",cell(w.as_str().unwrap_or("")))?; } }
                writeln!(out)?;
            }
            writeln!(out,"Checks discovery only; does not verify format compatibility, login, or native recovery.\nNext: agenthub sync")?;
        }
        return Ok(results.iter().all(|r| r["error"].is_null() && r["warnings"].as_array().is_none_or(Vec::is_empty)));
    }
    let mut store = Store::open(&settings.db)?;
    match cli.command.unwrap_or_else(|| Action::List(listing::ListArgs::default())) {
        Action::Sync { agent,force } => {
            let mut reports = BTreeMap::new();
            let mut success = true;
            if !cli.json { writeln!(out,"{}\n",ui.heading("AgentHub · Sync"))?; }
            for selected in Agent::ALL.into_iter().filter(|a| agent.is_none_or(|f| f == *a)) {
                let a = adapter(selected,settings.root(selected)?);
                let report = match store.sync(a.as_ref(),force) {
                    Ok(r) => r,
                    Err(e) => agenthub_index::SyncReport { errors:vec![format!("{e:#}")],..Default::default() },
                };
                success &= report.errors.is_empty();
                if !cli.json {
                    let status = if report.errors.is_empty() { "OK" } else { "ERROR" };
                    writeln!(out,"{selected:<9} {status:<5} {} indexed · {} unchanged · {} missing · {} discovered",report.indexed,report.unchanged,report.missing,report.discovered)?;
                    for warning in &report.warnings { eprintln!("warning: {}",cell(warning)); }
                    for error in &report.errors { eprintln!("error: {}",cell(error)); }
                }
                reports.insert(selected.to_string(),report);
            }
            if cli.json { writeln!(out,"{}",serde_json::to_string_pretty(&reports)?)?; }
            else { writeln!(out,"\n{}\nNext: agenthub list",if success { "Sync complete." } else { "Sync incomplete. Failed sources retain their last successful index; see errors above." })?; }
            out.flush()?;
            return Ok(success);
        }
        Action::List(args) => {
            let interactive = args.use_terminal(cli.json)?;
            if interactive {
                out.flush()?;
                drop(out);
                let plan = listing::choose(&store, args)?;
                drop(store);
                return match plan { Some(plan) => execute_plan(plan), None => Ok(true) };
            }
            let records = args.records(&store)?;
            if cli.json { writeln!(out,"{}",serde_json::to_string_pretty(&records)?)?; }
            else { listing::print(&mut out, &records, &args, &ui)?; }
        }
        Action::Show { id,raw,offset,limit } => {
            let record = store.find(&id)?;
            if cli.json { writeln!(out,"{}",serde_json::json!({"type":"session","session":record}))?; }
            else if !raw {
                writeln!(out,"{}  {}",ui.heading(&record.session.key),cell(&record.session.title))?;
                writeln!(out,"Updated: {} · {} records",ui::time(record.session.updated_at),record.message_count)?;
                writeln!(out,"Project: {}",cell(&record.session.project_path.as_deref().map(|p|p.to_string_lossy()).unwrap_or_default()))?;
                if !record.available { writeln!(out,"[original source unavailable; showing cached history]")?; }
                for warning in &record.session.warnings { writeln!(out,"[{}]",cell(warning))?; }
            }
            store.visit_messages(&record.session.key,offset,limit,|message| {
                if cli.json { writeln!(out,"{}",serde_json::json!({"type":"message","message":message}))?; }
                else if raw { writeln!(out,"{}",serde_json::to_string(&message.raw)?)?; }
                else {
                    writeln!(out,"\n{} · #{} · {} · {}",ui.heading(&message.role.to_uppercase()),message.ordinal,cell(&message.kind),ui::time(message.timestamp))?;
                    writeln!(out,"{}",safe(&message.text))?;
                }
                Ok(())
            })?;
        }
        Action::Search { query,selection } => {
            let hits = store.search(&query,&selection.filter()?)?;
            if cli.json { writeln!(out,"{}",serde_json::to_string_pretty(&hits)?)?; }
            else {
                writeln!(out,"{} · {} hit(s) for {:?}\n",ui.heading("AgentHub · Search"),hits.len(),cell(&query))?;
                if hits.is_empty() { writeln!(out,"No matches. Try a shorter keyword, adjust filters, or run agenthub sync.")?; }
                for h in &hits { writeln!(out,"{} · {}  {}\n{}\n",ui.heading(&h.key),if h.ordinal < 0 { "title/project".into() } else { format!("record #{}",h.ordinal) },cell(&h.title),safe(&h.excerpt))?; }
                if !hits.is_empty() { writeln!(out,"Read context: agenthub show <agent:id>")?; }
                if hits.len() == selection.limit as usize { writeln!(out,"Next page (keep your query and filters): --offset {} --limit {}",u64::from(selection.offset)+u64::from(selection.limit),selection.limit)?; }
            }
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
                return execute_plan(plan);
            }
        }
        Action::Doctor => unreachable!(),
    }
    out.flush()?;
    Ok(true)
}

fn execute_plan(plan: ResumeCommand) -> Result<bool> {
    let mut command = Command::new(&plan.program);
    command.args(&plan.args);
    for (key, value) in &plan.env { command.env(key, value); }
    if let Some(cwd) = &plan.cwd { command.current_dir(cwd); }
    #[cfg(unix)]
    { use std::os::unix::process::CommandExt; Err(command.exec()).with_context(|| format!("launch {}; check that the native CLI is installed and on PATH", plan.program)) }
    #[cfg(not(unix))]
    { Ok(command.status()?.success()) }
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

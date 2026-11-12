//! Grouped output and an on-demand terminal selector; never used for JSON/pipes.
use crate::{plan_for, ui::{self, cell, Ui}, Selection};
use agenthub_core::ResumeCommand;
use agenthub_index::{GroupBy, ListOrder, SessionRecord, SortKey, Store};
use anyhow::{bail, Result};
use clap::{Args, ValueEnum};
use crossterm::{cursor, event::{self, Event, KeyCode, KeyEventKind, KeyModifiers}, execute,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen}};
use std::io::{self, IsTerminal, Write};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Sort {
    #[default]
    #[value(alias="updated", alias="updated-at")]
    Time,
    Created,
    Agent,
    Project,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Group {
    None,
    #[default]
    Agent,
    Project,
}

#[derive(Args, Clone, Default)]
pub struct ListArgs {
    #[command(flatten)]
    pub selection: Selection,
    /// Classify sessions before sorting (default: agent).
    #[arg(long, value_enum, default_value="agent")]
    pub group_by: Group,
    /// Order within each group. Time/created: newest first; agent/project: A-Z.
    #[arg(long, value_enum, default_value="time")]
    pub sort: Sort,
    /// Reverse the selected sort direction (groups remain A-Z).
    #[arg(long)]
    pub reverse: bool,
    /// Print the list without selecting or opening a native agent.
    #[arg(long, conflicts_with="interactive")]
    pub no_interactive: bool,
    /// Require a terminal selector (fails if stdin/stdout are redirected).
    #[arg(long, conflicts_with="no_interactive")]
    pub interactive: bool,
}
impl ListArgs {
    fn order(&self) -> ListOrder {
        ListOrder {
            group: match self.group_by { Group::None => GroupBy::None, Group::Agent => GroupBy::Agent, Group::Project => GroupBy::Project },
            sort: match self.sort { Sort::Time => SortKey::Updated, Sort::Created => SortKey::Created, Sort::Agent => SortKey::Agent, Sort::Project => SortKey::Project },
            reverse: self.reverse,
        }
    }
    pub fn records(&self, store: &Store) -> Result<Vec<SessionRecord>> {
        store.list_ordered(&self.selection.filter()?, self.order())
    }
    pub fn use_terminal(&self, json: bool) -> Result<bool> {
        let capable = io::stdin().is_terminal() && io::stdout().is_terminal()
            && std::env::var_os("TERM").is_none_or(|v| v != "dumb");
        if self.interactive && (json || !capable) {
            bail!("--interactive requires terminal stdin/stdout, TERM other than dumb, and no --json; use --no-interactive for scripts");
        }
        Ok(!json && !self.no_interactive && capable)
    }
}

fn group_name(record: &SessionRecord, group: Group) -> String {
    match group {
        Group::None => String::new(),
        Group::Agent => record.session.agent.to_string(),
        Group::Project => record.session.project_path.as_deref()
            .map(|p|p.to_string_lossy().into_owned()).unwrap_or_else(|| "(no project)".into()),
    }
}

pub fn print(out: &mut impl Write, records: &[SessionRecord], args: &ListArgs, ui: &Ui) -> Result<()> {
    writeln!(out, "{} · {} shown · group: {:?} · sort: {:?}{}\n", ui.heading("AgentHub · Sessions"), records.len(), args.group_by, args.sort, if args.reverse { " (reverse)" } else { "" })?;
    if records.is_empty() {
        writeln!(out,"No sessions match. Run agenthub sync, or adjust your filters.\nUse --include-missing to include cached sessions whose sources disappeared.")?;
    }
    let mut previous = None;
    for record in records {
        let group = group_name(record, args.group_by);
        if args.group_by != Group::None && previous.as_ref() != Some(&group) {
            writeln!(out, "── {} ──", ui.heading(&group))?;
            previous = Some(group);
        }
        let s = &record.session;
        writeln!(out, "{}  {}", ui.heading(&s.key), cell(&s.title))?;
        writeln!(out, "  {} · {} records · {}", ui::time(s.updated_at), record.message_count, if record.available { "source available" } else { "cached only" })?;
        writeln!(out, "  Project: {}\n", cell(&s.project_path.as_deref().map(|p|p.to_string_lossy()).unwrap_or_default()))?;
    }
    if !records.is_empty() {
        writeln!(out, "View: agenthub show <agent:id>\nResume: agenthub resume <agent:id> --exec")?;
        if records.len() == args.selection.limit as usize {
            writeln!(out, "Next page (keep your filters): --offset {} --limit {}", u64::from(args.selection.offset)+u64::from(args.selection.limit), args.selection.limit)?;
        }
    }
    Ok(())
}

// The guard is dropped before exec, including on cancellation, read errors, and
// invalid resume plans. Do not leave the native agent in raw/alternate-screen mode.
struct Screen;
impl Screen {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(io::stdout(), EnterAlternateScreen, cursor::Hide)?;
        Ok(guard)
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), cursor::Show, LeaveAlternateScreen);
    }
}

fn clip(text: &str, width: usize) -> String {
    let clean = cell(text);
    let mut used = 0;
    clean.chars().take_while(|c| {
        used += c.width().unwrap_or(0);
        used <= width
    }).collect()
}
fn line(out: &mut impl Write, text: &str, width: u16) -> Result<()> {
    write!(out, "{}\r\n", clip(text, width.saturating_sub(1) as usize))?;
    Ok(())
}

pub fn choose(store: &Store, mut args: ListArgs) -> Result<Option<ResumeCommand>> {
    let _screen = Screen::enter()?;
    let mut records = args.records(store)?;
    let mut selected = 0usize;
    let mut message = String::new();
    loop {
        let (width, height) = terminal::size()?;
        let per_screen = (height.saturating_sub(9) as usize / 4).max(1);
        let start = selected / per_screen * per_screen;
        let mut out = io::stdout().lock();
        execute!(out, cursor::MoveTo(0, 0), Clear(ClearType::All))?;
        line(&mut out, &format!("AgentHub · Sessions · {} shown · offset {}", records.len(), args.selection.offset), width)?;
        line(&mut out, &format!("Group: {:?} [g]  Sort: {:?} [s]{} [r]", args.group_by, args.sort, if args.reverse { " reversed" } else { "" }), width)?;
        let mut previous = None;
        for (index, record) in records.iter().enumerate().skip(start).take(per_screen) {
            let group = group_name(record, args.group_by);
            if args.group_by != Group::None && previous.as_ref() != Some(&group) {
                line(&mut out, &format!("── {group} ──"), width)?;
                previous = Some(group);
            }
            let s = &record.session;
            line(&mut out, &format!("{} {}  {}", if index == selected { ">" } else { " " }, s.agent, s.title), width)?;
            line(&mut out, &format!("  {} · {} · {}", ui::time(s.updated_at), if record.available { "available" } else { "cached only" }, s.key), width)?;
            line(&mut out, &format!("  {}", s.project_path.as_deref().map(|p|p.to_string_lossy()).unwrap_or_default()), width)?;
        }
        if records.is_empty() { line(&mut out, "No sessions match. Run agenthub sync or change filters.", width)?; }
        // Keep controls visible even on a terminal too short for the rows above.
        execute!(out, cursor::MoveTo(0, height.saturating_sub(4)), Clear(ClearType::FromCursorDown))?;
        line(&mut out, &message, width)?;
        line(&mut out, "Up/Down j/k: select · Enter: resume · q/Esc/Ctrl-C: quit", width)?;
        line(&mut out, "g: group · s: sort · r: reverse · n/p: next/previous data page", width)?;
        out.flush()?;
        drop(out);
        let Event::Key(key) = event::read()? else { continue; };
        if key.kind == KeyEventKind::Release { continue; }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) { return Ok(None); }
        let mut reload = false;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(records.len().saturating_sub(1)),
            KeyCode::Home => selected = 0,
            KeyCode::End => selected = records.len().saturating_sub(1),
            KeyCode::PageUp => selected = selected.saturating_sub(per_screen),
            KeyCode::PageDown => selected = (selected + per_screen).min(records.len().saturating_sub(1)),
            KeyCode::Enter => if let Some(record) = records.get(selected) {
                match plan_for(store, record) {
                    Ok(plan) => return Ok(Some(plan)),
                    Err(e) => message = format!("Cannot resume: {e:#}"),
                }
            },
            KeyCode::Char('g') => { args.group_by = match args.group_by { Group::None => Group::Agent, Group::Agent => Group::Project, Group::Project => Group::None }; args.selection.offset = 0; reload = true; }
            KeyCode::Char('s') => { args.sort = match args.sort { Sort::Time => Sort::Created, Sort::Created => Sort::Agent, Sort::Agent => Sort::Project, Sort::Project => Sort::Time }; args.selection.offset = 0; reload = true; }
            KeyCode::Char('r') => { args.reverse = !args.reverse; args.selection.offset = 0; reload = true; }
            KeyCode::Char('n') if records.len() == args.selection.limit as usize => { args.selection.offset = args.selection.offset.saturating_add(args.selection.limit); reload = true; }
            KeyCode::Char('p') => { args.selection.offset = args.selection.offset.saturating_sub(args.selection.limit); reload = true; }
            _ => {}
        }
        if reload { records = args.records(store)?; selected = 0; message.clear(); }
    }
}

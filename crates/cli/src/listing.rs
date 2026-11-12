//! Grouped output and an on-demand terminal selector; never used for JSON/pipes.
use crate::{
    plan_for, transfer,
    ui::{self, cell, Ui},
    Selection, Settings,
};
use agenthub_core::{Agent, ResumeCommand};
use agenthub_index::{GroupBy, ListOrder, SessionRecord, SortKey, Store};
use anyhow::{bail, Result};
use clap::{Args, ValueEnum};
use crossterm::{
    cursor,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute, queue,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{self, IsTerminal, Write};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Sort {
    #[default]
    #[value(alias = "updated", alias = "updated-at")]
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
    #[arg(long, value_enum, default_value = "agent")]
    pub group_by: Group,
    /// Order within each group. Time/created: newest first; agent/project: A-Z.
    #[arg(long, value_enum, default_value = "time")]
    pub sort: Sort,
    /// Reverse the selected sort direction (groups remain A-Z).
    #[arg(long)]
    pub reverse: bool,
    /// Print the list without selecting or opening a native agent.
    #[arg(long, conflicts_with = "interactive")]
    pub no_interactive: bool,
    /// Require a terminal selector (fails if stdin/stdout are redirected).
    #[arg(long, conflicts_with = "no_interactive")]
    pub interactive: bool,
}
impl ListArgs {
    fn order(&self) -> ListOrder {
        ListOrder {
            group: match self.group_by {
                Group::None => GroupBy::None,
                Group::Agent => GroupBy::Agent,
                Group::Project => GroupBy::Project,
            },
            sort: match self.sort {
                Sort::Time => SortKey::Updated,
                Sort::Created => SortKey::Created,
                Sort::Agent => SortKey::Agent,
                Sort::Project => SortKey::Project,
            },
            reverse: self.reverse,
        }
    }
    pub fn records(&self, store: &Store) -> Result<Vec<SessionRecord>> {
        store.list_ordered(&self.selection.filter()?, self.order())
    }
    pub fn use_terminal(&self, json: bool) -> Result<bool> {
        let capable = io::stdin().is_terminal()
            && io::stdout().is_terminal()
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
        Group::Project => record
            .session
            .project_path
            .as_deref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "(no project)".into()),
    }
}

pub fn print(
    out: &mut impl Write,
    records: &[SessionRecord],
    args: &ListArgs,
    ui: &Ui,
) -> Result<()> {
    writeln!(
        out,
        "{} · {} shown · group: {:?} · sort: {:?}{}\n",
        ui.heading("AgentHub · Sessions"),
        records.len(),
        args.group_by,
        args.sort,
        if args.reverse { " (reverse)" } else { "" }
    )?;
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
        writeln!(
            out,
            "  {} · {} records · {}",
            ui::time(s.updated_at),
            record.message_count,
            if record.available {
                "source available"
            } else {
                "cached only"
            }
        )?;
        writeln!(
            out,
            "  Project: {}\n",
            cell(
                &s.project_path
                    .as_deref()
                    .map(|p| p.to_string_lossy())
                    .unwrap_or_default()
            )
        )?;
    }
    if !records.is_empty() {
        writeln!(
            out,
            "View: agenthub show <agent:id>\nResume: agenthub resume <agent:id> --exec"
        )?;
        if records.len() == args.selection.limit as usize {
            writeln!(
                out,
                "Next page (keep your filters): --offset {} --limit {}",
                u64::from(args.selection.offset) + u64::from(args.selection.limit),
                args.selection.limit
            )?;
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
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            cursor::Hide
        )?;
        Ok(guard)
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            cursor::Show,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
    }
}

fn clip(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut used = 0;
    let mut out = String::new();
    // Sanitize and clip while streaming, rather than copying an arbitrarily
    // long native title first. Bound zero-width/control-character runs too.
    for c in text.chars().take(width.saturating_mul(8).saturating_add(8)) {
        let c = if matches!(c, '\n' | '\t') {
            ' '
        } else if c.is_control() {
            continue;
        } else {
            c
        };
        used += c.width().unwrap_or(0);
        if used > width {
            break;
        }
        out.push(c);
    }
    out
}

fn padded(text: &str, width: usize) -> String {
    let clipped = clip(text, width);
    let used: usize = clipped.chars().map(|c| c.width().unwrap_or(0)).sum();
    format!("{clipped}{}", " ".repeat(width.saturating_sub(used)))
}
fn at(out: &mut impl Write, row: u16, text: &str) -> Result<()> {
    queue!(out, cursor::MoveTo(0, row), crossterm::style::Print(text))?;
    Ok(())
}

struct Page {
    records: Vec<SessionRecord>,
    counts: std::collections::BTreeMap<String, i64>,
}
impl Page {
    fn load(store: &Store, args: &ListArgs, query: &str) -> Result<Self> {
        let mut filter = args.selection.filter()?;
        let records = store.list_matching(&filter, args.order(), Some(query))?;
        // Counts belong to the full query, not the current page or selected tab.
        filter.agent = None;
        let counts = store.session_counts(&filter, Some(query))?;
        Ok(Self { records, counts })
    }
    fn total(&self, agent: Option<Agent>) -> i64 {
        agent
            .map(|a| self.counts.get(a.as_str()).copied().unwrap_or(0))
            .unwrap_or_else(|| self.counts.values().sum())
    }
}

const AGENTS: [Option<Agent>; 6] = [
    None,
    Some(Agent::Claude),
    Some(Agent::Codex),
    Some(Agent::Opencode),
    Some(Agent::Pi),
    Some(Agent::Agy),
];
const LABELS: [&str; 6] = ["All", "Claude", "Codex", "OpenCode", "Pi", "AGY"];

fn agent_bar(
    page: &Page,
    selected: Option<Agent>,
    range: std::ops::Range<usize>,
    width: usize,
    ui: &Ui,
) -> String {
    // All categories remain visible even with no indexed sessions. Each slot gets
    // a bounded display width, rather than letting an earlier category clip others.
    let slot = width / range.len();
    range
        .map(|i| {
            let text = padded(
                &format!("{} {}:{}", i, LABELS[i], page.total(AGENTS[i])),
                slot,
            );
            if AGENTS[i] == selected {
                ui.selected(&text)
            } else {
                ui.heading(&text)
            }
        })
        .collect()
}

#[derive(Clone)]
enum Row {
    Group(String),
    Session(usize),
}
fn rows(records: &[SessionRecord], group: Group) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut previous = None;
    for (i, record) in records.iter().enumerate() {
        let name = group_name(record, group);
        if group != Group::None && previous.as_ref() != Some(&name) {
            rows.push(Row::Group(name.clone()));
            previous = Some(name);
        }
        rows.push(Row::Session(i));
    }
    rows
}

fn columns(width: usize) -> (usize, usize, usize) {
    if width >= 90 {
        (16, 9, 20)
    } else if width >= 65 {
        (11, 9, 12)
    } else if width >= 45 {
        (5, 8, 8)
    } else {
        (0, 6, 0)
    }
}
fn prefix(mark: &str, date: &str, agent: &str, project: &str, width: usize) -> String {
    let (d, a, p) = columns(width);
    let mut text = format!("{mark} ");
    for (value, len) in [(date, d), (agent, a), (project, p)] {
        if len > 0 {
            text.push_str(&padded(value, len));
            text.push(' ');
        }
    }
    text
}
fn session_line(record: &SessionRecord, selected: bool, width: usize, ui: &Ui) -> String {
    let s = &record.session;
    let date = s
        .updated_at
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map(|v| {
            v.format(if width >= 90 {
                "%Y-%m-%d %H:%M"
            } else if width >= 65 {
                "%m-%d %H:%M"
            } else {
                "%m-%d"
            })
            .to_string()
        })
        .unwrap_or_else(|| "?".into());
    let project = s
        .project_path
        .as_deref()
        .map(|p| p.file_name().unwrap_or(p.as_os_str()).to_string_lossy())
        .unwrap_or_else(|| "(none)".into());
    let lead = prefix(
        if selected { ">" } else { " " },
        &date,
        s.agent.as_str(),
        &project,
        width,
    );
    let used: usize = lead.chars().map(|c| c.width().unwrap_or(0)).sum();
    let remaining = width.saturating_sub(used);
    let title = if record.available {
        padded(&s.title, remaining)
    } else {
        padded(
            &format!("[cached] {}", clip(&s.title, remaining.saturating_sub(9))),
            remaining,
        )
    };
    if selected {
        ui.selected(&padded(&format!("{lead}{title}"), width))
    } else {
        format!("{}{}", ui.dim(&lead), ui.title(&title))
    }
}

enum Handoff {
    Target,
    Preview(agenthub_transfer::Plan),
}

pub fn choose(
    store: &mut Store,
    settings: &Settings,
    mut args: ListArgs,
    ui: &Ui,
) -> Result<Option<ResumeCommand>> {
    let _screen = Screen::enter()?;
    let mut handoff: Option<Handoff> = None;
    let mut query = String::new();
    let mut draft: Option<String> = None;
    let mut page = Page::load(store, &args, &query)?;
    let mut selected = 0usize;
    let mut top = 0usize;
    let mut message = String::new();
    loop {
        let (width, height) = terminal::size()?;
        let width = width.saturating_sub(1) as usize;
        if height < 10 || width < 35 {
            let mut out = io::stdout().lock();
            execute!(out, cursor::MoveTo(0, 0), Clear(ClearType::All))?;
            at(
                &mut out,
                0,
                &clip("Terminal too small; resize or q/Esc to quit", width),
            )?;
            out.flush()?;
            drop(out);
            match event::read()? {
                Event::Key(k)
                    if matches!(k.code, KeyCode::Esc | KeyCode::Char('q'))
                        || (k.code == KeyCode::Char('c')
                            && k.modifiers.contains(KeyModifiers::CONTROL)) =>
                {
                    return Ok(None)
                }
                _ => continue,
            }
        }
        let visible = height.saturating_sub(9) as usize;
        let rows = rows(&page.records, args.group_by);
        let position = rows
            .iter()
            .position(|row| matches!(row, Row::Session(i) if *i == selected))
            .unwrap_or(0);
        if position < top {
            top = position;
        }
        if position >= top + visible {
            top = position + 1 - visible;
        }
        let mut out = io::stdout().lock();
        execute!(out, cursor::MoveTo(0, 0), Clear(ClearType::All))?;
        at(
            &mut out,
            0,
            &ui.bar(&padded(" AgentHub  / Sessions", width)),
        )?;
        at(
            &mut out,
            1,
            &agent_bar(&page, args.selection.agent, 0..3, width, ui),
        )?;
        at(
            &mut out,
            2,
            &agent_bar(&page, args.selection.agent, 3..6, width, ui),
        )?;
        at(
            &mut out,
            3,
            &ui.dim(&padded(
                &format!(
                    "Group: {:?} [g]  Sort: {:?}{} [s/r]  agent={} [0-5/Tab/Left/Right]",
                    args.group_by,
                    args.sort,
                    if args.reverse { " reversed" } else { "" },
                    args.selection
                        .agent
                        .map(|a| a.to_string())
                        .unwrap_or_else(|| "all".into())
                ),
                width,
            )),
        )?;
        let heading = prefix(" ", "UPDATED (UTC)", "AGENT", "PROJECT", width) + "TITLE";
        at(&mut out, 4, &ui.bar(&padded(&heading, width)))?;
        for (i, row) in rows.iter().skip(top).take(visible).enumerate() {
            let text = match row {
                Row::Group(name) => ui.heading(&padded(&format!("── {name} ──"), width)),
                Row::Session(index) => {
                    session_line(&page.records[*index], *index == selected, width, ui)
                }
            };
            at(&mut out, 5 + i as u16, &text)?;
        }
        if page.records.is_empty() {
            at(
                &mut out,
                5,
                &ui.title(&padded(
                    "No matching indexed sessions. Clear search/change Agent, or run ah sync.",
                    width,
                )),
            )?;
        }
        if let Some(state) = &handoff {
            for row in 5..height - 4 {
                at(&mut out, row, &padded("", width))?;
            }
            let details = match state {
                Handoff::Target => vec![
                    "SESSION TRANSFER · new native session".into(),
                    "1 Claude   2 Codex   3 OpenCode   4 Pi".into(),
                    "5 AGY: no verified importer (source-only)".into(),
                    "Copies active-branch user/assistant text; source unchanged.".into(),
                    "Esc/q cancels. No model is called.".into(),
                ],
                Handoff::Preview(plan) => vec![
                    format!("TRANSFER {} -> {}", plan.source, plan.target),
                    format!(
                        "Messages: {}  Excluded records/messages: {}",
                        plan.turn_count, plan.excluded_records
                    ),
                    format!("Project: {}", plan.project.display()),
                    format!("Destination: {}", plan.target_root.display()),
                    "Text only. Tools/results, reasoning, attachments and permissions excluded."
                        .into(),
                    "Creates a new ID. Source and project files unchanged. No model call.".into(),
                    "y: confirm transfer    Esc/q: cancel".into(),
                ],
            };
            for (i, line) in details.iter().take(visible).enumerate() {
                at(&mut out, 5 + i as u16, &ui.title(&padded(line, width)))?;
            }
        }
        let detail = page
            .records
            .get(selected)
            .map(|r| {
                format!(
                    "{} | {} records | {}",
                    r.session.key,
                    r.message_count,
                    r.session
                        .project_path
                        .as_deref()
                        .map(|p| p.to_string_lossy())
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        at(&mut out, height - 4, &ui.dim(&padded(&detail, width)))?;
        at(
            &mut out,
            height - 3,
            &ui.bar(&padded(
                &format!(
                    " {}/{} loaded | {} matches | offset {} | Search: /{}",
                    if page.records.is_empty() {
                        0
                    } else {
                        selected + 1
                    },
                    page.records.len(),
                    page.total(args.selection.agent),
                    args.selection.offset,
                    draft.as_ref().unwrap_or(&query)
                ),
                width,
            )),
        )?;
        at(
            &mut out,
            height - 2,
            &ui.dim(&padded(
                "Up/Down j/k select | Enter: resume | / search | t transfer | n/p page | q quit",
                width,
            )),
        )?;
        let prompt = if handoff.is_some() {
            Some("Transfer: 1-4 choose target / y confirm preview / Esc cancel".into())
        } else {
            draft
                .as_ref()
                .map(|d| format!("/{d}  [Enter apply, Esc cancel, Ctrl-U clear]"))
        };
        at(
            &mut out,
            height - 1,
            &if !message.is_empty() {
                ui.error(&padded(&message, width))
            } else if let Some(prompt) = prompt {
                ui.title(&padded(&prompt, width))
            } else {
                ui.dim(&padded(
                    "0 All  1 Claude  2 Codex  3 OpenCode  4 Pi  5 AGY",
                    width,
                ))
            },
        )?;
        if let Some(input) = &draft {
            let column = input.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>() + 1;
            queue!(
                out,
                cursor::MoveTo(column.min(width.saturating_sub(1)) as u16, height - 1),
                cursor::Show
            )?;
        } else {
            queue!(out, cursor::Hide)?;
        }
        out.flush()?;
        drop(out);

        let event = event::read()?;
        if let Event::Paste(text) = &event {
            if let Some(input) = &mut draft {
                input.push_str(&cell(text));
                *input = input.chars().take(2048).collect();
                args.selection.offset = 0;
                page = Page::load(store, &args, input)?;
                selected = 0;
                top = 0;
            }
            continue;
        }
        let Event::Key(key) = event else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(None);
        }
        if let Some(state) = handoff.take() {
            match (state, key.code) {
                (_, KeyCode::Esc | KeyCode::Char('q')) => {
                    message.clear();
                }
                (Handoff::Target, KeyCode::Char(c @ '1'..='5')) => {
                    if let Some(record) = page.records.get(selected) {
                        match transfer::prepare(
                            store,
                            settings,
                            &record.session.key,
                            AGENTS[(c as u8 - b'0') as usize].expect("target digit"),
                            None,
                            None,
                        ) {
                            Ok(plan) => {
                                handoff = Some(Handoff::Preview(plan));
                                message.clear();
                            }
                            Err(e) => {
                                handoff = Some(Handoff::Target);
                                message = format!("Cannot transfer: {e:#}");
                            }
                        }
                    }
                }
                (Handoff::Preview(plan), KeyCode::Char('y')) if height >= 16 && width >= 59 => {
                    let mut progress = io::stdout().lock();
                    at(
                        &mut progress,
                        height - 1,
                        &ui.title(&padded("Transferring to a new native session...", width)),
                    )?;
                    progress.flush()?;
                    drop(progress);
                    match transfer::write(store, settings, &plan) {
                        Ok(receipt) => {
                            args.selection.agent = Some(plan.target);
                            args.selection.offset = 0;
                            query.clear();
                            page = Page::load(store, &args, &query)?;
                            selected = page
                                .records
                                .iter()
                                .position(|r| r.session.key == receipt.key)
                                .unwrap_or(0);
                            top = 0;
                            message = format!("Created {}. Source unchanged. Run ah sync if index needs refreshing.",receipt.key);
                        }
                        Err(e) => message = format!("Transfer failed: {e:#}"),
                    }
                }
                (state, _) => {
                    handoff = Some(state);
                    if height < 16 || width < 59 {
                        message =
                            "Resize terminal to at least 60x16 to review and confirm transfer."
                                .into();
                    }
                }
            }
            continue;
        }
        if draft.is_some() {
            // Drain a burst of typed characters before querying. Bracketed paste
            // already arrives as one event. No historical-result cache is kept.
            let mut input_event = Event::Key(key);
            for consumed in 0..64 {
                match input_event {
                    Event::Key(k) if k.kind != KeyEventKind::Release => {
                        if k.code == KeyCode::Char('c')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            return Ok(None);
                        }
                        match k.code {
                            KeyCode::Esc => {
                                draft = None;
                                break;
                            }
                            KeyCode::Enter => {
                                query = draft.take().unwrap_or_default();
                                break;
                            }
                            KeyCode::Backspace => {
                                draft.as_mut().expect("search input").pop();
                            }
                            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                                draft.as_mut().expect("search input").clear()
                            }
                            KeyCode::Char(c)
                                if !k
                                    .modifiers
                                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                            {
                                let input = draft.as_mut().expect("search input");
                                if input.len() < 8192 {
                                    input.push(c);
                                }
                            }
                            _ => {}
                        }
                    }
                    Event::Paste(text) => {
                        let input = draft.as_mut().expect("search input");
                        input.push_str(&cell(&text));
                        *input = input.chars().take(2048).collect();
                    }
                    _ => {}
                }
                if consumed == 63 || !event::poll(std::time::Duration::ZERO)? {
                    break;
                }
                input_event = event::read()?;
            }
            args.selection.offset = 0;
            page = Page::load(store, &args, draft.as_ref().unwrap_or(&query))?;
            selected = 0;
            top = 0;
            message.clear();
            continue;
        }
        let mut reload = false;
        match key.code {
            KeyCode::Esc if !query.is_empty() => {
                query.clear();
                args.selection.offset = 0;
                reload = true;
            }
            KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
            KeyCode::Char('/') => {
                draft = Some(query.clone());
                message.clear();
            }
            KeyCode::Char('t') if !page.records.is_empty() => {
                handoff = Some(Handoff::Target);
                message.clear();
            }
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                selected = (selected + 1).min(page.records.len().saturating_sub(1))
            }
            KeyCode::Home => selected = 0,
            KeyCode::End => selected = page.records.len().saturating_sub(1),
            KeyCode::PageUp => selected = selected.saturating_sub(visible),
            KeyCode::PageDown => {
                selected = (selected + visible).min(page.records.len().saturating_sub(1))
            }
            KeyCode::Enter => {
                if let Some(record) = page.records.get(selected) {
                    match plan_for(store, record) {
                        Ok(plan) => return Ok(Some(plan)),
                        Err(e) => message = format!("Cannot resume: {e:#}"),
                    }
                }
            }
            KeyCode::Char(c @ '0'..='5') => {
                args.selection.agent = AGENTS[(c as u8 - b'0') as usize];
                args.selection.offset = 0;
                reload = true;
            }
            KeyCode::Tab | KeyCode::Right | KeyCode::Left | KeyCode::BackTab => {
                let index = AGENTS
                    .iter()
                    .position(|a| *a == args.selection.agent)
                    .unwrap_or(0);
                let next = if matches!(key.code, KeyCode::Left | KeyCode::BackTab) {
                    (index + 5) % 6
                } else {
                    (index + 1) % 6
                };
                args.selection.agent = AGENTS[next];
                args.selection.offset = 0;
                reload = true;
            }
            KeyCode::Char('g') => {
                args.group_by = match args.group_by {
                    Group::None => Group::Agent,
                    Group::Agent => Group::Project,
                    Group::Project => Group::None,
                };
                args.selection.offset = 0;
                reload = true;
            }
            KeyCode::Char('s') => {
                args.sort = match args.sort {
                    Sort::Time => Sort::Created,
                    Sort::Created => Sort::Agent,
                    Sort::Agent => Sort::Project,
                    Sort::Project => Sort::Time,
                };
                args.selection.offset = 0;
                reload = true;
            }
            KeyCode::Char('r') => {
                args.reverse = !args.reverse;
                args.selection.offset = 0;
                reload = true;
            }
            KeyCode::Char('n')
                if u64::from(args.selection.offset) + (page.records.len() as u64)
                    < page.total(args.selection.agent) as u64 =>
            {
                args.selection.offset = args.selection.offset.saturating_add(args.selection.limit);
                reload = true;
            }
            KeyCode::Char('p') => {
                args.selection.offset = args.selection.offset.saturating_sub(args.selection.limit);
                reload = true;
            }
            _ => {}
        }
        if reload {
            page = Page::load(store, &args, &query)?;
            selected = 0;
            top = 0;
            message.clear();
        }
    }
}

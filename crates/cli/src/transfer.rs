//! CLI orchestration for previews/new-session creation; no writes during preview.
use crate::{absolute, display_plan, Settings};
use agenthub_adapters::adapter;
use agenthub_core::Agent;
use agenthub_index::Store;
use agenthub_transfer::{Collector, Plan, Receipt};
use anyhow::{Context, Result};
use clap::Args;
use std::{env, io::Write, path::PathBuf};

#[derive(Args)]
pub struct TransferArgs {
    /// Full agent:id or an unambiguous native session ID from the index.
    pub id: String,
    /// Destination agent. AGY is currently source-only.
    #[arg(long)]
    pub to: Agent,
    /// Preview counts, losses and destination without writing any native session.
    #[arg(long)]
    pub dry_run: bool,
    /// Transfer only the last N user/assistant messages of the active branch.
    #[arg(long, value_parser=clap::value_parser!(u32).range(1..))]
    pub last: Option<u32>,
    /// Existing project directory; defaults to the source project's path.
    #[arg(long)]
    pub cwd: Option<PathBuf>,
}

pub fn prepare(
    store: &Store,
    settings: &Settings,
    id: &str,
    to: Agent,
    cwd: Option<PathBuf>,
    last: Option<u32>,
) -> Result<Plan> {
    let record = store.find(id)?;
    let mut collector = Collector::new(record.session.agent);
    store.visit_messages(&record.session.key, 0, None, |m| collector.push(m))?;
    let mut root = settings.root(to)?;
    if to == Agent::Opencode
        && root.is_file()
        && root.file_name().is_some_and(|n| n == "opencode.db")
    {
        root = root
            .parent()
            .context("OpenCode database has no parent")?
            .into();
    }
    let cwd = cwd
        .map(|p| {
            let home = env::var_os("HOME")
                .map(PathBuf::from)
                .context("HOME must be set")?;
            absolute(&p, &home)
        })
        .transpose()?;
    let mut plan = Plan::new(
        &record.session,
        to,
        root,
        cwd,
        collector,
        last.map(|n| n as usize),
    )?;
    if !record.available {
        plan.warnings
            .push("Source is unavailable; using its last successfully indexed transcript.".into());
    }
    Ok(plan)
}

pub fn write(store: &mut Store, settings: &Settings, plan: &Plan) -> Result<Receipt> {
    let mut receipt = plan.write()?;
    let target = adapter(plan.target, settings.root(plan.target)?);
    match store.sync(target.as_ref(), false) {
        Ok(report) => {
            if !report.errors.is_empty() { receipt.warnings.push("New native session was created, but index refresh had errors; run ah sync and inspect the target agent before retrying transfer.".into()); }
            receipt.warnings.extend(report.warnings);
        }
        Err(_) => receipt.warnings.push("New native session was created, but index refresh failed; run ah sync. Do not repeat transfer merely to refresh the index.".into()),
    }
    if store.find(&receipt.key).is_err() {
        receipt.warnings.push("Created session is not visible in the index yet; run ah sync and inspect the target before creating another copy.".into());
    }
    Ok(receipt)
}

pub fn preview(out: &mut impl Write, plan: &Plan) -> Result<()> {
    writeln!(out, "Session transfer preview\n  Source: {}\n  Target: {}\n  Project: {}\n  Destination root: {}\n  User/assistant messages: {}\n  Excluded records/messages: {}",
        crate::cell(&plan.source), plan.target, crate::cell(&plan.project.to_string_lossy()), crate::cell(&plan.target_root.to_string_lossy()), plan.turn_count, plan.excluded_records)?;
    for warning in &plan.warnings {
        writeln!(out, "  Note: {}", crate::cell(warning))?;
    }
    writeln!(out,"Source stays unchanged. Transfer creates a new native session ID; it does not launch a model.")?;
    Ok(())
}

pub fn receipt(out: &mut impl Write, result: &Receipt) -> Result<()> {
    writeln!(
        out,
        "Created {}\nFile: {}\nResume: {}",
        crate::cell(&result.key),
        crate::cell(&result.path.to_string_lossy()),
        display_plan(&result.resume)
    )?;
    for warning in &result.warnings {
        writeln!(out, "Note: {}", crate::cell(warning))?;
    }
    Ok(())
}

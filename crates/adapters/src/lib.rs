//! Built-in, statically registered adapters. Agent stores are opened as files for reading only.
mod agy;
mod dsh;
mod jsonl;
mod opencode;
mod protobuf;
mod sqlite;

pub use agy::AgyAdapter;
pub use dsh::DshAdapter;
pub use jsonl::JsonlAdapter;
pub use opencode::OpenCodeAdapter;

use agenthub_core::{Adapter, Agent, Discovery, Source};
use anyhow::{Context, Result};
use std::{env, path::{Path, PathBuf}};

pub fn default_root(agent: Agent, home: &Path) -> PathBuf {
    let env_path = |name: &str| env::var_os(name).filter(|s| !s.to_string_lossy().trim().is_empty()).map(PathBuf::from);
    match agent {
        Agent::Claude => env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")).join("projects"),
        Agent::Codex => env_path("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
        Agent::Opencode => env_path("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")).join("opencode"),
        Agent::Pi => env_path("PI_CODING_AGENT_SESSION_DIR").unwrap_or_else(|| {
            env_path("PI_CODING_AGENT_DIR").unwrap_or_else(|| home.join(".pi/agent")).join("sessions")
        }),
        Agent::Agy => home.join(".gemini/antigravity-cli"),
        Agent::Dsh => env_path("DSH_HOME").unwrap_or_else(|| home.join(".dsh")).join("sessions"),
    }
}

pub fn adapter(agent: Agent, root: PathBuf) -> Box<dyn Adapter> {
    match agent {
        Agent::Agy => Box::new(AgyAdapter::new(root)),
        Agent::Dsh => Box::new(DshAdapter::new(root)),
        Agent::Opencode => Box::new(OpenCodeAdapter::new(root)),
        _ => Box::new(JsonlAdapter::new(agent, root)),
    }
}

fn discover_files(agent: Agent, root: &Path, extension: &str, max_depth: usize) -> Result<Discovery> {
    let mut result = Discovery { complete: true, ..Discovery::default() };
    if !root.try_exists().with_context(|| format!("access {}", root.display()))? {
        return Ok(result);
    }
    for entry in walkdir::WalkDir::new(root).follow_links(false).max_depth(max_depth) {
        match entry {
            Ok(entry) if entry.file_type().is_file() && entry.path().extension().is_some_and(|s| s == extension) => {
                result.sources.push(Source::new(agent, entry.into_path()));
            }
            Ok(_) => {}
            Err(error) => { result.complete = false; result.warnings.push(error.to_string()); }
        }
    }
    result.sources.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

fn string(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(serde_json::Value::as_str).map(str::to_owned)
}

/// Extract readable content while retaining every unknown block in the raw JSON.
fn content(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(content).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n"),
        Value::Object(_) => {
            for key in ["text", "thinking", "content", "output"] {
                if let Some(v) = value.get(key) { return content(v); }
            }
            value.to_string()
        }
        _ => value.to_string(),
    }
}


//! Human output only. JSON and JSON Lines never pass through this renderer.
use chrono::{DateTime, Utc};
use clap::ValueEnum;
use std::{env, io::IsTerminal};

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum Color {
    #[default]
    Auto,
    Always,
    Never,
}

pub struct Ui { color: bool }
impl Ui {
    pub fn new(color: Color, json: bool) -> Self {
        Self { color: !json && match color {
            Color::Always => true,
            Color::Never => false,
            Color::Auto => std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none() && env::var_os("TERM").is_none_or(|v| v != "dumb"),
        } }
    }
    pub fn heading(&self, value: &str) -> String {
        let value = cell(value);
        if self.color { format!("\x1b[1;36m{value}\x1b[0m") } else { value }
    }
    fn paint(&self, value: &str, code: &str) -> String {
        let value = cell(value);
        if self.color { format!("\x1b[{code}m{value}\x1b[0m") } else { value }
    }
    pub fn bar(&self, value: &str) -> String { self.paint(value, "1;4") }
    pub fn selected(&self, value: &str) -> String { self.paint(value, "1;7") }
    pub fn title(&self, value: &str) -> String { self.paint(value, "1") }
    pub fn dim(&self, value: &str) -> String { self.paint(value, "90") }
    pub fn metric(&self, value: &str) -> String { self.paint(value, "32") }
    pub fn error(&self, value: &str) -> String { self.paint(value, "1;91") }
}

pub fn safe(text: &str) -> String {
    // Preserve content, but never execute a transcript's terminal control sequences.
    text.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect()
}
pub fn cell(text: &str) -> String { safe(text).replace(['\n', '\t'], " ") }
pub fn time(value: Option<i64>) -> String {
    value.and_then(DateTime::<Utc>::from_timestamp_millis)
        .map(|v| v.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "unknown time".into())
}

/// Logical native-record bytes; stable across JSONL and shared database stores.
pub fn size(bytes: i64) -> String {
    let bytes = bytes.max(0) as f64;
    if bytes >= 1024.0 * 1024.0 * 1024.0 { format!("{:.1} GiB", bytes / (1024.0 * 1024.0 * 1024.0)) }
    else if bytes >= 1024.0 * 1024.0 { format!("{:.1} MiB", bytes / (1024.0 * 1024.0)) }
    else if bytes >= 1024.0 { format!("{:.1} KiB", bytes / 1024.0) }
    else { format!("{bytes:.0} B") }
}

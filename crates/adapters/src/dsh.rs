use crate::{content, string};
use agenthub_core::{timestamp, Adapter, Agent, Discovery, Message, Session, SessionSink, Source};
use anyhow::{bail, Context, Result};
use ruzstd::decoding::StreamingDecoder;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, Cursor, Read},
    path::{Path, PathBuf},
};

const MAX_RECORD: u64 = 32 * 1024 * 1024;
const ZSTD_MAGIC: u32 = 0xFD2F_B528;
const MAX_KNOWN_VERSION: u64 = 4;

pub struct DshAdapter {
    root: PathBuf,
}

impl DshAdapter {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Generation {
    version: u32,
    compressed: bool,
}

fn parse_generation_filename(name: &str) -> Option<Generation> {
    let (stem, compressed) = if let Some(s) = name.strip_suffix(".jsonl.zstd") {
        (s, true)
    } else {
        (name.strip_suffix(".jsonl")?, false)
    };
    if stem == "session" {
        return Some(Generation { version: 0, compressed });
    }
    let ver = stem.strip_prefix("session.v")?;
    if ver.is_empty() || ver.starts_with('0') || !ver.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Generation { version: ver.parse().ok()?, compressed })
}

#[derive(Debug, Clone, Copy)]
struct FrameRange {
    start: usize,
    end: usize,
}

fn scan_zstd_frames(buffer: &[u8]) -> Result<(Vec<FrameRange>, Option<usize>)> {
    let mut frames = Vec::new();
    let mut offset = 0;
    while offset < buffer.len() {
        let start = offset;
        if buffer.len() - offset < 4 {
            return Ok((frames, Some(start)));
        }
        let magic = u32::from_le_bytes(buffer[offset..offset + 4].try_into().unwrap());
        if magic != ZSTD_MAGIC {
            bail!("corrupt Zstandard session log: invalid frame magic at byte {offset}");
        }
        offset += 4;
        if offset == buffer.len() {
            return Ok((frames, Some(start)));
        }
        let descriptor = buffer[offset];
        offset += 1;
        if (descriptor & 0x18) != 0 {
            bail!("corrupt Zstandard session log: reserved frame-header bit at byte {}", offset - 1);
        }
        let fcs_flag = descriptor >> 6;
        let single_segment = (descriptor & 0x20) != 0;
        let checksum = (descriptor & 0x04) != 0;
        let dict_flag = descriptor & 0x03;
        let dict_bytes = if dict_flag == 3 { 4 } else { usize::from(dict_flag) };
        let fcs_bytes = if fcs_flag == 0 {
            usize::from(single_segment)
        } else {
            1usize << fcs_flag
        };
        let rem_header = usize::from(!single_segment) + dict_bytes + fcs_bytes;
        if buffer.len() - offset < rem_header {
            return Ok((frames, Some(start)));
        }
        offset += rem_header;
        loop {
            if buffer.len() - offset < 3 {
                return Ok((frames, Some(start)));
            }
            let bh = u32::from(buffer[offset])
                | (u32::from(buffer[offset + 1]) << 8)
                | (u32::from(buffer[offset + 2]) << 16);
            offset += 3;
            let last_block = (bh & 1) != 0;
            let block_type = (bh >> 1) & 3;
            let block_size = (bh >> 3) as usize;
            if block_type == 3 {
                bail!("corrupt Zstandard session log: reserved block type at byte {}", offset - 3);
            }
            let payload_bytes = if block_type == 1 { 1 } else { block_size };
            if buffer.len() - offset < payload_bytes {
                return Ok((frames, Some(start)));
            }
            offset += payload_bytes;
            if last_block {
                break;
            }
        }
        if checksum {
            if buffer.len() - offset < 4 {
                return Ok((frames, Some(start)));
            }
            offset += 4;
        }
        frames.push(FrameRange { start, end: offset });
    }
    Ok((frames, None))
}

fn decode_zstd_log(path: &Path) -> Result<(Vec<u8>, Option<String>)> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let (frames, torn_start) = scan_zstd_frames(&bytes).with_context(|| format!("scan {}", path.display()))?;
    if frames.is_empty() {
        bail!("{}: empty or incomplete Zstandard session header", path.display());
    }
    let mut out = Vec::new();
    for (idx, frame) in frames.iter().enumerate() {
        let slice = &bytes[frame.start..frame.end];
        let mut decoder = StreamingDecoder::new(slice)
            .map_err(|e| anyhow::anyhow!("{}: frame {idx} at byte {}: {e}", path.display(), frame.start))?;
        let before = out.len();
        decoder
            .read_to_end(&mut out)
            .with_context(|| format!("{}: decompress frame {idx} at byte {}", path.display(), frame.start))?;
        if idx == 0 {
            let header = &out[before..];
            if header.is_empty() || !header.ends_with(b"\n") || header[..header.len() - 1].contains(&b'\n') {
                bail!("{}: first Zstandard frame is not a single newline-terminated header record", path.display());
            }
        }
    }
    let warning = torn_start.map(|pos| format!("incomplete final Zstandard frame at byte {pos}; retry after append"));
    Ok((out, warning))
}

fn message_text(value: &Value) -> String {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if item["type"].as_str() == Some("tool-call") {
                    let name = item["name"].as_str().unwrap_or("");
                    let args = item["arguments"].as_str().unwrap_or("");
                    format!("{name} {args}").trim().to_owned()
                } else {
                    content(item)
                }
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => content(value),
    }
}

fn seq_id(value: &Value) -> Option<String> {
    value["seq"].as_i64().map(|n| n.to_string()).or_else(|| string(value, "id"))
}

impl Adapter for DshAdapter {
    fn agent(&self) -> Agent {
        Agent::Dsh
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn discover(&self) -> Result<Discovery> {
        let mut result = Discovery { complete: true, ..Discovery::default() };
        if !self.root.try_exists().with_context(|| format!("access {}", self.root.display()))? {
            return Ok(result);
        }
        let mut by_dir: BTreeMap<PathBuf, Vec<(Generation, PathBuf)>> = BTreeMap::new();
        for entry in walkdir::WalkDir::new(&self.root).follow_links(false) {
            match entry {
                Ok(entry) if entry.file_type().is_file() => {
                    let Some(name) = entry.file_name().to_str() else { continue };
                    let Some(gen) = parse_generation_filename(name) else { continue };
                    let path = entry.into_path();
                    let dir = path.parent().unwrap_or(&self.root).to_path_buf();
                    by_dir.entry(dir).or_default().push((gen, path));
                }
                Ok(_) => {}
                Err(error) => {
                    result.complete = false;
                    result.warnings.push(error.to_string());
                }
            }
        }
        for (_, mut candidates) in by_dir {
            candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            let (_, selected) = candidates.last().expect("non-empty candidate group").clone();
            let mut dependencies: Vec<PathBuf> = candidates.into_iter().map(|(_, p)| p).collect();
            dependencies.sort();
            result.sources.push(Source { agent: Agent::Dsh, path: selected, dependencies });
        }
        result.sources.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(result)
    }

    fn read(&self, source: &Source, sink: &mut dyn SessionSink) -> Result<()> {
        let is_zstd = source
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|name| name.ends_with(".jsonl.zstd"));
        let (mut input, zstd_warning): (Box<dyn BufRead>, Option<String>) = if is_zstd {
            let (decoded, warn) = decode_zstd_log(&source.path)?;
            (Box::new(BufReader::new(Cursor::new(decoded))), warn)
        } else {
            (Box::new(BufReader::new(File::open(&source.path)?)), None)
        };

        let mut session: Option<Session> = None;
        let mut line = Vec::new();
        let mut ordinal = 0_i64;
        let mut line_no = 0_usize;

        loop {
            line.clear();
            let n = input.by_ref().take(MAX_RECORD + 1).read_until(b'\n', &mut line)?;
            if n == 0 {
                break;
            }
            line_no += 1;
            if n as u64 > MAX_RECORD {
                bail!("{}:{line_no}: record exceeds 32 MiB", source.path.display());
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            if !line.ends_with(b"\n") && input.fill_buf()?.is_empty() {
                if let Some(s) = &mut session {
                    s.warnings.push(format!("incomplete final JSONL record at line {line_no}; retry after append"));
                    break;
                }
                bail!("incomplete session header");
            }
            let value: Value = serde_json::from_slice(&line)
                .with_context(|| format!("{}:{line_no}", source.path.display()))?;
            if !value.is_object() {
                bail!("line {line_no}: expected a JSON object");
            }
            let kind = value["type"].as_str().unwrap_or("unknown");
            if session.is_none() {
                if kind != "session" {
                    bail!("missing or unsupported dsh session header");
                }
                let id = string(&value, "id").context("missing dsh session id")?;
                let created_at = timestamp(&value["createdAt"]).context("missing dsh session createdAt")?;
                let version = value["version"].as_u64().context("missing dsh session version")?;
                let mut s = Session::new(Agent::Dsh, id, source.path.clone())?;
                s.project_path = string(&value, "cwd").map(PathBuf::from);
                s.parent_id = string(&value, "parentSession");
                s.observe_time(Some(created_at));
                if version > MAX_KNOWN_VERSION {
                    s.warnings.push(format!(
                        "DeepSeek Harness session version v{version} newer than v{MAX_KNOWN_VERSION}; raw events preserved"
                    ));
                }
                if let Some(w) = zstd_warning.clone() {
                    s.warnings.push(w);
                }
                sink.begin(&s)?;
                sink.message(&Message {
                    ordinal,
                    native_id: Some(s.native_id.clone()),
                    parent_id: s.parent_id.clone(),
                    role: "system".into(),
                    kind: "session".into(),
                    timestamp: Some(created_at),
                    text: content(&value),
                    raw: value,
                })?;
                ordinal += 1;
                session = Some(s);
                continue;
            }

            let s = session.as_mut().context("missing session")?;
            let time = timestamp(&value["time"]).or_else(|| timestamp(&value["timestamp"]));
            s.observe_time(time);
            let data = &value["data"];

            let (role, text, native_id) = match kind {
                "session/title" => {
                    if let Some(title) = string(data, "title") {
                        let cleaned: String = title.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
                        if !cleaned.is_empty() {
                            s.title = cleaned;
                        }
                    }
                    ("event".into(), string(data, "title").unwrap_or_else(|| data.to_string()), seq_id(&value))
                }
                "working-directory/change" => {
                    if s.project_path.is_none() {
                        s.project_path = string(data, "cwd").map(PathBuf::from);
                    }
                    ("event".into(), content(data), seq_id(&value))
                }
                "user/message" => {
                    let msg = if data["message"].is_object() { &data["message"] } else { data };
                    let text = message_text(msg.get("content").unwrap_or(msg));
                    let source_kind = msg["source"]["kind"].as_str();
                    let is_user_prompt = source_kind.is_none() || source_kind == Some("user");
                    if is_user_prompt {
                        s.suggest_title(&text);
                    }
                    let role = if is_user_prompt { "user" } else { "system" };
                    let id = string(msg, "id").or_else(|| seq_id(&value));
                    (role.into(), text, id)
                }
                "assistant/message" | "assistant/attempt" => {
                    let msg = if data["message"].is_object() { &data["message"] } else { data };
                    let text = message_text(msg.get("content").unwrap_or(msg));
                    let id = string(msg, "id").or_else(|| seq_id(&value));
                    ("assistant".into(), text, id)
                }
                "tool/call" => {
                    let name = data["name"].as_str().unwrap_or("");
                    let args = data["arguments"].as_str().unwrap_or("");
                    let text = if !name.is_empty() {
                        format!("{name} {args}").trim().to_owned()
                    } else {
                        data.to_string()
                    };
                    let id = string(data, "callId").or_else(|| seq_id(&value));
                    ("tool_call".into(), text, id)
                }
                "tool/result" => {
                    let msg = if data["message"].is_object() { &data["message"] } else { data };
                    let text = message_text(msg.get("content").unwrap_or(msg));
                    let id = string(msg, "id")
                        .or_else(|| string(msg, "toolCallId"))
                        .or_else(|| seq_id(&value));
                    ("tool".into(), text, id)
                }
                "system/message" | "developer/message" => {
                    let msg = if data["message"].is_object() { &data["message"] } else { data };
                    let role = if kind.starts_with("developer") { "developer" } else { "system" };
                    let text = message_text(msg.get("content").unwrap_or(msg));
                    let id = string(msg, "id").or_else(|| seq_id(&value));
                    (role.into(), text, id)
                }
                _ => ("event".into(), content(value.get("data").unwrap_or(&value)), seq_id(&value)),
            };

            let parent_id = value["sourceEventSeqs"]
                .as_array()
                .and_then(|xs| xs.first())
                .and_then(Value::as_i64)
                .map(|n| n.to_string())
                .or_else(|| string(&value, "parentId"));

            sink.message(&Message {
                ordinal,
                native_id,
                parent_id,
                role,
                kind: kind.into(),
                timestamp: time,
                text,
                raw: value,
            })?;
            ordinal += 1;
        }

        let mut s = session.context("empty session file")?;
        if s.title.is_empty() {
            s.title = s.native_id.clone();
        }
        sink.finish(&s)
    }
}

//! Small bounded protobuf wire reader for AGY's undocumented binary records.
//! It does not guess a vendor schema or scan arbitrary byte strings for prose.
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};

#[derive(Debug)]
pub(crate) enum Wire<'a> { Varint(u64), Bytes(&'a [u8]), Fixed(&'a [u8]) }

pub(crate) fn fields(data: &[u8]) -> Result<Vec<(u32, Wire<'_>)>> {
    if data.len() > 32 * 1024 * 1024 { bail!("protobuf record exceeds 32 MiB"); }
    let mut p = 0;
    let mut out = Vec::new();
    while p < data.len() {
        if out.len() >= 100_000 { bail!("too many protobuf fields"); }
        let tag = varint(data, &mut p)?;
        let number = u32::try_from(tag >> 3)?;
        if number == 0 || number >= 1 << 29 { bail!("invalid protobuf field number"); }
        let value = match tag & 7 {
            0 => Wire::Varint(varint(data, &mut p)?),
            1 => Wire::Fixed(take(data, &mut p, 8)?),
            2 => {
                let len = usize::try_from(varint(data, &mut p)?)?;
                Wire::Bytes(take(data, &mut p, len)?)
            }
            5 => Wire::Fixed(take(data, &mut p, 4)?),
            _ => bail!("unsupported protobuf wire type"),
        };
        out.push((number, value));
    }
    Ok(out)
}

fn varint(data: &[u8], p: &mut usize) -> Result<u64> {
    let mut out = 0;
    for shift in (0..70).step_by(7) {
        let b = *data.get(*p).context("truncated protobuf varint")?;
        *p += 1;
        if shift == 63 && b > 1 { bail!("protobuf varint overflow"); }
        out |= u64::from(b & 127) << shift;
        if b & 128 == 0 { return Ok(out); }
    }
    bail!("protobuf varint overflow")
}

fn take<'a>(data: &'a [u8], p: &mut usize, n: usize) -> Result<&'a [u8]> {
    let end = p.checked_add(n).context("protobuf length overflow")?;
    let bytes = data.get(*p..end).context("truncated protobuf payload")?;
    *p = end;
    Ok(bytes)
}

pub(crate) fn bytes_at<'a>(data: &'a [u8], path: &[u32]) -> Result<Option<&'a [u8]>> {
    if path.is_empty() { return Ok(Some(data)); }
    for (number, value) in fields(data)? {
        if number == path[0] {
            if let Wire::Bytes(b) = value { return bytes_at(b, &path[1..]); }
        }
    }
    Ok(None)
}

pub(crate) fn text_at(data: &[u8], path: &[u32]) -> Result<Option<String>> {
    bytes_at(data, path)?.map(|b| String::from_utf8(b.to_vec()).context("invalid UTF-8 in known AGY text field")).transpose()
}

pub(crate) fn integer_at(data: &[u8], path: &[u32]) -> Result<Option<u64>> {
    let Some((&last, prefix)) = path.split_last() else { return Ok(None) };
    let Some(bytes) = bytes_at(data, prefix)? else { return Ok(None) };
    Ok(fields(bytes)?.into_iter().find_map(|(n, v)| match v { Wire::Varint(i) if n == last => Some(i), _ => None }))
}

pub(crate) fn wire_json(data: &[u8], depth: usize) -> Value {
    if depth >= 10 { return json!({"base64": STANDARD.encode(data)}); }
    let Ok(values) = fields(data) else { return json!({"base64": STANDARD.encode(data)}); };
    Value::Array(values.into_iter().map(|(field, wire)| {
        let value = match wire {
            Wire::Varint(n) => json!({"varint": n}),
            Wire::Fixed(b) => json!({"fixed_base64": STANDARD.encode(b)}),
            Wire::Bytes(b) => match std::str::from_utf8(b) {
                Ok(s) if s.chars().all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t')) => json!({"text":s}),
                _ => wire_json(b, depth + 1),
            },
        };
        json!({"field":field,"value":value})
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_lengths_and_overflows() {
        assert!(fields(&[0]).is_err());
        assert!(fields(&[10, 12, 65]).is_err());
        assert!(fields(&[8, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 1]).is_err());
        assert_eq!(text_at(&[10, 2, 104, 105], &[1]).unwrap().as_deref(), Some("hi"));
    }
}


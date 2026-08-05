//! Provider-reported token counters read from harness transcripts.
//!
//! Mirrors the eval's `transcript_usage.py`: Codex transcripts carry
//! cumulative `token_count` payloads (only the last one matters, so large
//! files use a tail fast-path); Claude transcripts are summed per provider
//! message id, keeping the most complete usage row per id. Only usage
//! metadata is inspected — message content is never returned.

use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct TranscriptTokens {
    /// Logical input, cached reads included.
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub total: u64,
}

const TAIL_BYTES: u64 = 1_048_576;

fn integer(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0)
}

/// Read counters for a session transcript. `harness` comes from the session
/// store; unknown harnesses fall back to content sniffing.
pub fn read(path: &Path, harness: &str) -> Option<TranscriptTokens> {
    match harness.to_ascii_lowercase().as_str() {
        "codex" => codex_tokens(path),
        "claude" => claude_tokens(path),
        _ => codex_tokens(path).or_else(|| claude_tokens(path)),
    }
}

/// Codex counters are cumulative, so only the LAST `token_count` line
/// matters; for large transcripts read just the tail and fall back to a
/// full scan when the tail holds no counter.
fn codex_tokens(path: &Path) -> Option<TranscriptTokens> {
    let tail = read_tail(path, TAIL_BYTES)?;
    codex_from_lines(&tail).or_else(|| {
        let full = fs::read_to_string(path).ok()?;
        codex_from_lines(&full)
    })
}

fn codex_from_lines(text: &str) -> Option<TranscriptTokens> {
    let mut latest = None;
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let payload = value.get("payload").unwrap_or(&value);
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            continue;
        }
        if let Some(total) = payload.get("info").and_then(|i| i.get("total_token_usage")) {
            latest = Some(total.clone());
        }
    }
    let usage = latest?;
    let input = integer(usage.get("input_tokens"));
    let output = integer(usage.get("output_tokens"));
    let total = integer(usage.get("total_tokens"));
    Some(TranscriptTokens {
        input,
        cached: integer(usage.get("cached_input_tokens")),
        output,
        total: if total > 0 { total } else { input + output },
    })
}

/// Claude transcripts stream usage updates per provider message id; keep the
/// most complete row per id and sum.
fn claude_tokens(path: &Path) -> Option<TranscriptTokens> {
    let text = fs::read_to_string(path).ok()?;
    let mut best: HashMap<String, (u64, Value)> = HashMap::new();
    let mut index = 0usize;
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(usage) = message.get("usage").filter(|u| u.is_object()) else {
            continue;
        };
        index += 1;
        let key = message
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("missing-id:{index}"));
        let score = integer(usage.get("input_tokens"))
            + integer(usage.get("output_tokens"))
            + integer(usage.get("cache_creation_input_tokens"))
            + integer(usage.get("cache_read_input_tokens"));
        let entry = best.entry(key).or_insert((0, Value::Null));
        if score >= entry.0 {
            *entry = (score, usage.clone());
        }
    }
    if best.is_empty() {
        return None;
    }
    let mut tokens = TranscriptTokens::default();
    for (_, usage) in best.values() {
        let uncached = integer(usage.get("input_tokens"));
        let cache_write = integer(usage.get("cache_creation_input_tokens"));
        let cached = integer(usage.get("cache_read_input_tokens"));
        tokens.input += uncached + cache_write + cached;
        tokens.cached += cached;
        tokens.output += integer(usage.get("output_tokens"));
    }
    tokens.total = tokens.input + tokens.output;
    Some(tokens)
}

fn read_tail(path: &Path, max_bytes: u64) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len <= max_bytes {
        let mut text = String::new();
        file.read_to_string(&mut text).ok()?;
        return Some(text);
    }
    file.seek(SeekFrom::End(-(max_bytes as i64))).ok()?;
    let mut bytes = Vec::with_capacity(max_bytes as usize);
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    // Drop the first (probably partial) line.
    Some(match text.find('\n') {
        Some(cut) => text[cut + 1..].to_string(),
        None => text.into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn codex_uses_the_last_cumulative_counter() {
        let lines = [
            r#"{"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":20,"total_tokens":120}}}}"#,
            r#"{"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":180,"cached_input_tokens":90,"output_tokens":35,"total_tokens":215}}}}"#,
        ]
        .join("\n");
        let tokens = codex_from_lines(&lines).unwrap();
        assert_eq!(tokens.total, 215);
        assert_eq!(tokens.cached, 90);
        assert_eq!(tokens.input, 180);
    }

    #[test]
    fn claude_deduplicates_stream_updates_per_message_id() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"type":"assistant","message":{{"id":"m1","usage":{{"input_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":1}}}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"assistant","message":{{"id":"m1","usage":{{"input_tokens":10,"cache_read_input_tokens":500,"cache_creation_input_tokens":20,"output_tokens":9}}}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"assistant","message":{{"id":"m2","usage":{{"input_tokens":5,"cache_read_input_tokens":100,"cache_creation_input_tokens":0,"output_tokens":2}}}}}}"#
        )
        .unwrap();
        let tokens = claude_tokens(file.path()).unwrap();
        // m1 keeps its most complete row (530 logical input), m2 adds 105.
        assert_eq!(tokens.input, 530 + 105);
        assert_eq!(tokens.cached, 600);
        assert_eq!(tokens.output, 11);
        assert_eq!(tokens.total, tokens.input + tokens.output);
    }

    #[test]
    fn tail_fast_path_still_finds_the_final_counter() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        // Pad beyond the tail window, then a counter near the end.
        let filler = format!("{{\"note\":\"{}\"}}\n", "x".repeat(512));
        for _ in 0..3000 {
            file.write_all(filler.as_bytes()).unwrap();
        }
        writeln!(
            file,
            r#"{{"payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":7,"cached_input_tokens":1,"output_tokens":2,"total_tokens":9}}}}}}}}"#
        )
        .unwrap();
        let tokens = read(file.path(), "codex").unwrap();
        assert_eq!(tokens.total, 9);
    }
}

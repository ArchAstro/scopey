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

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TranscriptTokens {
    /// Logical input, cached reads included.
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub total: u64,
    /// Which model(s) the session spent these tokens on. Claude usage rows
    /// name their model, so the split is exact; Codex counters are
    /// cumulative with no per-model breakdown, so the whole total is
    /// attributed to the model(s) the transcript declares (names joined
    /// when the session switched models mid-flight).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelTokens>,
}

/// Token share of one model within a session transcript.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ModelTokens {
    pub model: String,
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub total: u64,
}

const TAIL_BYTES: u64 = 1_048_576;
const HEAD_BYTES: u64 = 262_144;

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
/// full scan when the tail holds no counter. Model declarations sit near
/// the head (session settings) and recur on switches, so when the tail
/// was truncated the head's names are unioned in — otherwise a session
/// that switched models would report only the later one. Declarations
/// outside both bounded windows can still be missed.
fn codex_tokens(path: &Path) -> Option<TranscriptTokens> {
    let len = fs::metadata(path).ok()?.len();
    let tail = read_tail(path, TAIL_BYTES)?;
    let Some((tokens, tail_names)) = codex_scan(&tail) else {
        let full = fs::read_to_string(path).ok()?;
        return codex_from_lines(&full);
    };
    let mut names = if len > TAIL_BYTES {
        read_head(path, HEAD_BYTES)
            .map(|head| codex_model_names(&head))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    for name in tail_names {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Some(finish_codex(tokens, names))
}

fn codex_from_lines(text: &str) -> Option<TranscriptTokens> {
    let (tokens, names) = codex_scan(text)?;
    Some(finish_codex(tokens, names))
}

/// One pass over JSONL: the last cumulative counter plus declared model
/// names. Returns `None` when no counter exists in `text`.
fn codex_scan(text: &str) -> Option<(TranscriptTokens, Vec<String>)> {
    let mut latest = None;
    let names = codex_model_names(text);
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
    let tokens = TranscriptTokens {
        input,
        cached: integer(usage.get("cached_input_tokens")),
        output,
        total: if total > 0 { total } else { input + output },
        models: Vec::new(),
    };
    Some((tokens, names))
}

fn finish_codex(mut tokens: TranscriptTokens, names: Vec<String>) -> TranscriptTokens {
    tokens.models = attribute_whole(&tokens, names);
    tokens
}

/// Transcript content is untrusted; keep a corrupt or hostile model field
/// from producing unbounded-width report lines.
const MAX_MODEL_NAME: usize = 80;

fn clamp_name(name: &str) -> String {
    name.trim().chars().take(MAX_MODEL_NAME).collect()
}

/// Distinct model names declared by codex session/thread settings and turn
/// context lines, in order of first appearance.
fn codex_model_names(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let payload = value.get("payload").unwrap_or(&value);
        let candidates = [
            payload.get("model"),
            payload.get("state").and_then(|s| s.get("model")),
            payload.get("thread_settings").and_then(|s| s.get("model")),
        ];
        for name in candidates.into_iter().flatten().filter_map(Value::as_str) {
            let name = clamp_name(name);
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// Cumulative counters cannot be split per model, so the whole total goes
/// to the declared model(s) as one row.
fn attribute_whole(tokens: &TranscriptTokens, names: Vec<String>) -> Vec<ModelTokens> {
    if names.is_empty() {
        return Vec::new();
    }
    vec![ModelTokens {
        model: names.join(" + "),
        input: tokens.input,
        cached: tokens.cached,
        output: tokens.output,
        total: tokens.total,
    }]
}

/// Claude transcripts stream usage updates per provider message id; keep the
/// most complete row per id and sum.
fn claude_tokens(path: &Path) -> Option<TranscriptTokens> {
    let text = fs::read_to_string(path).ok()?;
    let mut best: HashMap<String, (u64, Value, String)> = HashMap::new();
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
        let model = clamp_name(message.get("model").and_then(Value::as_str).unwrap_or(""));
        let score = integer(usage.get("input_tokens"))
            + integer(usage.get("output_tokens"))
            + integer(usage.get("cache_creation_input_tokens"))
            + integer(usage.get("cache_read_input_tokens"));
        let entry = best.entry(key).or_insert((0, Value::Null, String::new()));
        if score >= entry.0 {
            *entry = (score, usage.clone(), model);
        }
    }
    if best.is_empty() {
        return None;
    }
    let mut tokens = TranscriptTokens::default();
    for (_, usage, model) in best.values() {
        let uncached = integer(usage.get("input_tokens"));
        let cache_write = integer(usage.get("cache_creation_input_tokens"));
        let cached = integer(usage.get("cache_read_input_tokens"));
        let input = uncached + cache_write + cached;
        let output = integer(usage.get("output_tokens"));
        tokens.input += input;
        tokens.cached += cached;
        tokens.output += output;
        let name = if model.is_empty() { "unknown" } else { model };
        let row = match tokens.models.iter_mut().find(|row| row.model == name) {
            Some(row) => row,
            None => {
                tokens.models.push(ModelTokens {
                    model: name.to_string(),
                    ..ModelTokens::default()
                });
                tokens.models.last_mut().expect("just pushed")
            }
        };
        row.input += input;
        row.cached += cached;
        row.output += output;
    }
    tokens.total = tokens.input + tokens.output;
    for row in &mut tokens.models {
        row.total = row.input + row.output;
    }
    tokens
        .models
        .sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.model.cmp(&b.model)));
    Some(tokens)
}

fn read_head(path: &Path, max_bytes: u64) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut bytes = Vec::new();
    file.take(max_bytes).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    if len <= max_bytes {
        return Some(text.into_owned());
    }
    // Drop the last (probably partial) line.
    Some(match text.rfind('\n') {
        Some(cut) => text[..cut].to_string(),
        None => text.into_owned(),
    })
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
    fn codex_attributes_the_cumulative_total_to_the_declared_model() {
        let lines = [
            r#"{"timestamp":"t","payload":{"thread_settings":{"model":"gpt-5.6-terra"},"type":"thread_settings_applied"}}"#,
            r#"{"timestamp":"t","payload":{"model":"gpt-5.6-terra","approval_policy":"never"}}"#,
            r#"{"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":180,"cached_input_tokens":90,"output_tokens":35,"total_tokens":215}}}}"#,
        ]
        .join("\n");
        let tokens = codex_from_lines(&lines).unwrap();
        assert_eq!(tokens.models.len(), 1);
        assert_eq!(tokens.models[0].model, "gpt-5.6-terra");
        assert_eq!(tokens.models[0].total, 215);
        assert_eq!(tokens.models[0].cached, 90);
    }

    #[test]
    fn codex_joins_model_names_when_the_session_switched() {
        let lines = [
            r#"{"payload":{"state":{"model":"gpt-5.6-terra"}}}"#,
            r#"{"payload":{"model":"gpt-5.6"}}"#,
            r#"{"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":5,"total_tokens":15}}}}"#,
        ]
        .join("\n");
        let tokens = codex_from_lines(&lines).unwrap();
        assert_eq!(tokens.models.len(), 1);
        assert_eq!(tokens.models[0].model, "gpt-5.6-terra + gpt-5.6");
        assert_eq!(tokens.models[0].total, 15);
    }

    #[test]
    fn codex_tail_fast_path_recovers_the_model_from_the_head() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"t","payload":{{"state":{{"model":"gpt-5.6-terra"}}}}}}"#
        )
        .unwrap();
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
        assert_eq!(tokens.models.len(), 1);
        assert_eq!(tokens.models[0].model, "gpt-5.6-terra");
        assert_eq!(tokens.models[0].total, 9);
    }

    #[test]
    fn codex_unions_head_model_with_a_switch_seen_only_in_the_tail() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        // Original model declared at the head only.
        writeln!(
            file,
            r#"{{"timestamp":"t","payload":{{"state":{{"model":"gpt-5.6-early"}}}}}}"#
        )
        .unwrap();
        // Push the head declaration beyond the tail window.
        let filler = format!("{{\"note\":\"{}\"}}\n", "x".repeat(512));
        for _ in 0..3000 {
            file.write_all(filler.as_bytes()).unwrap();
        }
        // Mid-session switch and final counter, both tail-visible.
        writeln!(
            file,
            r#"{{"timestamp":"t","payload":{{"type":"thread_settings_applied","thread_settings":{{"model":"gpt-5.6-late"}}}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":7,"cached_input_tokens":1,"output_tokens":2,"total_tokens":9}}}}}}}}"#
        )
        .unwrap();
        let tokens = read(file.path(), "codex").unwrap();
        assert_eq!(tokens.models.len(), 1);
        assert_eq!(tokens.models[0].model, "gpt-5.6-early + gpt-5.6-late");
        assert_eq!(tokens.models[0].total, 9);
    }

    #[test]
    fn claude_splits_tokens_per_model() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"type":"assistant","message":{{"id":"m1","model":"claude-opus-5","usage":{{"input_tokens":100,"cache_read_input_tokens":60,"cache_creation_input_tokens":0,"output_tokens":30}}}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"assistant","message":{{"id":"m2","model":"claude-haiku-4-5","usage":{{"input_tokens":10,"cache_read_input_tokens":5,"cache_creation_input_tokens":0,"output_tokens":4}}}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"assistant","message":{{"id":"m3","model":"claude-opus-5","usage":{{"input_tokens":40,"cache_read_input_tokens":20,"cache_creation_input_tokens":0,"output_tokens":6}}}}}}"#
        )
        .unwrap();
        let tokens = claude_tokens(file.path()).unwrap();
        assert_eq!(tokens.models.len(), 2);
        assert_eq!(tokens.models[0].model, "claude-opus-5");
        assert_eq!(tokens.models[0].input, 220);
        assert_eq!(tokens.models[0].output, 36);
        assert_eq!(tokens.models[0].total, 256);
        assert_eq!(tokens.models[1].model, "claude-haiku-4-5");
        assert_eq!(tokens.models[1].total, 19);
        let split: u64 = tokens.models.iter().map(|m| m.total).sum();
        assert_eq!(split, tokens.total);
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

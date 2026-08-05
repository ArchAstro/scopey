//! Structured tool journal for judging trajectory.
//!
//! Prefer hook-side `ToolEvent` records over raw transcript tails. Raw Codex/Claude
//! JSONL often ends mid-command-output dump, which produced false "no evidence" warnings.

use crate::session::ToolEvent;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Max chars of tool args kept in the journal / judge excerpt.
pub const DEFAULT_ARGS_PREVIEW_CHARS: usize = 400;

/// Tools that fire PostToolUse often but are not agent decisions (Codex especially).
const NOISE_TOOLS: &[&str] = &[
    "write_stdin",
    "wait_agent",
    "list_agents",
    "token_count",
    "followup_task",
];

/// True when this tool name should advance the meaningful tool counter / N-window.
pub fn counts_toward_n(tool_name: &str) -> bool {
    let n = normalize_name(tool_name);
    if n.is_empty() || n == "unknown" {
        // Unknown hooks still count once — better over-count than miss writes.
        return true;
    }
    // Codex ships its multi-agent tools namespaced without a separator
    // (`collaborationwait_agent`), so compare the bare name too. Spawning an
    // agent is a real decision and still counts; waiting/listing is not.
    let bare = n.strip_prefix("collaboration").unwrap_or(&n);
    !NOISE_TOOLS.iter().any(|t| n == *t || bare == *t)
}

fn normalize_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('-', "_")
}

/// Tool names that never mutate repository or system state.
const READ_ONLY_TOOLS: &[&str] = &[
    "read",
    "read_file",
    "glob",
    "grep",
    "ls",
    "list_dir",
    "list_files",
    "view",
    "notebookread",
];

/// Tool names that execute a shell command carried in the args preview.
const EXEC_TOOLS: &[&str] = &[
    "bash",
    "shell",
    "exec_command",
    "execute_command",
    "run_command",
    "run_terminal_cmd",
    "terminal",
];

/// Shell programs that are read-only when their arguments pass the guards in
/// `command_is_read_only`. Anything not listed is treated as mutating.
const READ_ONLY_PROGRAMS: &[&str] = &[
    "rg", "grep", "cat", "ls", "head", "tail", "wc", "find", "file", "stat", "pwd", "tree", "du",
    "which", "git", "sed", "awk",
];

const READ_ONLY_GIT_SUBCOMMANDS: &[&str] = &[
    "status",
    "log",
    "show",
    "diff",
    "branch",
    "blame",
    "rev-parse",
    "ls-files",
    "remote",
    "describe",
    "shortlog",
    "grep",
];

/// Conservatively decide whether a shell command string is read-only.
///
/// Returns false on anything it cannot prove harmless: redirection, command
/// substitution, unknown programs, sed without `-n`/with `-i`, find with
/// `-delete`/`-exec`, non-allowlisted git subcommands, or a preview that was
/// clipped (the dangerous part could be beyond the clip).
pub fn command_is_read_only(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed.ends_with('…') {
        return false;
    }
    if trimmed.contains('>') || trimmed.contains('`') || trimmed.contains("$(") {
        return false;
    }
    let segments = trimmed
        .split("&&")
        .flat_map(|s| s.split("||"))
        .flat_map(|s| s.split(';'))
        .flat_map(|s| s.split('|'));
    for segment in segments {
        let tokens: Vec<&str> = segment.split_whitespace().collect();
        let Some(first) = tokens.first() else {
            return false;
        };
        if first.contains('=') {
            return false;
        }
        let program = first
            .rsplit('/')
            .next()
            .unwrap_or(first)
            .to_ascii_lowercase();
        if !READ_ONLY_PROGRAMS.contains(&program.as_str()) {
            return false;
        }
        match program.as_str() {
            "sed" if !tokens.contains(&"-n") || tokens.iter().any(|t| t.starts_with("-i")) => {
                return false;
            }
            "find"
                if tokens
                    .iter()
                    .any(|t| matches!(*t, "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir")) =>
            {
                return false;
            }
            "git" => {
                let sub = tokens
                    .iter()
                    .skip(1)
                    .find(|t| !t.starts_with('-'))
                    .copied()
                    .unwrap_or("");
                if !READ_ONLY_GIT_SUBCOMMANDS.contains(&sub) {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

/// True when this journal event provably cannot have mutated state.
pub fn is_read_only_event(event: &ToolEvent) -> bool {
    let name = normalize_name(&event.name);
    if READ_ONLY_TOOLS.contains(&name.as_str()) {
        return true;
    }
    if EXEC_TOOLS.contains(&name.as_str()) {
        return command_is_read_only(&event.args_preview);
    }
    false
}

/// True when a non-empty window contains only provably read-only tools.
pub fn window_is_read_only(events: &[ToolEvent]) -> bool {
    !events.is_empty() && events.iter().all(is_read_only_event)
}

pub fn clip_args(args: &str, max_chars: usize) -> String {
    let max = max_chars.max(32);
    let t = args.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let mut out: String = t.chars().take(max).collect();
    out.push('…');
    out
}

/// Compact JSON/value args for journal storage.
pub fn preview_tool_input(input: Option<&Value>, max_chars: usize) -> String {
    let Some(v) = input else {
        return String::new();
    };
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Object(map) => {
            // Prefer common command/path keys first for readability.
            for key in [
                "cmd",
                "command",
                "file_path",
                "path",
                "filePath",
                "file",
                "pattern",
                "query",
            ] {
                if let Some(x) = map.get(key) {
                    if let Some(t) = x.as_str() {
                        return clip_args(t, max_chars);
                    }
                }
            }
            v.to_string()
        }
        other => other.to_string(),
    };
    clip_args(&s, max_chars)
}

pub fn format_tools_for_judge(events: &[ToolEvent]) -> String {
    if events.is_empty() {
        return "(no tool actions recorded for this window)".into();
    }
    let mut lines = Vec::with_capacity(events.len());
    for e in events {
        let args = if e.args_preview.is_empty() {
            String::new()
        } else {
            format!(": {}", e.args_preview.replace('\n', " "))
        };
        lines.push(format!("TOOL {} {}{}", e.index, e.name, args));
    }
    lines.join("\n")
}

/// Extract high-signal tool calls from a Codex/Claude-style JSONL transcript.
/// Used only as a fallback when the hook journal is empty for a window.
pub fn extract_tools_from_transcript(path: &Path, max_events: usize) -> Vec<ToolEvent> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    extract_tools_from_transcript_text(&text, max_events)
}

pub fn extract_tools_from_transcript_text(text: &str, max_events: usize) -> Vec<ToolEvent> {
    let mut out = Vec::new();
    let mut index = 0u64;
    for line in text.lines() {
        let Ok(o) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let pl = o.get("payload").cloned().unwrap_or(o.clone());
        let pt = pl.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let is_call = matches!(
            pt,
            "function_call" | "custom_tool_call" | "tool_use" | "tool_call"
        );
        if !is_call {
            continue;
        }
        let name = pl
            .get("name")
            .or_else(|| pl.get("tool_name"))
            .and_then(|n| n.as_str())
            .unwrap_or("unknown");
        if !counts_toward_n(name) {
            continue;
        }
        let args = pl
            .get("arguments")
            .or_else(|| pl.get("input"))
            .or_else(|| pl.get("params"))
            .or_else(|| pl.get("command"));
        let preview = match args {
            Some(Value::String(s)) => clip_args(s, DEFAULT_ARGS_PREVIEW_CHARS),
            Some(v) => preview_tool_input(Some(v), DEFAULT_ARGS_PREVIEW_CHARS),
            None => String::new(),
        };
        let ts = o
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(parse_ts)
            .unwrap_or_else(Utc::now);
        index += 1;
        out.push(ToolEvent {
            index,
            name: name.to_string(),
            args_preview: preview,
            ts,
            noise: false,
        });
    }
    if out.len() > max_events {
        let skip = out.len() - max_events;
        out = out.split_off(skip);
    }
    out
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
        .or_else(|| {
            // Codex uses "2026-07-31T00:36:39.371Z"
            chrono::DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.fZ")
                .ok()
                .map(|d| d.with_timezone(&Utc))
        })
}

/// Slice journal events whose index is in [from, to).
pub fn events_in_window(events: &[ToolEvent], from: u64, to: u64) -> Vec<ToolEvent> {
    events
        .iter()
        .filter(|e| e.index > from && e.index <= to && !e.noise)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_tools_do_not_count() {
        assert!(!counts_toward_n("write_stdin"));
        assert!(!counts_toward_n("Wait-Agent"));
        assert!(counts_toward_n("apply_patch"));
        assert!(counts_toward_n("exec_command"));
        assert!(counts_toward_n("Bash"));
        assert!(counts_toward_n("Edit"));
    }

    #[test]
    fn codex_collab_tools_classified_by_bare_name() {
        // Observed live from Codex multi-agent runs: the collab namespace has
        // no separator, so the bare suffix decides. Waiting on a child agent
        // is bookkeeping; spawning one is a real delegation decision.
        assert!(!counts_toward_n("collaborationwait_agent"));
        assert!(!counts_toward_n("collaborationlist_agents"));
        assert!(counts_toward_n("collaborationspawn_agent"));
    }

    #[test]
    fn format_tools_includes_index_and_name() {
        let ev = vec![ToolEvent {
            index: 3,
            name: "apply_patch".into(),
            args_preview: "Update File: foo.rs".into(),
            ts: Utc::now(),
            noise: false,
        }];
        let s = format_tools_for_judge(&ev);
        assert!(s.contains("TOOL 3 apply_patch"));
        assert!(s.contains("foo.rs"));
    }

    #[test]
    fn extract_codex_style_calls_skips_token_and_dumps() {
        // Simulate a transcript that ends with a huge output dump after tools.
        let mut lines = Vec::new();
        lines.push(r#"{"timestamp":"2026-07-31T00:00:00.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"rg foo\"}"}}"#.to_string());
        lines.push(r#"{"timestamp":"2026-07-31T00:00:01.000Z","type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"*** Begin Patch\n*** Update File: a.rs\n"}}"#.to_string());
        lines.push(r#"{"timestamp":"2026-07-31T00:00:02.000Z","type":"response_item","payload":{"type":"custom_tool_call","name":"write_stdin","input":{}}}"#.to_string());
        // giant dump line (would poison a raw char-tail)
        let dump = format!(
            r#"{{"timestamp":"2026-07-31T00:00:03.000Z","type":"event_msg","payload":{{"type":"token_count","info":"{}"}}}}"#,
            "AccountsFixtures\n".repeat(2000)
        );
        lines.push(dump);
        let text = lines.join("\n");
        let tools = extract_tools_from_transcript_text(&text, 50);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "exec_command");
        assert_eq!(tools[1].name, "apply_patch");
        assert!(tools[0].args_preview.contains("rg foo") || tools[0].args_preview.contains("cmd"));
    }

    #[test]
    fn events_in_window_filters_by_index() {
        let ev: Vec<ToolEvent> = (1..=20)
            .map(|i| ToolEvent {
                index: i,
                name: format!("t{i}"),
                args_preview: String::new(),
                ts: Utc::now(),
                noise: false,
            })
            .collect();
        let w = events_in_window(&ev, 10, 15);
        assert_eq!(w.len(), 5);
        assert_eq!(w[0].index, 11);
        assert_eq!(w[4].index, 15);
    }

    #[test]
    fn empty_window_format() {
        assert!(format_tools_for_judge(&[]).contains("no tool actions"));
    }

    fn event(name: &str, args: &str) -> ToolEvent {
        ToolEvent {
            index: 1,
            name: name.into(),
            args_preview: args.into(),
            ts: Utc::now(),
            noise: false,
        }
    }

    #[test]
    fn read_only_commands_are_conservative() {
        assert!(command_is_read_only("rg -n scope src/"));
        assert!(command_is_read_only("cat a.txt && grep foo b.txt"));
        assert!(command_is_read_only("git log --oneline | head -5"));
        assert!(command_is_read_only("sed -n '1,20p' file.rs"));
        assert!(command_is_read_only("/usr/bin/ls -la"));
        // Anything that could write, or that we cannot see fully, is not read-only.
        assert!(!command_is_read_only("cat a.txt > b.txt"));
        assert!(!command_is_read_only("sed -i '' 's/a/b/' file.rs"));
        assert!(!command_is_read_only("sed 's/a/b/' file.rs"));
        assert!(!command_is_read_only("find . -name '*.tmp' -delete"));
        assert!(!command_is_read_only("git push"));
        assert!(!command_is_read_only("git checkout main"));
        assert!(!command_is_read_only("python3 run.py"));
        assert!(!command_is_read_only("FOO=1 rg pattern"));
        assert!(!command_is_read_only("echo $(rm -rf /tmp/x)"));
        assert!(!command_is_read_only(
            "rg -n very-long-command-that-was-clipped…"
        ));
        assert!(!command_is_read_only(""));
    }

    #[test]
    fn read_only_window_requires_every_event_proven_safe() {
        let safe = vec![event("Read", "src/main.rs"), event("Bash", "git status")];
        assert!(window_is_read_only(&safe));
        let mixed = vec![
            event("Read", "src/main.rs"),
            event("apply_patch", "Update File: a.rs"),
        ];
        assert!(!window_is_read_only(&mixed));
        let unknown = vec![event("mystery_tool", "")];
        assert!(!window_is_read_only(&unknown));
        assert!(!window_is_read_only(&[]));
    }
}

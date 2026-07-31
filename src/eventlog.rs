//! Per-session JSONL debug log.
//!
//! Path: `~/.scopey/logs/<session_id>.jsonl`
//!
//! Each line is one JSON object:
//! ```json
//! {
//!   "ts": "2026-07-30T12:00:00Z",
//!   "level": "info",
//!   "session_id": "…",
//!   "event": "hook.post_tool",
//!   "message": "scheduled judge",
//!   "pid": 1234,
//!   "internal": false,
//!   "fields": { "tool_count": 15 }
//! }
//! ```

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::config::Config;
use crate::guard;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "debug" | "dbg" => Some(Level::Debug),
            "info" => Some(Level::Info),
            "warn" | "warning" => Some(Level::Warn),
            "error" | "err" => Some(Level::Error),
            _ => None,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Level::Debug => 0,
            Level::Info => 1,
            Level::Warn => 2,
            Level::Error => 3,
        }
    }

    fn meets(self, min: Level) -> bool {
        self.rank() >= min.rank()
    }
}

/// Sanitize session id for a single path component.
pub fn safe_session_id(session_id: &str) -> String {
    let s: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "_unknown".into()
    } else {
        s
    }
}

pub fn logs_dir() -> PathBuf {
    Config::scopey_home().join("logs")
}

pub fn session_log_path(session_id: &str) -> PathBuf {
    logs_dir().join(format!("{}.jsonl", safe_session_id(session_id)))
}

/// Append one structured log line for a session. Best-effort (never panics callers).
pub fn write(session_id: &str, level: Level, event: &str, message: impl AsRef<str>, fields: Value) {
    let sid = session_id.trim();
    if sid.is_empty() {
        return;
    }
    let path = session_log_path(sid);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let mut field_map = Map::new();
    if let Value::Object(m) = fields {
        field_map = m;
    } else if !fields.is_null() {
        field_map.insert("value".into(), fields);
    }

    let entry = json!({
        "ts": Utc::now().to_rfc3339(),
        "level": level.as_str(),
        "session_id": sid,
        "event": event,
        "message": message.as_ref(),
        "pid": std::process::id(),
        "internal": guard::is_internal(),
        "fields": field_map,
    });

    let line = match serde_json::to_string(&entry) {
        Ok(s) => s,
        Err(_) => return,
    };

    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }

    // Mirror to stderr for live operators (hooks / bg jobs).
    match level {
        Level::Error => eprintln!("scopey[{sid}] ERROR {event}: {}", message.as_ref()),
        Level::Warn => eprintln!("scopey[{sid}] WARN {event}: {}", message.as_ref()),
        Level::Info => eprintln!("scopey[{sid}] {event}: {}", message.as_ref()),
        Level::Debug => {
            if std::env::var_os("SCOPEY_DEBUG").is_some() {
                eprintln!("scopey[{sid}] DEBUG {event}: {}", message.as_ref());
            }
        }
    }
}

pub fn info(session_id: &str, event: &str, message: impl AsRef<str>, fields: Value) {
    write(session_id, Level::Info, event, message, fields);
}
pub fn warn(session_id: &str, event: &str, message: impl AsRef<str>, fields: Value) {
    write(session_id, Level::Warn, event, message, fields);
}
pub fn error(session_id: &str, event: &str, message: impl AsRef<str>, fields: Value) {
    write(session_id, Level::Error, event, message, fields);
}
pub fn debug(session_id: &str, event: &str, message: impl AsRef<str>, fields: Value) {
    write(session_id, Level::Debug, event, message, fields);
}

#[derive(Debug)]
pub struct LogsQuery {
    pub session_id: String,
    pub tail: Option<usize>,
    pub min_level: Level,
    pub event_prefix: Option<String>,
    pub follow: bool,
    pub raw: bool,
    pub path_only: bool,
}

/// Print or stream logs for a session.
pub fn cmd_logs(q: LogsQuery) -> Result<()> {
    let path = session_log_path(&q.session_id);
    if q.path_only {
        println!("{}", path.display());
        return Ok(());
    }

    if !path.exists() {
        // Helpful: list nearby sessions if typo
        let available = list_session_ids(40)?;
        eprintln!(
            "scopey logs: no log file at {}\n\
             (hooks write here as they run; start a session or check --session spelling)",
            path.display()
        );
        if !available.is_empty() {
            eprintln!("recent session log ids:");
            for id in available.iter().take(15) {
                eprintln!("  {id}");
            }
        }
        bail!("log not found for session {}", q.session_id);
    }

    if q.follow {
        follow_file(&path, &q)?;
    } else {
        print_file(&path, &q)?;
    }
    Ok(())
}

fn print_file(path: &Path, q: &LogsQuery) -> Result<()> {
    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut lines: Vec<String> = reader.lines().collect::<std::io::Result<_>>()?;
    if let Some(n) = q.tail {
        if lines.len() > n {
            lines = lines.split_off(lines.len() - n);
        }
    }
    for line in lines {
        emit_line(&line, q);
    }
    Ok(())
}

fn follow_file(path: &Path, q: &LogsQuery) -> Result<()> {
    // Print existing (respecting tail), then poll for new lines.
    let mut file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut all = String::new();
    file.read_to_string(&mut all)?;
    let mut existing: Vec<&str> = all.lines().collect();
    if let Some(n) = q.tail {
        if existing.len() > n {
            existing = existing.split_off(existing.len() - n);
        }
    }
    for line in existing {
        emit_line(line, q);
    }

    let mut pos = file.seek(SeekFrom::End(0))?;
    loop {
        thread::sleep(Duration::from_millis(300));
        let mut f = match fs::File::open(path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let len = f.seek(SeekFrom::End(0))?;
        if len < pos {
            // truncated / rotated
            pos = 0;
        }
        if len == pos {
            continue;
        }
        f.seek(SeekFrom::Start(pos))?;
        let mut buf = String::new();
        f.read_to_string(&mut buf)?;
        pos = f.stream_position().unwrap_or(len);
        for line in buf.lines() {
            if !line.is_empty() {
                emit_line(line, q);
            }
        }
    }
}

fn emit_line(line: &str, q: &LogsQuery) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    if q.raw {
        // still filter level/event if possible
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if !passes_filters(&v, q) {
                return;
            }
        }
        println!("{line}");
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        println!("{line}");
        return;
    };
    if !passes_filters(&v, q) {
        return;
    }
    let ts = v.get("ts").and_then(|x| x.as_str()).unwrap_or("?");
    // shorten ts for display
    let ts_short = if ts.len() >= 19 { &ts[..19] } else { ts };
    let level = v
        .get("level")
        .and_then(|x| x.as_str())
        .unwrap_or("info")
        .to_ascii_uppercase();
    let event = v.get("event").and_then(|x| x.as_str()).unwrap_or("-");
    let msg = v.get("message").and_then(|x| x.as_str()).unwrap_or("");
    let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0);
    let internal = v.get("internal").and_then(|x| x.as_bool()).unwrap_or(false);
    let flag = if internal { " int" } else { "" };
    let fields = v.get("fields").cloned().unwrap_or(json!({}));
    let fields_s = if fields.as_object().map(|m| !m.is_empty()).unwrap_or(false) {
        format!(" {}", fields)
    } else {
        String::new()
    };
    println!("{ts_short} {level:<5} pid={pid}{flag} {event}  {msg}{fields_s}");
}

fn passes_filters(v: &Value, q: &LogsQuery) -> bool {
    let level = v
        .get("level")
        .and_then(|x| x.as_str())
        .and_then(Level::parse)
        .unwrap_or(Level::Info);
    if !level.meets(q.min_level) {
        return false;
    }
    if let Some(ref pref) = q.event_prefix {
        let event = v.get("event").and_then(|x| x.as_str()).unwrap_or("");
        if !event.starts_with(pref.as_str()) && !event.contains(pref.as_str()) {
            return false;
        }
    }
    true
}

pub fn list_session_ids(limit: usize) -> Result<Vec<String>> {
    let dir = logs_dir();
    if !dir.is_dir() {
        return Ok(vec![]);
    }
    let mut entries: Vec<(std::time::SystemTime, String)> = Vec::new();
    for e in fs::read_dir(&dir)? {
        let e = e?;
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        // skip non-session artifacts like summarize-*.log
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if stem.is_empty() || stem.starts_with("summarize-") || stem.starts_with("judge-") {
            continue;
        }
        let modified = e
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        entries.push((modified, stem));
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    entries.truncate(limit);
    Ok(entries.into_iter().map(|(_, id)| id).collect())
}

/// List recent log files (for `scopey logs` without --session).
pub fn cmd_list_logs(limit: usize) -> Result<()> {
    let ids = list_session_ids(limit)?;
    if ids.is_empty() {
        println!("(no session jsonl logs under {})", logs_dir().display());
        println!("Logs appear after hooks run for a session.");
        return Ok(());
    }
    for id in ids {
        let path = session_log_path(&id);
        let meta = fs::metadata(&path).ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let mtime = meta
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        println!("{id}  {size}B  mtime_unix={mtime}  {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn safe_id() {
        assert_eq!(safe_session_id("abc-123"), "abc-123");
        assert!(safe_session_id("../x").contains('_'));
        assert_eq!(safe_session_id(""), "_unknown");
        assert!(!safe_session_id("a/b\\c").contains('/'));
    }

    #[test]
    fn level_filter() {
        assert!(Level::Error.meets(Level::Info));
        assert!(!Level::Debug.meets(Level::Warn));
        assert!(Level::Warn.meets(Level::Warn));
        assert_eq!(Level::parse("WARNING"), Some(Level::Warn));
        assert_eq!(Level::parse("err"), Some(Level::Error));
        assert_eq!(Level::parse("nope"), None);
    }

    #[test]
    fn write_and_read_session_log() {
        Config::with_temp_scopey_home(|_| {
            let sid = "log-sess-1";
            info(sid, "test.event", "hello", json!({"n": 1}));
            warn(sid, "test.warn", "careful", json!({}));
            error(sid, "test.err", "boom", json!({"code": 7}));
            let path = session_log_path(sid);
            assert!(path.exists(), "{}", path.display());
            let text = fs::read_to_string(&path).unwrap();
            let lines: Vec<_> = text.lines().filter(|l| !l.is_empty()).collect();
            assert_eq!(lines.len(), 3);
            let v: Value = serde_json::from_str(lines[0]).unwrap();
            assert_eq!(v["session_id"], sid);
            assert_eq!(v["event"], "test.event");
            assert_eq!(v["message"], "hello");
            assert_eq!(v["fields"]["n"], 1);
            assert!(v["pid"].as_u64().unwrap() > 0);

            let q = LogsQuery {
                session_id: sid.into(),
                tail: Some(2),
                min_level: Level::Warn,
                event_prefix: None,
                follow: false,
                raw: false,
                path_only: false,
            };
            cmd_logs(q).unwrap();

            let ids = list_session_ids(10).unwrap();
            assert!(ids.iter().any(|i| i == sid));
        });
    }

    #[test]
    fn passes_event_prefix_filter() {
        let v = json!({
            "level": "info",
            "event": "hook.post_tool.judge",
            "message": "x"
        });
        let q = LogsQuery {
            session_id: "s".into(),
            tail: None,
            min_level: Level::Info,
            event_prefix: Some("guard".into()),
            follow: false,
            raw: false,
            path_only: false,
        };
        assert!(!passes_filters(&v, &q));
        let q2 = LogsQuery {
            event_prefix: Some("hook".into()),
            ..q
        };
        assert!(passes_filters(&v, &q2));
    }

    #[test]
    fn path_only_and_missing_session() {
        Config::with_temp_scopey_home(|_| {
            let q = LogsQuery {
                session_id: "missing-xyz".into(),
                tail: None,
                min_level: Level::Info,
                event_prefix: None,
                follow: false,
                raw: false,
                path_only: true,
            };
            cmd_logs(q).unwrap();
            let q2 = LogsQuery {
                session_id: "missing-xyz".into(),
                tail: None,
                min_level: Level::Info,
                event_prefix: None,
                follow: false,
                raw: false,
                path_only: false,
            };
            assert!(cmd_logs(q2).is_err());
        });
    }
}

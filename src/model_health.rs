//! Cross-session health record for background model jobs (summarize/judge).
//!
//! Workers already log every model failure to the per-session JSONL, but
//! nothing user-facing reads those files, so a broken model runner looks like
//! a healthy install while every scope silently degrades to the
//! FALLBACK_LATEST echo of the last prompt. Workers record each job outcome
//! here; `scopey doctor`, `scopey status`, and `scopey models --verify`
//! surface it, and a persistent failure streak fires a notification
//! (`notify_on_model_fallback`).

use crate::config::Config;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::path::PathBuf;

/// Consecutive failures before the "model unavailable" notification fires and
/// `scopey doctor` reports the check as failed. One failure can be a blip;
/// two in a row means jobs are not recovering on their own.
pub const PERSISTENT_FAILURE_THRESHOLD: u64 = 2;
/// While the streak continues, re-notify at most this often.
const RENOTIFY_INTERVAL_HOURS: i64 = 6;
/// Refuse to parse a health file larger than this. Writers keep the file far
/// smaller (`model_health_history` bounds `recent`, errors are clipped), so
/// anything bigger is corrupt or foreign and is treated as empty; the next
/// recorded outcome rewrites it at a bounded size.
const MAX_HEALTH_FILE_BYTES: u64 = 262_144;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelHealth {
    pub consecutive_failures: u64,
    pub total_failures: u64,
    pub total_successes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_notified_at: Option<DateTime<Utc>>,
    /// Job kind of the most recent outcome: "summarize" or "judge".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_kind: Option<String>,
    /// Recent job outcomes, oldest first. Truncated to `model_health_history`
    /// on every write so the file stays small.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<JobRecord>,
}

/// One background job outcome in the bounded `recent` history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub at: DateTime<Utc>,
    /// "summarize" or "judge".
    pub kind: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ModelHealth {
    pub fn attempts(&self) -> u64 {
        self.total_failures + self.total_successes
    }

    pub fn failing_persistently(&self) -> bool {
        self.consecutive_failures >= PERSISTENT_FAILURE_THRESHOLD
    }
}

fn health_path() -> PathBuf {
    Config::scopey_home().join("model_health.json")
}

/// Best-effort read; a missing, corrupt, or oversized file is an empty record.
pub fn load() -> ModelHealth {
    let path = health_path();
    match fs::metadata(&path) {
        Ok(meta) if meta.len() <= MAX_HEALTH_FILE_BYTES => {}
        _ => return ModelHealth::default(),
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn store(health: &ModelHealth) -> Result<()> {
    let path = health_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Atomic replace so lock-less readers never see a torn file.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(health)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

/// Serialize read-modify-write across concurrent workers (unlocked on drop).
/// Best-effort: if the lock cannot be taken the update proceeds unlocked — a
/// lost counter increment beats a blocked worker.
fn take_update_lock() -> Option<fs::File> {
    let path = Config::scopey_home().join("model_health.json.lock");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .ok()?;
    file.lock_exclusive().ok()?;
    Some(file)
}

/// Drop the oldest entries beyond the configured history size.
fn truncate_recent(health: &mut ModelHealth, cfg: &Config) {
    let cap = cfg.model_health_history;
    if health.recent.len() > cap {
        let excess = health.recent.len() - cap;
        health.recent.drain(..excess);
    }
}

/// Record a successful background model call, clearing the failure streak.
pub fn record_success(cfg: &Config, kind: &str) {
    let now = Utc::now();
    let _lock = take_update_lock();
    let mut health = load();
    health.consecutive_failures = 0;
    health.total_successes = health.total_successes.saturating_add(1);
    health.last_success_at = Some(now);
    health.last_kind = Some(kind.to_string());
    health.recent.push(JobRecord {
        at: now,
        kind: kind.to_string(),
        ok: true,
        error: None,
    });
    truncate_recent(&mut health, cfg);
    if let Err(e) = store(&health) {
        eprintln!("scopey model-health: write failed: {e:#}");
    }
}

/// Record a failed background model call; notify once the failure is persistent.
pub fn record_failure(cfg: &Config, kind: &str, error: &str) {
    let now = Utc::now();
    let _lock = take_update_lock();
    let mut health = load();
    let clipped = clip(error, 500);
    health.consecutive_failures = health.consecutive_failures.saturating_add(1);
    health.total_failures = health.total_failures.saturating_add(1);
    health.last_error = Some(clipped.clone());
    health.last_failure_at = Some(now);
    health.last_kind = Some(kind.to_string());
    health.recent.push(JobRecord {
        at: now,
        kind: kind.to_string(),
        ok: false,
        error: Some(clipped),
    });
    truncate_recent(&mut health, cfg);

    let notify_now = cfg.notify_on_model_fallback && should_notify(&health, now);
    if notify_now {
        health.last_notified_at = Some(now);
    }
    if let Err(e) = store(&health) {
        eprintln!("scopey model-health: write failed: {e:#}");
    }
    if notify_now {
        let body = format!(
            "{} {kind}/model call(s) failed in a row — scope tracking is degraded \
             to echoing the latest prompt. Run `scopey doctor`. Last error: {}",
            health.consecutive_failures,
            clip(error, 160),
        );
        if let Err(e) = crate::notify::notify(cfg, "scopey: model unavailable", &body, None) {
            eprintln!("scopey model-health: notify failed: {e:#}");
        }
    }
}

/// Fire on reaching the persistent threshold, then at most every few hours
/// while the streak continues. A success resets the streak, so the throttle
/// only matters while jobs keep failing.
fn should_notify(health: &ModelHealth, now: DateTime<Utc>) -> bool {
    if !health.failing_persistently() {
        return false;
    }
    match health.last_notified_at {
        None => true,
        Some(t) => now - t >= Duration::hours(RENOTIFY_INTERVAL_HOURS),
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.trim().to_string();
    }
    let clipped: String = s.chars().take(max).collect();
    format!("{}…", clipped.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_streak_counts_and_success_resets() {
        Config::with_temp_scopey_home(|_| {
            let cfg = Config {
                notify_on_model_fallback: false,
                ..Config::default()
            };
            record_failure(&cfg, "summarize", "boom");
            record_failure(&cfg, "judge", "boom again");
            let h = load();
            assert_eq!(h.consecutive_failures, 2);
            assert_eq!(h.total_failures, 2);
            assert!(h.failing_persistently());
            assert_eq!(h.last_kind.as_deref(), Some("judge"));
            assert!(h.last_error.unwrap().contains("boom again"));

            record_success(&cfg, "summarize");
            let h = load();
            assert_eq!(h.consecutive_failures, 0);
            assert_eq!(h.total_successes, 1);
            assert_eq!(h.total_failures, 2);
            assert!(!h.failing_persistently());
            assert_eq!(h.recent.len(), 3);
            assert!(h.recent[2].ok);
        });
    }

    #[test]
    fn recent_history_truncates_to_configured_cap() {
        Config::with_temp_scopey_home(|_| {
            let cfg = Config {
                model_health_history: 3,
                notify_on_model_fallback: false,
                ..Config::default()
            };
            record_failure(&cfg, "summarize", "e1");
            record_success(&cfg, "judge");
            record_failure(&cfg, "summarize", "e3");
            record_failure(&cfg, "judge", "e4");
            record_success(&cfg, "summarize");

            let h = load();
            // Oldest two dropped, order preserved, newest last.
            assert_eq!(h.recent.len(), 3);
            assert_eq!(h.recent[0].error.as_deref(), Some("e3"));
            assert_eq!(h.recent[1].error.as_deref(), Some("e4"));
            assert!(h.recent[2].ok);
            // Counters still cover every outcome, not just retained ones.
            assert_eq!(h.total_failures, 3);
            assert_eq!(h.total_successes, 2);
        });
    }

    #[test]
    fn zero_history_disables_recent_records() {
        Config::with_temp_scopey_home(|_| {
            let cfg = Config {
                model_health_history: 0,
                notify_on_model_fallback: false,
                ..Config::default()
            };
            record_failure(&cfg, "summarize", "boom");
            record_success(&cfg, "judge");
            let h = load();
            assert!(h.recent.is_empty());
            assert_eq!(h.attempts(), 2);
        });
    }

    #[test]
    fn oversized_health_file_reads_as_empty() {
        Config::with_temp_scopey_home(|dir| {
            let big = "x".repeat((MAX_HEALTH_FILE_BYTES + 1) as usize);
            fs::write(dir.join("model_health.json"), big).unwrap();
            assert_eq!(load().attempts(), 0);
        });
    }

    #[test]
    fn notify_gate_needs_streak_and_respects_throttle() {
        let now = Utc::now();
        let mut h = ModelHealth {
            consecutive_failures: 1,
            ..Default::default()
        };
        assert!(!should_notify(&h, now));
        h.consecutive_failures = PERSISTENT_FAILURE_THRESHOLD;
        assert!(should_notify(&h, now));
        h.last_notified_at = Some(now - Duration::minutes(5));
        assert!(!should_notify(&h, now));
        h.last_notified_at = Some(now - Duration::hours(RENOTIFY_INTERVAL_HOURS + 1));
        assert!(should_notify(&h, now));
    }

    #[test]
    fn corrupt_health_file_reads_as_empty() {
        Config::with_temp_scopey_home(|dir| {
            fs::write(dir.join("model_health.json"), "{not json").unwrap();
            let h = load();
            assert_eq!(h.attempts(), 0);
        });
    }
}

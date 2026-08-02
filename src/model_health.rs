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

/// Best-effort read; a missing or corrupt file is an empty record.
pub fn load() -> ModelHealth {
    fs::read_to_string(health_path())
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

/// Record a successful background model call, clearing the failure streak.
pub fn record_success(kind: &str) {
    let _lock = take_update_lock();
    let mut health = load();
    health.consecutive_failures = 0;
    health.total_successes = health.total_successes.saturating_add(1);
    health.last_success_at = Some(Utc::now());
    health.last_kind = Some(kind.to_string());
    if let Err(e) = store(&health) {
        eprintln!("scopey model-health: write failed: {e:#}");
    }
}

/// Record a failed background model call; notify once the failure is persistent.
pub fn record_failure(cfg: &Config, kind: &str, error: &str) {
    let now = Utc::now();
    let _lock = take_update_lock();
    let mut health = load();
    health.consecutive_failures = health.consecutive_failures.saturating_add(1);
    health.total_failures = health.total_failures.saturating_add(1);
    health.last_error = Some(clip(error, 500));
    health.last_failure_at = Some(now);
    health.last_kind = Some(kind.to_string());

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

            record_success("summarize");
            let h = load();
            assert_eq!(h.consecutive_failures, 0);
            assert_eq!(h.total_successes, 1);
            assert_eq!(h.total_failures, 2);
            assert!(!h.failing_persistently());
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

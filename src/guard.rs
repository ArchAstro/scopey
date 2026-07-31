//! Process-storm guards for scopey.
//!
//! 1. **Internal mode** (`SCOPEY_INTERNAL=1`): all hook handlers no-op. Every
//!    child process we spawn (background summarize/judge, `claude -p`, `codex
//!    exec`) must set this so headless model runs cannot re-enter hooks.
//!
//! 2. **Per-session single-flight**: at most one background job
//!    (summarize|judge) may run for a given session_id. A lock file holds the
//!    pid; if that pid is still alive, new spawns are refused.
//!
//! 3. **Min interval**: refuse a new spawn for a session if the last successful
//!    acquire was too recent (config: min_job_interval_secs).

use crate::config::Config;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Env var set on every process scopey spawns. Hooks must exit immediately.
pub const ENV_INTERNAL: &str = "SCOPEY_INTERNAL";
/// Alias accepted for the same guard.
pub const ENV_HOOKS_DISABLED: &str = "SCOPEY_HOOKS_DISABLED";

/// True when this process must not run hook side-effects (model children, nested scopey).
pub fn is_internal() -> bool {
    env_truthy(ENV_INTERNAL) || env_truthy(ENV_HOOKS_DISABLED)
}

fn env_truthy(key: &str) -> bool {
    match std::env::var(key) {
        Ok(v) => {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes")
        }
        Err(_) => false,
    }
}

/// Disable scopey re-entry (and Claude Code hooks) without breaking OAuth.
///
/// Prefer this for `claude -p` children: setting `CLAUDE_CODE_SIMPLE=1` forces
/// API-key-only auth and prints "Not logged in" with exit 0 when the user is
/// only OAuth-authenticated — which is the common Claude Code desktop case.
pub fn apply_hook_disable_env(cmd: &mut std::process::Command) {
    cmd.env(ENV_INTERNAL, "1");
    cmd.env(ENV_HOOKS_DISABLED, "1");
    // Defensive: some Claude wrappers honor this without disabling OAuth.
    cmd.env("CLAUDE_CODE_DISABLE_HOOKS", "1");
}

/// Apply storm-prevention env to a Command before spawn.
///
/// Includes `CLAUDE_CODE_SIMPLE=1` for maximum surface reduction. Use
/// [`apply_hook_disable_env`] for `claude -p` so OAuth still works.
pub fn apply_internal_env(cmd: &mut std::process::Command) {
    apply_hook_disable_env(cmd);
    // Claude Code: reduce hook/plugin surface when children inherit env.
    // NOTE: breaks OAuth — only use for non-Claude runners or true sandboxes.
    cmd.env("CLAUDE_CODE_SIMPLE", "1");
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JobLock {
    pid: u32,
    kind: String,
    session_id: String,
    started_unix: u64,
    /// Last time any job was acquired for this session (even if finished).
    last_acquire_unix: u64,
}

pub struct SessionJobGuard {
    path: PathBuf,
    session_id: String,
    #[allow(dead_code)]
    kind: String,
    held: bool,
}

impl SessionJobGuard {
    fn lock_path(_cfg: &Config, session_id: &str) -> PathBuf {
        let safe: String = session_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Config::scopey_home()
            .join("locks")
            .join(format!("{safe}.job.lock"))
    }

    /// Try to become the sole background worker for this session.
    /// Returns None if another live worker holds the lock or min interval not elapsed.
    pub fn try_acquire(cfg: &Config, session_id: &str, kind: &str) -> Result<Option<Self>> {
        let path = Self::lock_path(cfg, session_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let now = unix_now();
        // 0 disables throttle (useful in tests); otherwise enforce min interval.
        let min_iv = cfg.min_job_interval_secs;

        // Exclusive flock while we decide + write.
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open job lock {}", path.display()))?;

        use fs2::FileExt;
        file.lock_exclusive()
            .with_context(|| format!("lock {}", path.display()))?;

        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        let existing: Option<JobLock> = if buf.trim().is_empty() {
            None
        } else {
            serde_json::from_str(&buf).ok()
        };

        if let Some(ref ex) = existing {
            if pid_alive(ex.pid) {
                crate::eventlog::info(
                    session_id,
                    "guard.busy",
                    format!(
                        "refuse {kind}: live job pid={} kind={} age={}s",
                        ex.pid,
                        ex.kind,
                        now.saturating_sub(ex.started_unix)
                    ),
                    serde_json::json!({
                        "requested_kind": kind,
                        "live_pid": ex.pid,
                        "live_kind": ex.kind,
                    }),
                );
                let _ = file.unlock();
                return Ok(None);
            }
            // Stale lock — check throttle against last_acquire (skip if min_iv == 0)
            if min_iv > 0 && now.saturating_sub(ex.last_acquire_unix) < min_iv {
                crate::eventlog::info(
                    session_id,
                    "guard.throttle",
                    format!(
                        "refuse {kind}: min_job_interval_secs={min_iv} (last {}s ago)",
                        now.saturating_sub(ex.last_acquire_unix)
                    ),
                    serde_json::json!({ "requested_kind": kind, "min_iv": min_iv }),
                );
                let _ = file.unlock();
                return Ok(None);
            }
        }

        let lock = JobLock {
            pid: std::process::id(),
            kind: kind.to_string(),
            session_id: session_id.to_string(),
            started_unix: now,
            last_acquire_unix: now,
        };
        let json = serde_json::to_string_pretty(&lock)?;
        file.set_len(0)?;
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(0))?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        let _ = file.unlock();

        crate::eventlog::info(
            session_id,
            "guard.acquire",
            format!("acquired job lock kind={kind}"),
            serde_json::json!({ "kind": kind, "pid": lock.pid }),
        );

        Ok(Some(Self {
            path,
            session_id: session_id.to_string(),
            kind: kind.to_string(),
            held: true,
        }))
    }

    /// Parent-side: try to reserve the slot for a *child* we are about to spawn.
    /// Writes a placeholder with our pid until the child re-acquires / rewrites.
    /// Actually better: parent only checks can_spawn, child acquires.
    pub fn can_spawn(cfg: &Config, session_id: &str) -> Result<bool> {
        let path = Self::lock_path(cfg, session_id);
        if !path.exists() {
            return Ok(true);
        }
        let text = fs::read_to_string(&path).unwrap_or_default();
        let Ok(ex) = serde_json::from_str::<JobLock>(&text) else {
            return Ok(true);
        };
        let now = unix_now();
        if pid_alive(ex.pid) {
            eprintln!(
                "scopey guard: can_spawn=false session={session_id} live_pid={} kind={}",
                ex.pid, ex.kind
            );
            return Ok(false);
        }
        let min_iv = cfg.min_job_interval_secs;
        if min_iv > 0 && now.saturating_sub(ex.last_acquire_unix) < min_iv {
            eprintln!(
                "scopey guard: can_spawn=false session={session_id} throttle {}s remaining",
                min_iv.saturating_sub(now.saturating_sub(ex.last_acquire_unix))
            );
            return Ok(false);
        }
        Ok(true)
    }
}

impl Drop for SessionJobGuard {
    fn drop(&mut self) {
        if self.held {
            release_lock(&self.path, &self.session_id, std::process::id());
            self.held = false;
        }
    }
}

fn release_lock(path: &Path, session_id: &str, expected_pid: u32) {
    let Ok(mut file) = OpenOptions::new().read(true).write(true).open(path) else {
        return;
    };
    use fs2::FileExt;
    let _ = file.lock_exclusive();
    let mut buf = String::new();
    let _ = file.read_to_string(&mut buf);
    if let Ok(mut ex) = serde_json::from_str::<JobLock>(&buf) {
        if ex.pid == expected_pid {
            // Keep last_acquire_unix for throttle; clear live pid by writing dead marker
            // Use pid 0 to mean free but remember throttle time.
            ex.pid = 0;
            ex.kind = format!("released:{}", ex.kind);
            if let Ok(json) = serde_json::to_string_pretty(&ex) {
                let _ = file.set_len(0);
                use std::io::Seek;
                let _ = file.seek(std::io::SeekFrom::Start(0));
                let _ = file.write_all(json.as_bytes());
                let _ = file.sync_all();
            }
            crate::eventlog::info(
                session_id,
                "guard.release",
                "released job lock",
                serde_json::json!({ "pid": expected_pid }),
            );
        }
    }
    let _ = file.unlock();
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // signal 0: existence check
        let r = unsafe { libc::kill(pid as i32, 0) };
        if r == 0 {
            return true;
        }
        // EPERM means process exists but we can't signal it
        let err = std::io::Error::last_os_error().raw_os_error();
        return err == Some(libc::EPERM);
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Internal prompt fingerprints that must never be stored as user prompts.
pub fn looks_like_scopey_internal_prompt(prompt: &str) -> bool {
    let p = prompt.trim_start();
    p.starts_with("You are a scope analyst")
        || p.starts_with("You are a strict scope auditor")
        || p.contains("SCOPE REQUIREMENTS checklist")
        || p.contains("[scopey COURSE CORRECTION")
        || p.contains("[scopey SCOPE REMINDER")
        || std::env::var(ENV_INTERNAL).is_ok()
}

/// Kill leaked scopey background workers and orphaned claude -p/codex from storms.
/// Conservative: only matches known scopey log patterns / SCOPEY_INTERNAL children is hard;
/// we kill processes whose command line is `scopey summarize|judge` or claude -p with scope prompts.
pub fn purge_leaked_jobs() -> Result<usize> {
    #[cfg(unix)]
    {
        let mut killed = 0usize;
        // scopey summarize / judge background
        let out = std::process::Command::new("pgrep")
            .args(["-fl", "scopey"])
            .output();
        if let Ok(o) = out {
            let text = String::from_utf8_lossy(&o.stdout);
            for line in text.lines() {
                if line.contains(" summarize") || line.contains(" judge") {
                    if let Some(pid_s) = line.split_whitespace().next() {
                        if let Ok(pid) = pid_s.parse::<i32>() {
                            if pid as u32 != std::process::id() {
                                let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
                                killed += 1;
                            }
                        }
                    }
                }
            }
        }
        // recursive storm: claude -p with our analyst prompt
        let out2 = std::process::Command::new("pgrep")
            .args(["-fl", "claude"])
            .output();
        if let Ok(o) = out2 {
            let text = String::from_utf8_lossy(&o.stdout);
            for line in text.lines() {
                if line.contains("scope analyst")
                    || line.contains("strict scope auditor")
                    || line.contains("SCOPE REQUIREMENTS checklist")
                {
                    if let Some(pid_s) = line.split_whitespace().next() {
                        if let Ok(pid) = pid_s.parse::<i32>() {
                            let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
                            killed += 1;
                        }
                    }
                }
            }
        }
        // clear stale locks with dead pids
        let lock_dir = Config::scopey_home().join("locks");
        if lock_dir.is_dir() {
            if let Ok(rd) = fs::read_dir(&lock_dir) {
                for e in rd.flatten() {
                    if let Ok(text) = fs::read_to_string(e.path()) {
                        if let Ok(lock) = serde_json::from_str::<JobLock>(&text) {
                            if lock.pid != 0 && !pid_alive(lock.pid) {
                                let _ = fs::remove_file(e.path());
                            }
                        }
                    }
                }
            }
        }
        Ok(killed)
    }
    #[cfg(not(unix))]
    {
        bail!("purge only implemented on unix");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn internal_prompt_detect() {
        assert!(looks_like_scopey_internal_prompt(
            "You are a scope analyst for a coding agent session."
        ));
        assert!(looks_like_scopey_internal_prompt(
            "You are a strict scope auditor for a coding agent."
        ));
        assert!(looks_like_scopey_internal_prompt(
            "  You are a scope analyst ..."
        ));
        assert!(looks_like_scopey_internal_prompt(
            "x\nSCOPE REQUIREMENTS checklist\n"
        ));
        assert!(!looks_like_scopey_internal_prompt("fix the login bug"));
        assert!(!looks_like_scopey_internal_prompt("refactor billing"));
    }

    #[test]
    fn env_truthy_variants() {
        Config::with_temp_scopey_home(|_| {
            // Serialize env mutations via the same global lock as SCOPEY_HOME.
            std::env::remove_var(ENV_INTERNAL);
            std::env::remove_var(ENV_HOOKS_DISABLED);
            assert!(!is_internal());
            std::env::set_var(ENV_INTERNAL, "1");
            assert!(is_internal());
            std::env::set_var(ENV_INTERNAL, "true");
            assert!(is_internal());
            std::env::set_var(ENV_INTERNAL, "yes");
            assert!(is_internal());
            std::env::set_var(ENV_INTERNAL, "0");
            assert!(!is_internal());
            std::env::remove_var(ENV_INTERNAL);
            std::env::set_var(ENV_HOOKS_DISABLED, "1");
            assert!(is_internal());
            std::env::remove_var(ENV_HOOKS_DISABLED);
        });
    }

    #[test]
    fn apply_internal_env_sets_keys() {
        let mut cmd = std::process::Command::new("true");
        apply_internal_env(&mut cmd);
        let mut cmd2 = std::process::Command::new("true");
        apply_hook_disable_env(&mut cmd2);
    }

    #[test]
    fn single_flight_blocks_second_acquire() {
        Config::with_temp_scopey_home(|_| {
            let mut cfg = Config::default();
            cfg.min_job_interval_secs = 0;
            let sid = "sess-single-flight";
            let g1 = SessionJobGuard::try_acquire(&cfg, sid, "summarize")
                .unwrap()
                .expect("first acquire");
            assert!(!SessionJobGuard::can_spawn(&cfg, sid).unwrap());
            let g2 = SessionJobGuard::try_acquire(&cfg, sid, "judge").unwrap();
            assert!(g2.is_none(), "second acquire must fail while first holds");
            drop(g1);
            // min_iv=0 → can acquire again after release
            let g3 = SessionJobGuard::try_acquire(&cfg, sid, "judge")
                .unwrap()
                .expect("after release");
            drop(g3);
        });
    }

    #[test]
    fn throttle_blocks_reacquire_when_interval_set() {
        Config::with_temp_scopey_home(|_| {
            let mut cfg = Config::default();
            cfg.min_job_interval_secs = 3600;
            let sid = "sess-throttle";
            let g1 = SessionJobGuard::try_acquire(&cfg, sid, "summarize")
                .unwrap()
                .expect("first");
            drop(g1);
            let g2 = SessionJobGuard::try_acquire(&cfg, sid, "judge").unwrap();
            assert!(
                g2.is_none(),
                "throttle should refuse immediately after release"
            );
            assert!(!SessionJobGuard::can_spawn(&cfg, sid).unwrap());
        });
    }

    #[test]
    fn can_spawn_true_when_no_lock() {
        Config::with_temp_scopey_home(|_| {
            let cfg = Config::default();
            assert!(SessionJobGuard::can_spawn(&cfg, "never-seen").unwrap());
        });
    }
}

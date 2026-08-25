//! Herdr awareness: detection, in-app notifications, optional pane annotations.
//!
//! Herdr is an agent multiplexer with a socket API. When a process runs inside a
//! Herdr pane it typically has:
//!   HERDR_ENV=1
//!   HERDR_SOCKET_PATH=…
//!   HERDR_PANE_ID=w1:p2
//!
//! Notification CLI (v0.7+):
//!   `herdr notification show <title> [--body TEXT]`
//!     [--position top-left|top-right|bottom-left|bottom-right]
//!     [--sound none|done|request]
//!
//! Toast delivery is configured in Herdr itself under `[ui.toast] delivery`:
//!   off | herdr | terminal | system
//!
//! Display-only overlay (default — does not rename the real agent or take
//! lifecycle authority):
//!   `herdr pane report-metadata <pane_id> --source scopey`
//!     --state-label blocked=scope --token scopey=off_track [--ttl-ms N]
//!
//! Lifecycle reporting (opt-in only; must use the real agent label, never
//! hardcode "scopey" as --agent):
//!   `herdr pane report-agent <pane_id> --source scopey --agent <codex|…>`
//!     --state idle|working|blocked|unknown [--message TEXT]

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Token name written via `report-metadata` for scopey attention.
pub const METADATA_TOKEN: &str = "scopey";
/// `state_labels` key used when scopey wants a blocked annotation.
pub const METADATA_BLOCKED_LABEL_KEY: &str = "blocked";
/// Default value for the blocked state label (short sidebar text).
pub const METADATA_BLOCKED_LABEL_VALUE: &str = "scope";
/// Default metadata TTL: 5 minutes (Herdr auto-expires overlay).
pub const DEFAULT_METADATA_TTL_MS: u64 = 300_000;

/// Runtime view of whether we're inside / can talk to Herdr.
#[derive(Debug, Clone)]
pub struct HerdrContext {
    pub env_flag: bool,
    pub socket_path: Option<PathBuf>,
    pub pane_id: Option<String>,
    pub binary: Option<PathBuf>,
    pub server_running: bool,
}

impl HerdrContext {
    pub fn detect() -> Self {
        let env_flag = env::var("HERDR_ENV").ok().as_deref() == Some("1")
            || env::var("HERDR_ENV")
                .ok()
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
        let socket_path = env::var_os("HERDR_SOCKET_PATH")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(default_socket_path);
        let pane_id = env::var("HERDR_PANE_ID").ok().filter(|s| !s.is_empty());
        let binary = which::which("herdr").ok();
        let server_running = socket_path.as_ref().map(|p| p.exists()).unwrap_or(false)
            || binary
                .as_ref()
                .map(|b| {
                    Command::new(b)
                        .args(["status"])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false)
                })
                .unwrap_or(false);

        Self {
            env_flag,
            socket_path,
            pane_id,
            binary,
            server_running,
        }
    }

    /// Inside a Herdr-managed pane (hooks/agents running under Herdr).
    pub fn inside_pane(&self) -> bool {
        self.env_flag || self.pane_id.is_some()
    }

    /// Can we invoke `herdr notification show` usefully?
    pub fn can_notify(&self) -> bool {
        self.binary.is_some() && (self.server_running || self.inside_pane())
    }

    pub fn summary_line(&self) -> String {
        format!(
            "inside_pane={} pane_id={} socket={} binary={} server_running={}",
            self.inside_pane(),
            self.pane_id.as_deref().unwrap_or("-"),
            self.socket_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "-".into()),
            self.binary
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "missing".into()),
            self.server_running,
        )
    }
}

fn default_socket_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let p = home.join(".config/herdr/herdr.sock");
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

fn herdr_bin() -> Result<PathBuf> {
    which::which("herdr").context("herdr not on PATH")
}

/// Pane id for report/clear calls. Reads `HERDR_PANE_ID` only — never runs
/// `herdr status` / full detect. Hook hot paths (user-prompt clear) call this
/// on every prompt; probing a stuck Herdr CLI outside a pane would hang the
/// harness.
fn current_pane_id() -> Option<String> {
    env::var("HERDR_PANE_ID").ok().filter(|s| !s.is_empty())
}

/// Map scopey config sound / verdict to Herdr's sound enum: none|done|request.
///
/// Off-track / warning default to **request** (needs-attention). Unknown OS
/// sound names (default, Glass, …) also map to request for attention alerts.
pub fn herdr_sound_for(verdict: &str, configured: Option<&str>) -> &'static str {
    if let Some(s) = configured {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "silent" | "" => return "none",
            "done" => return "done",
            "request" | "attention" | "blocked" => return "request",
            // OS sound names / "default" → attention ping for alerts
            _ if matches!(verdict, "off_track" | "warning") => return "request",
            _ => {}
        }
    }
    match verdict {
        "off_track" | "warning" => "request",
        "on_track" => "done",
        _ => "none",
    }
}

/// Show an in-Herdr (or Herdr-routed desktop) notification.
///
/// Returns Ok(true) if Herdr reported the toast as shown, Ok(false) if the
/// server accepted the call but delivery is disabled (`shown: false`).
pub fn notification_show(
    title: &str,
    body: &str,
    sound: &str,
    position: Option<&str>,
) -> Result<bool> {
    let bin = herdr_bin()?;
    let mut cmd = Command::new(bin);
    cmd.arg("notification")
        .arg("show")
        .arg(title)
        .arg("--body")
        .arg(body)
        .arg("--sound")
        .arg(sound);
    if let Some(pos) = position.filter(|p| !p.is_empty()) {
        cmd.arg("--position").arg(pos);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = cmd.output().context("spawn herdr notification show")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!(
            "herdr notification show failed ({}): {}{}",
            output.status,
            stdout.trim(),
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!(" stderr={}", stderr.trim())
            }
        );
    }

    // Response is JSON like:
    // {"id":"cli:notification:show","result":{"reason":"disabled","shown":false,"type":"notification_show"}}
    if let Ok(v) = serde_json::from_str::<Value>(stdout.trim()) {
        let shown = v
            .pointer("/result/shown")
            .and_then(|x| x.as_bool())
            .unwrap_or(true);
        let reason = v
            .pointer("/result/reason")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if !shown {
            eprintln!(
                "scopey herdr: notification not shown (reason={reason}). \
                 Check [ui.toast] delivery in ~/.config/herdr/config.toml \
                 (herdr|terminal|system; not off)."
            );
        }
        return Ok(shown);
    }
    Ok(true)
}

/// Resolve the real agent label for the current (or given) pane.
///
/// Prefers `pane get` → `agent`, then foreground process name. Never invents
/// "scopey" — that would hijack the sidebar identity.
pub fn resolve_agent_label(pane_id: Option<&str>) -> Option<String> {
    let pane = pane_id.map(|s| s.to_string()).or_else(current_pane_id)?;
    let bin = herdr_bin().ok()?;

    if let Ok(output) = Command::new(&bin)
        .args(["pane", "get", &pane])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        if output.status.success() {
            if let Ok(v) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&output.stdout)) {
                if let Some(agent) = v
                    .pointer("/result/pane/agent")
                    .and_then(|x| x.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("scopey"))
                {
                    return Some(agent.to_string());
                }
            }
        }
    }

    if let Ok(output) = Command::new(&bin)
        .args(["pane", "process-info", "--pane", &pane])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        if output.status.success() {
            if let Ok(v) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&output.stdout)) {
                if let Some(procs) = v
                    .pointer("/result/process_info/foreground_processes")
                    .and_then(|x| x.as_array())
                {
                    for p in procs {
                        // Prefer argv0 (stable "grok") over process name
                        // ("grok-0.2.118-ma") so lifecycle --agent matches Herdr.
                        let raw = p
                            .get("argv0")
                            .and_then(|x| x.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .or_else(|| {
                                p.get("name")
                                    .and_then(|x| x.as_str())
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                            });
                        if let Some(raw) = raw {
                            if let Some(label) = normalize_process_agent_label(raw) {
                                return Some(label);
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

/// Map a foreground process name/argv0 to a Herdr agent label.
///
/// Returns None for empty / "scopey". Recognizes versioned binaries such as
/// `grok-0.2.118-ma` → `grok`.
pub fn normalize_process_agent_label(raw: &str) -> Option<String> {
    let base = std::path::Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(raw)
        .trim();
    if base.is_empty() {
        return None;
    }
    let lower = base.to_ascii_lowercase();
    if lower == "scopey" {
        return None;
    }
    // Longer prefixes first so "opencode" wins over "open".
    const KNOWN: &[&str] = &[
        "opencode", "claude", "codex", "cursor", "hermes", "grok", "kimi", "kilo", "omp", "pi",
    ];
    for known in KNOWN {
        // Exact or `name-…` (versioned builds). Avoid bare prefix so "pineapple" ≠ "pi".
        if lower == *known || lower.starts_with(&format!("{known}-")) {
            return Some((*known).to_string());
        }
    }
    Some(lower)
}

/// Mark scope attention on the current pane via **display-only** metadata.
///
/// Does not take lifecycle authority or rename the real agent (codex/claude/…).
/// Herdr designed `report-metadata` for user hooks / overlays exactly like this.
pub fn report_scope_attention(source: &str, verdict: &str, ttl_ms: u64) -> Result<()> {
    let pane_id = match current_pane_id() {
        Some(p) => p,
        None => {
            eprintln!("scopey herdr: no HERDR_PANE_ID; skip report-metadata");
            return Ok(());
        }
    };
    let bin = herdr_bin()?;
    let token_value = if verdict.is_empty() {
        "attention".to_string()
    } else {
        verdict.to_string()
    };
    let mut cmd = Command::new(bin);
    cmd.arg("pane")
        .arg("report-metadata")
        .arg(&pane_id)
        .arg("--source")
        .arg(source)
        .arg("--state-label")
        .arg(format!(
            "{METADATA_BLOCKED_LABEL_KEY}={METADATA_BLOCKED_LABEL_VALUE}"
        ))
        .arg("--token")
        .arg(format!("{METADATA_TOKEN}={token_value}"));
    if ttl_ms > 0 {
        cmd.arg("--ttl-ms").arg(ttl_ms.to_string());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = cmd.output().context("spawn herdr pane report-metadata")?;
    if !output.status.success() {
        bail!(
            "herdr pane report-metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Clear scopey's display-only metadata on the current pane (recovery).
pub fn clear_scope_attention(source: &str) -> Result<()> {
    let pane_id = match current_pane_id() {
        Some(p) => p,
        None => return Ok(()),
    };
    let bin = herdr_bin()?;
    let mut cmd = Command::new(bin);
    cmd.arg("pane")
        .arg("report-metadata")
        .arg(&pane_id)
        .arg("--source")
        .arg(source)
        .arg("--clear-state-labels")
        .arg("--clear-token")
        .arg(METADATA_TOKEN);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = cmd
        .output()
        .context("spawn herdr pane report-metadata clear")?;
    if !output.status.success() {
        bail!(
            "herdr pane report-metadata clear failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Opt-in lifecycle report. Caller must pass the **real** agent label (codex/…).
///
/// Prefer `report_scope_attention` for normal use. Lifecycle authority
/// suppresses screen detection until released or re-reported.
pub fn report_agent_state(
    state: &str,
    message: &str,
    source: &str,
    agent_label: &str,
) -> Result<()> {
    if agent_label.trim().is_empty() {
        bail!("report-agent requires a non-empty agent label (use resolve_agent_label)");
    }
    if agent_label.eq_ignore_ascii_case("scopey") {
        bail!(
            "refusing to report-agent with --agent scopey (hijacks pane identity); \
             pass the real harness label"
        );
    }
    let pane_id = match current_pane_id() {
        Some(p) => p,
        None => {
            eprintln!("scopey herdr: no HERDR_PANE_ID; skip report-agent");
            return Ok(());
        }
    };
    let bin = herdr_bin()?;
    let mut cmd = Command::new(bin);
    cmd.arg("pane")
        .arg("report-agent")
        .arg(&pane_id)
        .arg("--source")
        .arg(source)
        .arg("--agent")
        .arg(agent_label)
        .arg("--state")
        .arg(state);
    if !message.is_empty() {
        cmd.arg("--message").arg(message);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = cmd.output().context("spawn herdr pane report-agent")?;
    if !output.status.success() {
        bail!(
            "herdr pane report-agent failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Release lifecycle authority previously taken by `report_agent_state`.
pub fn release_agent(source: &str, agent_label: &str) -> Result<()> {
    let pane_id = match current_pane_id() {
        Some(p) => p,
        None => return Ok(()),
    };
    let bin = herdr_bin()?;
    let mut cmd = Command::new(bin);
    cmd.arg("pane")
        .arg("release-agent")
        .arg(&pane_id)
        .arg("--source")
        .arg(source)
        .arg("--agent")
        .arg(agent_label);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = cmd.output().context("spawn herdr pane release-agent")?;
    if !output.status.success() {
        bail!(
            "herdr pane release-agent failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Best-effort recovery after scope attention: clear metadata, and if lifecycle
/// was used, **release** authority so Herdr screen detection can resume.
pub fn clear_scope_attention_full(
    source: &str,
    lifecycle: bool,
    agent_label_override: Option<&str>,
) -> Result<()> {
    if let Err(e) = clear_scope_attention(source) {
        eprintln!("scopey herdr: clear metadata failed: {e:#}");
    }
    if !lifecycle {
        return Ok(());
    }
    let label = agent_label_override
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("scopey"))
        .map(|s| s.to_string())
        .or_else(|| resolve_agent_label(None));
    if let Some(agent) = label {
        // Release, don't just re-report working: working leaves scopey holding
        // lifecycle authority and keeps screen detection suppressed.
        if let Err(e) = release_agent(source, &agent) {
            eprintln!("scopey herdr: release-agent failed: {e:#}");
        }
    } else {
        eprintln!("scopey herdr: lifecycle release skipped (could not resolve real agent label)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_mapping() {
        assert_eq!(herdr_sound_for("off_track", None), "request");
        assert_eq!(herdr_sound_for("warning", None), "request");
        assert_eq!(herdr_sound_for("on_track", None), "done");
        assert_eq!(herdr_sound_for("warning", Some("done")), "done");
        assert_eq!(herdr_sound_for("on_track", Some("none")), "none");
        assert_eq!(herdr_sound_for("off_track", Some("silent")), "none");
        assert_eq!(herdr_sound_for("off_track", Some("request")), "request");
        assert_eq!(herdr_sound_for("off_track", Some("Glass")), "request");
        assert_eq!(herdr_sound_for("off_track", Some("default")), "request");
        assert_eq!(herdr_sound_for("warning", Some("default")), "request");
        assert_eq!(herdr_sound_for("unknown", None), "none");
    }

    #[test]
    fn detect_does_not_panic() {
        let h = HerdrContext::detect();
        let _ = h.summary_line();
        let _ = h.can_notify();
        let _ = h.inside_pane();
    }

    #[test]
    fn refuse_scopey_as_lifecycle_agent() {
        let err = report_agent_state("blocked", "x", "scopey", "scopey")
            .expect_err("must refuse --agent scopey");
        assert!(
            err.to_string().contains("hijacks pane identity"),
            "unexpected err: {err:#}"
        );
        let empty = report_agent_state("blocked", "x", "scopey", "  ")
            .expect_err("must refuse empty agent");
        assert!(
            empty.to_string().contains("non-empty"),
            "unexpected err: {empty:#}"
        );
    }

    #[test]
    fn normalize_process_agent_label_handles_versioned_binaries() {
        assert_eq!(
            normalize_process_agent_label("grok-0.2.118-ma").as_deref(),
            Some("grok")
        );
        assert_eq!(
            normalize_process_agent_label("grok").as_deref(),
            Some("grok")
        );
        assert_eq!(
            normalize_process_agent_label("/opt/homebrew/bin/codex").as_deref(),
            Some("codex")
        );
        assert_eq!(normalize_process_agent_label("scopey"), None);
        assert_eq!(
            normalize_process_agent_label("pineapple").as_deref(),
            Some("pineapple")
        );
    }
}

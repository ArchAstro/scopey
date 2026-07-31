//! Herdr awareness: detection, in-app notifications, optional pane state reports.
//!
//! Herdr is an agent multiplexer with a socket API. When a process runs inside a
//! Herdr pane it typically has:
//!   HERDR_ENV=1
//!   HERDR_SOCKET_PATH=…
//!   HERDR_PANE_ID=w1:p2
//!
//! Notification CLI (v0.7+):
//!   herdr notification show <title> [--body TEXT]
//!     [--position top-left|top-right|bottom-left|bottom-right]
//!     [--sound none|done|request]
//!
//! Toast delivery is configured in Herdr itself under `[ui.toast] delivery`:
//!   off | herdr | terminal | system
//!
//! State reporting (affects sidebar rollups / waits / Herdr-native alerts):
//!   herdr pane report-agent <pane_id> --source scopey --agent scopey
//!     --state idle|working|blocked|unknown [--message TEXT]

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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
    let bin = which::which("herdr").context("herdr not on PATH")?;
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

/// Mark the current Herdr pane's agent state (blocked/working/idle).
///
/// No-op (Ok) when not inside a pane. Failures are returned for the caller to log.
pub fn report_agent_state(
    state: &str,
    message: &str,
    source: &str,
    agent_label: &str,
) -> Result<()> {
    let ctx = HerdrContext::detect();
    let pane_id = match ctx.pane_id {
        Some(ref p) => p.clone(),
        None => {
            eprintln!("scopey herdr: no HERDR_PANE_ID; skip report-agent");
            return Ok(());
        }
    };
    let bin = which::which("herdr").context("herdr not on PATH")?;
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
}

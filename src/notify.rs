use crate::config::Config;
use crate::herdr::{self, HerdrContext};
use crate::session::JudgementVerdict;
use anyhow::{bail, Context, Result};
use std::process::Command;

/// Context for building a notification from templates.
#[derive(Debug, Clone)]
pub struct NotifyContext<'a> {
    pub verdict: &'a JudgementVerdict,
    pub summary: &'a str,
    pub details: &'a str,
    pub session_id: &'a str,
    pub cwd: &'a str,
    pub from_count: u64,
    pub to_count: u64,
    pub harness: &'a str,
}

impl NotifyContext<'_> {
    pub fn verdict_label(&self) -> &'static str {
        match self.verdict {
            JudgementVerdict::OnTrack => "on_track",
            JudgementVerdict::Warning => "warning",
            JudgementVerdict::OffTrack => "off_track",
            JudgementVerdict::InsufficientEvidence => "insufficient_evidence",
            JudgementVerdict::Unknown => "unknown",
        }
    }

    fn expand(&self, template: &str) -> String {
        template
            .replace("{verdict}", self.verdict_label())
            .replace("{summary}", self.summary)
            .replace("{details}", self.details)
            .replace("{session_id}", self.session_id)
            .replace("{cwd}", self.cwd)
            .replace("{from_count}", &self.from_count.to_string())
            .replace("{to_count}", &self.to_count.to_string())
            .replace("{harness}", self.harness)
            .replace("{session}", self.session_id)
    }
}

/// Build title/body from config templates and deliver via configured backend.
pub fn notify_judgement(cfg: &Config, ctx: &NotifyContext<'_>) -> Result<()> {
    let title_tmpl = match ctx.verdict {
        JudgementVerdict::OffTrack => cfg.notify_title_off_track.as_str(),
        JudgementVerdict::Warning => cfg.notify_title_warning.as_str(),
        _ => "scopey",
    };
    let title = ctx.expand(title_tmpl);
    let body = ctx.expand(&cfg.notify_body);
    let sound = cfg.notify_sound.as_deref().filter(|s| !s.is_empty());

    // Optional: tell Herdr this pane is blocked / needs attention.
    if cfg.herdr_report_state {
        let state = match ctx.verdict {
            JudgementVerdict::OffTrack | JudgementVerdict::Warning => "blocked",
            JudgementVerdict::OnTrack => "working",
            JudgementVerdict::InsufficientEvidence | JudgementVerdict::Unknown => "unknown",
        };
        let msg = format!("{}: {}", ctx.verdict_label(), ctx.summary);
        if let Err(e) =
            herdr::report_agent_state(state, &msg, &cfg.herdr_source, &cfg.herdr_agent_label)
        {
            eprintln!("scopey herdr: report-agent failed: {e:#}");
        }
    }

    deliver(cfg, &title, &body, sound, ctx)
}

fn deliver(
    cfg: &Config,
    title: &str,
    body: &str,
    sound: Option<&str>,
    ctx: &NotifyContext<'_>,
) -> Result<()> {
    let backend = cfg.notify_backend.trim().to_ascii_lowercase();

    // Highest priority: explicit notify_command when backend is command or auto with command set.
    if backend == "command" {
        let tmpl = cfg
            .notify_command
            .as_deref()
            .context("notify_backend=command requires notify_command")?;
        return notify_custom(tmpl, title, body, ctx, sound);
    }

    if backend == "herdr" {
        return deliver_herdr(cfg, title, body, sound, ctx);
    }

    if backend == "os" || backend == "system" {
        return notify_os(title, body, sound);
    }

    // auto:
    // 1. notify_command if set (user override)
    // 2. herdr when available (inside pane or server up)
    // 3. OS desktop
    if let Some(ref tmpl) = cfg.notify_command {
        return notify_custom(tmpl, title, body, ctx, sound);
    }

    let hctx = HerdrContext::detect();
    if hctx.can_notify() {
        match deliver_herdr(cfg, title, body, sound, ctx) {
            Ok(()) => return Ok(()),
            Err(e) => {
                eprintln!("scopey herdr notify failed ({e:#}); falling back to OS");
            }
        }
    }

    notify_os(title, body, sound)
}

fn deliver_herdr(
    cfg: &Config,
    title: &str,
    body: &str,
    sound_override: Option<&str>,
    ctx: &NotifyContext<'_>,
) -> Result<()> {
    // Off-track / warning always use Herdr `request` (needs-attention) unless the
    // caller explicitly chose none/done/request via herdr_notify_sound or --sound.
    let explicit = cfg
        .herdr_notify_sound
        .as_deref()
        .or(sound_override)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let sound = match ctx.verdict {
        JudgementVerdict::OffTrack | JudgementVerdict::Warning => match explicit {
            Some(s) => match s.to_ascii_lowercase().as_str() {
                "none" | "off" | "silent" => "none",
                "done" => "done",
                // request, attention, blocked, default, Glass, … → attention ping
                _ => "request",
            },
            None => "request",
        },
        _ => herdr::herdr_sound_for(
            ctx.verdict_label(),
            explicit.or(cfg.notify_sound.as_deref()),
        ),
    };
    let position = cfg.herdr_notify_position.as_deref();
    let shown = herdr::notification_show(title, body, sound, position)?;
    if !shown && cfg.notify_fallback_os_if_herdr_disabled {
        eprintln!("scopey: herdr toast disabled; falling back to OS notification");
        let os_sound = sound_override
            .or(cfg.notify_sound.as_deref())
            .filter(|s| !s.is_empty());
        return notify_os(title, body, os_sound);
    }
    if shown {
        eprintln!("scopey notify: herdr toast shown (sound={sound})");
    } else {
        eprintln!(
            "scopey notify: herdr accepted but toast not shown (delivery busy/disabled; no OS fallback)"
        );
    }
    Ok(())
}

/// Fire a notification using the same backend rules as off-track alerts.
///
/// When `notify_backend` is `auto`/`herdr` and Herdr is available (inside a
/// pane or server up), calls `herdr notification show` with sound **request**.
/// Otherwise uses OS desktop notify (osascript on macOS).
pub fn notify(cfg: &Config, title: &str, body: &str, sound: Option<&str>) -> Result<()> {
    // CLI notifies are attention alerts → Herdr sound "request" by default.
    let verdict = JudgementVerdict::OffTrack;
    let ctx = NotifyContext {
        verdict: &verdict,
        summary: body,
        details: "",
        session_id: "cli",
        cwd: "",
        from_count: 0,
        to_count: 0,
        harness: "cli",
    };
    let sound = sound.or(Some("request"));
    deliver(cfg, title, body, sound, &ctx)
}

fn notify_os(title: &str, body: &str, sound: Option<&str>) -> Result<()> {
    if cfg!(target_os = "macos") {
        notify_macos(title, body, sound)
    } else if cfg!(target_os = "linux") {
        notify_linux(title, body)
    } else if cfg!(target_os = "windows") {
        notify_windows(title, body)
    } else {
        eprintln!("scopey notify: unsupported platform; {title}: {body}");
        Ok(())
    }
}

fn notify_custom(
    tmpl: &str,
    title: &str,
    body: &str,
    ctx: &NotifyContext<'_>,
    sound: Option<&str>,
) -> Result<()> {
    let cmd = tmpl
        .replace("{title}", title)
        .replace("{body}", body)
        .replace("{verdict}", ctx.verdict_label())
        .replace("{summary}", ctx.summary)
        .replace("{details}", ctx.details)
        .replace("{session_id}", ctx.session_id)
        .replace("{session}", ctx.session_id)
        .replace("{cwd}", ctx.cwd)
        .replace("{from_count}", &ctx.from_count.to_string())
        .replace("{to_count}", &ctx.to_count.to_string())
        .replace("{harness}", ctx.harness)
        .replace("{sound}", sound.unwrap_or(""));

    let status = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .status()
        .with_context(|| format!("spawn notify_command: {cmd}"))?;
    if !status.success() {
        bail!("notify_command failed ({status}): {cmd}");
    }
    Ok(())
}

fn notify_macos(title: &str, body: &str, sound: Option<&str>) -> Result<()> {
    let title_e = escape_applescript(title);
    let body_e = escape_applescript(body);
    let script = if let Some(s) = sound {
        let sound_e = escape_applescript(s);
        format!(r#"display notification "{body_e}" with title "{title_e}" sound name "{sound_e}""#)
    } else {
        format!(r#"display notification "{body_e}" with title "{title_e}""#)
    };
    let status = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .status()
        .context("spawn osascript")?;
    if !status.success() {
        bail!("osascript failed: {status}");
    }
    Ok(())
}

fn notify_linux(title: &str, body: &str) -> Result<()> {
    let status = Command::new("notify-send")
        .arg(title)
        .arg(body)
        .status()
        .context("spawn notify-send (install libnotify)")?;
    if !status.success() {
        bail!("notify-send failed: {status}");
    }
    Ok(())
}

fn notify_windows(title: &str, body: &str) -> Result<()> {
    let ps = format!(
        r#"Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.MessageBox]::Show('{body}', '{title}') | Out-Null"#,
        title = title.replace('\'', "''"),
        body = body.replace('\'', "''"),
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps])
        .status()
        .context("spawn powershell")?;
    if !status.success() {
        bail!("powershell notify failed: {status}");
    }
    Ok(())
}

fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(v: &'a JudgementVerdict) -> NotifyContext<'a> {
        NotifyContext {
            verdict: v,
            summary: "drifted",
            details: "edited main.rs",
            session_id: "abc",
            cwd: "/tmp/p",
            from_count: 0,
            to_count: 10,
            harness: "claude",
        }
    }

    #[test]
    fn expand_placeholders() {
        let v = JudgementVerdict::OffTrack;
        let c = ctx(&v);
        assert_eq!(
            c.expand("[{verdict}] {summary} @ {session_id}"),
            "[off_track] drifted @ abc"
        );
        assert_eq!(c.expand("{session}/{from_count}-{to_count}"), "abc/0-10");
        assert_eq!(c.expand("{harness}:{cwd}"), "claude:/tmp/p");
        assert!(c.expand("{details}").contains("main.rs"));
    }

    #[test]
    fn verdict_labels() {
        assert_eq!(
            ctx(&JudgementVerdict::OffTrack).verdict_label(),
            "off_track"
        );
        assert_eq!(ctx(&JudgementVerdict::Warning).verdict_label(), "warning");
        assert_eq!(ctx(&JudgementVerdict::OnTrack).verdict_label(), "on_track");
        assert_eq!(ctx(&JudgementVerdict::Unknown).verdict_label(), "unknown");
    }

    #[test]
    fn title_template_selection_uses_config() {
        let cfg = Config {
            notify_title_off_track: "OFF {verdict}".into(),
            notify_title_warning: "WARN {summary}".into(),
            notify_body: "body {session_id}".into(),
            notify_backend: "command".into(),
            notify_command: Some("true".into()),
            herdr_report_state: false,
            ..Config::default()
        };

        let v = JudgementVerdict::OffTrack;
        let c = ctx(&v);
        notify_judgement(&cfg, &c).unwrap();

        let v2 = JudgementVerdict::Warning;
        let c2 = ctx(&v2);
        notify_judgement(&cfg, &c2).unwrap();
    }

    #[test]
    fn notify_cli_uses_config_backend_not_raw_os() {
        let cfg = Config {
            notify_backend: "command".into(),
            notify_command: Some("true".into()),
            herdr_report_state: false,
            ..Config::default()
        };
        // Would fail if this still always went to osascript-only path without config.
        notify(&cfg, "t", "b", None).unwrap();
    }
}

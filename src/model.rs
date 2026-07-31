use crate::config::Config;
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::NamedTempFile;

/// Which product CLI runs summarize/judge jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    Claude,
    Codex,
    Grok,
    Pi,
    OpenCode,
}

impl Runner {
    pub fn as_str(self) -> &'static str {
        match self {
            Runner::Claude => "claude",
            Runner::Codex => "codex",
            Runner::Grok => "grok",
            Runner::Pi => "pi",
            Runner::OpenCode => "opencode",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "anthropic" => Some(Runner::Claude),
            "codex" | "openai" => Some(Runner::Codex),
            "grok" | "grok-build" | "xai" => Some(Runner::Grok),
            "pi" => Some(Runner::Pi),
            "opencode" | "open-code" | "open_code" => Some(Runner::OpenCode),
            _ => None,
        }
    }

    fn binary_names(self) -> &'static [&'static str] {
        match self {
            Runner::Claude => &["claude"],
            Runner::Codex => &["codex"],
            Runner::Grok => &["grok"],
            Runner::Pi => &["pi"],
            Runner::OpenCode => &["opencode"],
        }
    }
}

/// Resolved runner + model for one completion call.
#[derive(Debug, Clone)]
pub struct ModelChoice {
    pub runner: Runner,
    pub model: String,
    /// Why this pair was chosen (for doctor / logs).
    pub reason: String,
}

/// Pick the CLI harness that should run the lightweight model job.
///
/// Priority:
/// 1. Explicit `model_runner` when not `auto`
/// 2. Session harness when known and binary exists
/// 3. `default_harness` if that binary exists
/// 4. First available among claude, codex, grok, pi, opencode
pub fn resolve_runner(cfg: &Config, session_harness: &str) -> Result<Runner> {
    let explicit = cfg.model_runner.trim().to_ascii_lowercase();
    if explicit != "auto" && !explicit.is_empty() {
        return Runner::parse(&explicit).with_context(|| {
            format!(
                "unknown model_runner {explicit:?}; use auto|claude|codex|grok|pi|opencode"
            )
        });
    }

    if let Some(r) = Runner::parse(session_harness) {
        if which_any(r.binary_names()) {
            return Ok(r);
        }
    }

    if let Some(r) = Runner::parse(&cfg.default_harness) {
        if which_any(r.binary_names()) {
            return Ok(r);
        }
    }

    for r in [
        Runner::Claude,
        Runner::Codex,
        Runner::Grok,
        Runner::Pi,
        Runner::OpenCode,
    ] {
        if which_any(r.binary_names()) {
            return Ok(r);
        }
    }
    bail!(
        "no model runner on PATH (need claude|codex|grok|pi|opencode); set model_runner explicitly"
    );
}

/// Fast model defaults for each product when `model = "auto"`.
pub fn resolve_model(cfg: &Config, runner: Runner) -> String {
    let explicit = cfg.model.trim();
    if !explicit.is_empty() && !explicit.eq_ignore_ascii_case("auto") {
        return explicit.to_string();
    }
    match runner {
        Runner::Claude => nonempty_or(&cfg.claude_fast_model, "haiku"),
        Runner::Codex => nonempty_or(&cfg.codex_fast_model, "gpt-5.6-terra"),
        Runner::Grok => nonempty_or(&cfg.grok_fast_model, "grok-3-mini"),
        Runner::Pi => cfg.pi_fast_model.trim().to_string(),
        Runner::OpenCode => cfg.opencode_fast_model.trim().to_string(),
    }
}

fn nonempty_or(s: &str, default: &str) -> String {
    let t = s.trim();
    if t.is_empty() {
        default.into()
    } else {
        t.to_string()
    }
}

pub fn resolve_choice(cfg: &Config, session_harness: &str) -> Result<ModelChoice> {
    let runner = resolve_runner(cfg, session_harness)?;
    let model = resolve_model(cfg, runner);
    let reason = format!(
        "runner={} (session_harness={:?}, model_runner={:?}); model={} (config.model={:?})",
        runner.as_str(),
        session_harness,
        cfg.model_runner,
        if model.is_empty() {
            "(harness default)"
        } else {
            model.as_str()
        },
        cfg.model
    );
    Ok(ModelChoice {
        runner,
        model,
        reason,
    })
}

fn which_ok(bin: &str) -> bool {
    which::which(bin).is_ok()
}

fn which_any(names: &[&str]) -> bool {
    names.iter().any(|n| which_ok(n))
}

/// Run the configured cheap model with a prompt; return stdout text.
pub fn complete(cfg: &Config, prompt: &str, session_harness: &str) -> Result<String> {
    let choice = resolve_choice(cfg, session_harness)?;
    eprintln!("scopey model: {}", choice.reason);

    let mut prompt_file = NamedTempFile::new().context("temp prompt file")?;
    prompt_file.write_all(prompt.as_bytes())?;
    prompt_file.flush()?;
    let prompt_path = prompt_file.path().to_path_buf();

    if let Some(ref tmpl) = cfg.model_command {
        let cmd_str = tmpl
            .replace("{model}", &choice.model)
            .replace("{prompt_file}", &prompt_path.to_string_lossy())
            .replace("{runner}", choice.runner.as_str());
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&cmd_str)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::guard::apply_internal_env(&mut cmd);
        let output = run_with_timeout(cmd, cfg.model_timeout_secs, "model_command")?;
        if !output.status.success() {
            bail!(
                "model_command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }

    match choice.runner {
        Runner::Claude => complete_claude(cfg, prompt, &choice.model),
        Runner::Codex => complete_codex(cfg, prompt, &choice.model),
        Runner::Grok => complete_grok(cfg, prompt, &choice.model),
        Runner::Pi => complete_pi(cfg, prompt, &choice.model),
        Runner::OpenCode => complete_opencode(cfg, prompt, &choice.model),
    }
}

fn complete_claude(cfg: &Config, prompt: &str, model: &str) -> Result<String> {
    use crate::guard;
    let mut cmd = Command::new("claude");
    cmd.arg("-p")
        .arg(prompt)
        .arg("--model")
        .arg(model)
        .arg("--bare")
        .arg("--output-format")
        .arg("text")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    guard::apply_internal_env(&mut cmd);
    let output = run_with_timeout(cmd, cfg.model_timeout_secs, "claude")?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }

    let mut cmd2 = Command::new("claude");
    cmd2.arg("-p")
        .arg(prompt)
        .arg("--model")
        .arg(model)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    guard::apply_internal_env(&mut cmd2);
    let output2 = run_with_timeout(cmd2, cfg.model_timeout_secs, "claude")?;
    if !output2.status.success() {
        bail!(
            "claude -p --model {model} failed: {}",
            String::from_utf8_lossy(&output2.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output2.stdout).trim().to_string())
}

fn complete_codex(cfg: &Config, prompt: &str, model: &str) -> Result<String> {
    let out_file = NamedTempFile::new().context("codex out file")?;
    let out_path = out_file.path().to_path_buf();

    let system = format!(
        "{prompt}\n\n\
         CRITICAL: Do not run tools or edit files. Reply with text only. \
         No preamble about being Codex."
    );

    use crate::guard;
    let mut cmd = Command::new("codex");
    cmd.arg("exec")
        .arg("--ephemeral")
        .arg("--skip-git-repo-check")
        .arg("-m")
        .arg(model)
        .arg("-c")
        .arg("model_reasoning_effort=\"low\"")
        .arg("--output-last-message")
        .arg(&out_path)
        .arg("--dangerously-bypass-approvals-and-sandbox")
        .arg(&system)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    guard::apply_internal_env(&mut cmd);

    let output = run_with_timeout(cmd, cfg.model_timeout_secs, "codex")?;
    let stdout = if output.status.success() {
        output.stdout
    } else {
        let mut cmd2 = Command::new("codex");
        cmd2.arg("exec")
            .arg("--ephemeral")
            .arg("--skip-git-repo-check")
            .arg("-m")
            .arg(model)
            .arg("--output-last-message")
            .arg(&out_path)
            .arg(&system)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        guard::apply_internal_env(&mut cmd2);
        let output2 = run_with_timeout(cmd2, cfg.model_timeout_secs, "codex")?;
        if !output2.status.success() {
            bail!(
                "codex exec -m {model} failed: {}",
                String::from_utf8_lossy(&output2.stderr)
            );
        }
        output2.stdout
    };

    let from_file = std::fs::read_to_string(&out_path).unwrap_or_default();
    let text = if !from_file.trim().is_empty() {
        from_file
    } else {
        String::from_utf8_lossy(&stdout).to_string()
    };
    Ok(text.trim().to_string())
}

fn complete_grok(cfg: &Config, prompt: &str, model: &str) -> Result<String> {
    use crate::guard;
    // Grok Build: `grok -p` / `--single` headless print mode.
    let mut cmd = Command::new("grok");
    cmd.arg("-p")
        .arg(prompt)
        .arg("--model")
        .arg(model)
        .arg("--always-approve")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    guard::apply_internal_env(&mut cmd);
    let output = run_with_timeout(cmd, cfg.model_timeout_secs, "grok")?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }

    let mut cmd2 = Command::new("grok");
    cmd2.arg("--single")
        .arg(prompt)
        .arg("--model")
        .arg(model)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    guard::apply_internal_env(&mut cmd2);
    let output2 = run_with_timeout(cmd2, cfg.model_timeout_secs, "grok")?;
    if !output2.status.success() {
        bail!(
            "grok -p/--single failed: {}",
            String::from_utf8_lossy(&output2.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output2.stdout).trim().to_string())
}

fn complete_pi(cfg: &Config, prompt: &str, model: &str) -> Result<String> {
    use crate::guard;
    // pi -p / --print non-interactive; --no-session avoids polluting history.
    let mut cmd = Command::new("pi");
    cmd.arg("--print")
        .arg("--no-session")
        .arg("--mode")
        .arg("text");
    if !model.is_empty() {
        cmd.arg("--model").arg(model);
    }
    cmd.arg(prompt)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    guard::apply_internal_env(&mut cmd);
    let output = run_with_timeout(cmd, cfg.model_timeout_secs, "pi")?;
    if !output.status.success() {
        bail!(
            "pi --print failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn complete_opencode(cfg: &Config, prompt: &str, model: &str) -> Result<String> {
    use crate::guard;
    // opencode run [message..]
    let mut cmd = Command::new("opencode");
    cmd.arg("run");
    if !model.is_empty() {
        // Common flags across versions; ignored if unsupported after failure retry.
        cmd.arg("--model").arg(model);
    }
    cmd.arg(prompt)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    guard::apply_internal_env(&mut cmd);
    let output = run_with_timeout(cmd, cfg.model_timeout_secs, "opencode")?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    // Retry without --model
    let mut cmd2 = Command::new("opencode");
    cmd2.arg("run")
        .arg(prompt)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    guard::apply_internal_env(&mut cmd2);
    let output2 = run_with_timeout(cmd2, cfg.model_timeout_secs, "opencode")?;
    if !output2.status.success() {
        bail!(
            "opencode run failed: {}",
            String::from_utf8_lossy(&output2.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output2.stdout).trim().to_string())
}

fn run_with_timeout(
    mut cmd: Command,
    timeout_secs: u64,
    name: &str,
) -> Result<std::process::Output> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let name_owned = name.to_string();
    std::thread::spawn(move || {
        let res = cmd.output();
        let _ = tx.send(res);
    });
    match rx.recv_timeout(Duration::from_secs(timeout_secs.max(5))) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(e).with_context(|| format!("spawn {name_owned}")),
        Err(_) => bail!("{name} timed out after {timeout_secs}s"),
    }
}

/// Print resolved defaults and optionally probe each runner with a tiny prompt.
pub fn report_models(cfg: &Config, verify: bool) -> Result<()> {
    println!("config.model_runner = {:?}", cfg.model_runner);
    println!("config.model        = {:?}", cfg.model);
    println!("config.claude_fast_model = {:?}", cfg.claude_fast_model);
    println!("config.codex_fast_model  = {:?}", cfg.codex_fast_model);
    println!("config.grok_fast_model   = {:?}", cfg.grok_fast_model);
    println!("config.pi_fast_model     = {:?}", cfg.pi_fast_model);
    println!("config.opencode_fast_model = {:?}", cfg.opencode_fast_model);
    println!("config.default_harness   = {:?}", cfg.default_harness);
    println!();

    for harness in ["claude", "codex", "grok", "pi", "opencode"] {
        match resolve_choice(cfg, harness) {
            Ok(c) => println!(
                "session_harness={harness:8} → runner={} model={}  ({})",
                c.runner.as_str(),
                if c.model.is_empty() {
                    "(default)"
                } else {
                    c.model.as_str()
                },
                if which_any(c.runner.binary_names()) {
                    "binary ok"
                } else {
                    "binary MISSING"
                }
            ),
            Err(e) => println!("session_harness={harness:8} → error: {e:#}"),
        }
    }

    if !verify {
        println!("\n(pass --verify to send a one-word probe to each available runner)");
        return Ok(());
    }

    println!("\nVerifying with probe prompt…");
    let mut failed = 0;
    let mut seen = std::collections::HashSet::new();
    for harness in ["claude", "codex", "grok", "pi", "opencode"] {
        let Ok(choice) = resolve_choice(cfg, harness) else {
            continue;
        };
        if !which_any(choice.runner.binary_names()) {
            println!("SKIP {} (not on PATH)", choice.runner.as_str());
            continue;
        }
        if !seen.insert(choice.runner.as_str()) {
            continue;
        }
        print!(
            "PROBE {} model={} … ",
            choice.runner.as_str(),
            if choice.model.is_empty() {
                "(default)"
            } else {
                choice.model.as_str()
            }
        );
        let _ = std::io::Write::flush(&mut std::io::stdout());
        match complete(cfg, "Reply with exactly the single word: pong", harness) {
            Ok(text) => {
                let preview: String = text.chars().take(80).collect();
                if text.to_ascii_lowercase().contains("pong") {
                    println!("OK ({preview})");
                } else {
                    println!("WEAK response ({preview})");
                    failed += 1;
                }
            }
            Err(e) => {
                println!("FAIL ({e:#})");
                failed += 1;
            }
        }
    }

    if failed > 0 {
        bail!("{failed} model probe(s) failed");
    }
    Ok(())
}

/// Known shipped fast-tier identifiers (documentation / doctor).
pub fn shipped_fast_defaults() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        (
            "claude",
            "haiku",
            "Claude Code alias for the current fast Haiku tier",
        ),
        (
            "codex",
            "gpt-5.6-terra",
            "Codex mini-like / lower-cost tier for the current GPT-5.6 line",
        ),
        (
            "grok",
            "grok-3-mini",
            "Grok Build fast/mini tier (override with grok_fast_model)",
        ),
        (
            "pi",
            "(provider default)",
            "Pi uses configured provider/model; set pi_fast_model to pin",
        ),
        (
            "opencode",
            "(provider default)",
            "OpenCode uses configured provider; set opencode_fast_model to pin",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_model_auto_claude() {
        let mut cfg = Config::default();
        cfg.model = "auto".into();
        cfg.claude_fast_model = "haiku".into();
        assert_eq!(resolve_model(&cfg, Runner::Claude), "haiku");
    }

    #[test]
    fn resolve_model_auto_codex() {
        let mut cfg = Config::default();
        cfg.model = "auto".into();
        cfg.codex_fast_model = "gpt-5.6-terra".into();
        assert_eq!(resolve_model(&cfg, Runner::Codex), "gpt-5.6-terra");
    }

    #[test]
    fn resolve_model_auto_grok() {
        let mut cfg = Config::default();
        cfg.model = "auto".into();
        cfg.grok_fast_model = "grok-3-mini".into();
        assert_eq!(resolve_model(&cfg, Runner::Grok), "grok-3-mini");
    }

    #[test]
    fn resolve_model_explicit_wins() {
        let mut cfg = Config::default();
        cfg.model = "claude-haiku-4-5".into();
        assert_eq!(resolve_model(&cfg, Runner::Claude), "claude-haiku-4-5");
        assert_eq!(resolve_model(&cfg, Runner::Codex), "claude-haiku-4-5");
    }

    #[test]
    fn parse_runner() {
        assert_eq!(Runner::parse("claude"), Some(Runner::Claude));
        assert_eq!(Runner::parse("claude-code"), Some(Runner::Claude));
        assert_eq!(Runner::parse("codex"), Some(Runner::Codex));
        assert_eq!(Runner::parse("openai"), Some(Runner::Codex));
        assert_eq!(Runner::parse("grok"), Some(Runner::Grok));
        assert_eq!(Runner::parse("pi"), Some(Runner::Pi));
        assert_eq!(Runner::parse("opencode"), Some(Runner::OpenCode));
        assert_eq!(Runner::parse("open-code"), Some(Runner::OpenCode));
        assert_eq!(Runner::parse("auto"), None);
        assert_eq!(Runner::Grok.as_str(), "grok");
    }

    #[test]
    fn resolve_runner_explicit_pin() {
        let mut cfg = Config::default();
        cfg.model_runner = "claude".into();
        assert_eq!(resolve_runner(&cfg, "codex").unwrap(), Runner::Claude);
        cfg.model_runner = "grok".into();
        assert_eq!(resolve_runner(&cfg, "claude").unwrap(), Runner::Grok);
    }

    #[test]
    fn resolve_runner_auto_uses_session_when_binary_present() {
        let mut cfg = Config::default();
        cfg.model_runner = "auto".into();
        let r = resolve_runner(&cfg, "claude");
        assert!(
            r.is_ok()
                || resolve_runner(&cfg, "codex").is_ok()
                || resolve_runner(&cfg, "grok").is_ok()
                || resolve_runner(&cfg, "").is_ok()
        );
    }

    #[test]
    fn resolve_choice_reason_nonempty() {
        let mut cfg = Config::default();
        cfg.model_runner = "claude".into();
        cfg.model = "haiku".into();
        let c = resolve_choice(&cfg, "claude").unwrap();
        assert_eq!(c.runner, Runner::Claude);
        assert_eq!(c.model, "haiku");
        assert!(!c.reason.is_empty());
    }
}

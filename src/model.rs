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

/// Provider-reported token usage for one analyzer call, when the CLI exposes
/// it. `input_tokens` is logical input including cached reads, matching the
/// transcript-counter semantics used elsewhere.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

/// One completed analyzer call: the completion text plus the resolved
/// runner/model and provider-reported usage where the CLI exposes it
/// (Claude `--output-format json`, Codex `--json`; other runners and
/// custom `model_command`s report `None`).
#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub runner: &'static str,
    pub model: String,
    pub usage: Option<ModelUsage>,
}

/// Pick the CLI harness that should run the lightweight model job.
///
/// Priority:
/// 1. Explicit `model_runner` when not `auto`
/// 2. Session harness when known and binary exists
/// 3. `default_harness` if that binary exists
/// 4. First available among claude, codex, grok, pi, opencode
pub fn resolve_runner(cfg: &Config, session_harness: &str) -> Result<Runner> {
    resolve_runner_with(cfg, session_harness, which_any)
}

fn resolve_runner_with(
    cfg: &Config,
    session_harness: &str,
    is_available: impl Fn(&[&str]) -> bool,
) -> Result<Runner> {
    let explicit = cfg.model_runner.trim().to_ascii_lowercase();
    if explicit != "auto" && !explicit.is_empty() {
        return Runner::parse(&explicit).with_context(|| {
            format!("unknown model_runner {explicit:?}; use auto|claude|codex|grok|pi|opencode")
        });
    }

    if let Some(r) = Runner::parse(session_harness) {
        if is_available(r.binary_names()) {
            return Ok(r);
        }
    }

    if let Some(r) = Runner::parse(&cfg.default_harness) {
        if is_available(r.binary_names()) {
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
        if is_available(r.binary_names()) {
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

/// Run the configured cheap model with a prompt; return the completion text
/// plus provider-reported usage where the runner exposes it.
pub fn complete(cfg: &Config, prompt: &str, session_harness: &str) -> Result<Completion> {
    let choice = resolve_choice(cfg, session_harness)?;
    eprintln!("scopey model: {}", choice.reason);

    let mut prompt_file = NamedTempFile::new().context("temp prompt file")?;
    prompt_file.write_all(prompt.as_bytes())?;
    prompt_file.flush()?;
    let prompt_path = prompt_file.path().to_path_buf();

    let done = |text: String, usage: Option<ModelUsage>| Completion {
        text,
        runner: choice.runner.as_str(),
        model: choice.model.clone(),
        usage,
    };

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
        // Custom commands may invoke an OAuth-authenticated Claude CLI. Keep
        // recursion guards, but do not force Claude's API-key-only SIMPLE mode.
        crate::guard::apply_hook_disable_env(&mut cmd);
        let output = run_with_timeout(cmd, cfg.model_timeout_secs, "model_command")?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let detail = if !stderr.is_empty() {
                format!("stderr={stderr}")
            } else if !stdout.is_empty() {
                format!("stdout={stdout}")
            } else {
                "(empty stdout/stderr)".to_string()
            };
            bail!("model_command failed: status={} {detail}", output.status);
        }
        return Ok(done(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
            None,
        ));
    }

    match choice.runner {
        Runner::Claude => {
            complete_claude(cfg, prompt, &choice.model).map(|(text, usage)| done(text, usage))
        }
        Runner::Codex => {
            complete_codex(cfg, prompt, &choice.model).map(|(text, usage)| done(text, usage))
        }
        Runner::Grok => complete_grok(cfg, prompt, &choice.model).map(|text| done(text, None)),
        Runner::Pi => complete_pi(cfg, prompt, &choice.model).map(|text| done(text, None)),
        Runner::OpenCode => {
            complete_opencode(cfg, prompt, &choice.model).map(|text| done(text, None))
        }
    }
}

fn complete_claude(
    cfg: &Config,
    prompt: &str,
    model: &str,
) -> Result<(String, Option<ModelUsage>)> {
    use crate::guard;

    // Single OAuth-compatible invocation. A `--bare` (API-key-only) fallback
    // used to run second; it can never succeed without an API key, and its
    // "Not logged in" error overwrote the first attempt's real diagnostics
    // (which hid a sustained claude-harness judge outage caused by a stale
    // installed binary), so it is deliberately gone.
    //
    // JSON output carries provider-reported usage alongside the result text;
    // if the CLI ever returns non-JSON, the raw stdout is used as the text
    // and usage is simply absent.
    let mut cmd = Command::new("claude");
    cmd.arg("-p")
        .arg(prompt)
        .arg("--model")
        .arg(model)
        .arg("--output-format")
        .arg("json");
    // Do not set CLAUDE_CODE_SIMPLE — it forces API-key-only auth.
    guard::apply_hook_disable_env(&mut cmd);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let output = run_with_timeout(cmd, cfg.model_timeout_secs, "claude")
        .with_context(|| format!("claude -p --model {model} [{}]", claude_env_diagnostics()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();

    // Claude sometimes exits 0 with "Not logged in · Please run /login" on stdout.
    let not_logged_in =
        combined.contains("not logged in") || combined.contains("please run /login");
    let (text, usage, is_error) = parse_claude_print_json(&stdout);
    if output.status.success() && !text.is_empty() && !not_logged_in && !is_error {
        return Ok((text, usage));
    }
    bail!(
        "claude -p --model {model} failed: status={} stdout={:?} stderr={:?} [{}]",
        output.status,
        clip_model_error(&stdout),
        clip_model_error(&stderr),
        claude_env_diagnostics(),
    )
}

/// Parse `claude -p --output-format json` output: the result text, usage,
/// and the CLI's own error flag. Non-JSON input degrades to (raw, None,
/// false) so a CLI format change never breaks completions, only usage.
fn parse_claude_print_json(stdout: &str) -> (String, Option<ModelUsage>, bool) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout) else {
        return (stdout.trim().to_string(), None, false);
    };
    let text = value
        .get("result")
        .and_then(|r| r.as_str())
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let is_error = value
        .get("is_error")
        .and_then(|e| e.as_bool())
        .unwrap_or(false);
    let usage = value.get("usage").map(|u| {
        let field = |name: &str| u.get(name).and_then(|v| v.as_u64()).unwrap_or(0);
        let uncached = field("input_tokens");
        let cached = field("cache_read_input_tokens");
        let cache_write = field("cache_creation_input_tokens");
        let output = field("output_tokens");
        let input = uncached + cached + cache_write;
        ModelUsage {
            input_tokens: input,
            cached_input_tokens: cached,
            output_tokens: output,
            total_tokens: input + output,
        }
    });
    if text.is_empty() && !is_error {
        // JSON without a result string: fall back to raw text semantics.
        return (stdout.trim().to_string(), usage, false);
    }
    (text, usage, is_error)
}

/// Presence-only environment facts that decide Claude CLI auth behavior,
/// attached to every failure so a single record diagnoses the auth context.
/// HOME's value is included (auth state lives under it); everything else is
/// reported set/unset only.
fn claude_env_diagnostics() -> String {
    let flag = |name: &str| {
        format!(
            "{name}={}",
            if std::env::var_os(name).is_some() {
                "set"
            } else {
                "unset"
            }
        )
    };
    format!(
        "HOME={} {} {} {} {}",
        std::env::var("HOME").unwrap_or_else(|_| "(unset)".into()),
        flag("CLAUDECODE"),
        flag("CLAUDE_CODE_SIMPLE"),
        flag("ANTHROPIC_API_KEY"),
        flag("CLAUDE_CONFIG_DIR"),
    )
}

fn clip_model_error(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 400 {
        trimmed.to_string()
    } else {
        let mut out: String = trimmed.chars().take(400).collect();
        out.push('…');
        out
    }
}

fn complete_codex(cfg: &Config, prompt: &str, model: &str) -> Result<(String, Option<ModelUsage>)> {
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
        .arg("--json")
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
            .arg("--json")
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

    let stdout_text = String::from_utf8_lossy(&stdout).to_string();
    let usage = parse_codex_exec_usage(&stdout_text);
    let from_file = std::fs::read_to_string(&out_path).unwrap_or_default();
    let text = if !from_file.trim().is_empty() {
        from_file
    } else {
        // Without --output-last-message content, extract the agent message
        // from the JSONL stream rather than returning raw event JSON.
        parse_codex_exec_message(&stdout_text).unwrap_or(stdout_text)
    };
    Ok((text.trim().to_string(), usage))
}

/// Cumulative usage from a `codex exec --json` event stream: the last
/// `turn.completed` event's `usage` object wins.
fn parse_codex_exec_usage(stdout: &str) -> Option<ModelUsage> {
    let mut latest = None;
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) != Some("turn.completed") {
            continue;
        }
        let Some(u) = value.get("usage") else {
            continue;
        };
        let field = |name: &str| u.get(name).and_then(|v| v.as_u64()).unwrap_or(0);
        let input = field("input_tokens");
        let output = field("output_tokens");
        let total = field("total_tokens");
        latest = Some(ModelUsage {
            input_tokens: input,
            cached_input_tokens: field("cached_input_tokens"),
            output_tokens: output,
            total_tokens: if total > 0 { total } else { input + output },
        });
    }
    latest
}

fn parse_codex_exec_message(stdout: &str) -> Option<String> {
    let mut message = None;
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) != Some("item.completed") {
            continue;
        }
        let Some(item) = value.get("item") else {
            continue;
        };
        if item.get("type").and_then(|t| t.as_str()) == Some("agent_message") {
            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                if !text.trim().is_empty() {
                    message = Some(text.trim().to_string());
                }
            }
        }
    }
    message
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
            Ok(completion) => {
                let text = completion.text;
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

    // Probes run in this shell's environment; live jobs run detached from
    // hooks. Surface the recorded job outcomes so a green probe cannot mask a
    // failing production path (the issue-15 trap).
    let health = crate::model_health::load();
    if health.attempts() > 0 {
        println!(
            "\nbackground jobs (live summarize/judge): ok={} failed={}",
            health.total_successes, health.total_failures
        );
        if health.consecutive_failures > 0 {
            println!(
                "WARNING: the last {} background model call(s) failed even though probes \
                 may pass; scope extraction falls back to echoing the latest prompt.\n\
                 last error: {}",
                health.consecutive_failures,
                health
                    .last_error
                    .as_deref()
                    .unwrap_or("(no error captured)")
            );
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
        let cfg = Config {
            model: "auto".into(),
            claude_fast_model: "haiku".into(),
            ..Config::default()
        };
        assert_eq!(resolve_model(&cfg, Runner::Claude), "haiku");
    }

    #[test]
    fn resolve_model_auto_codex() {
        let cfg = Config {
            model: "auto".into(),
            codex_fast_model: "gpt-5.6-terra".into(),
            ..Config::default()
        };
        assert_eq!(resolve_model(&cfg, Runner::Codex), "gpt-5.6-terra");
    }

    #[test]
    fn resolve_model_auto_grok() {
        let cfg = Config {
            model: "auto".into(),
            grok_fast_model: "grok-3-mini".into(),
            ..Config::default()
        };
        assert_eq!(resolve_model(&cfg, Runner::Grok), "grok-3-mini");
    }

    #[test]
    fn resolve_model_explicit_wins() {
        let cfg = Config {
            model: "claude-haiku-4-5".into(),
            ..Config::default()
        };
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
        let mut cfg = Config {
            model_runner: "claude".into(),
            ..Config::default()
        };
        assert_eq!(resolve_runner(&cfg, "codex").unwrap(), Runner::Claude);
        cfg.model_runner = "grok".into();
        assert_eq!(resolve_runner(&cfg, "claude").unwrap(), Runner::Grok);
    }

    #[test]
    fn resolve_runner_auto_uses_session_when_binary_present() {
        let cfg = Config {
            model_runner: "auto".into(),
            ..Config::default()
        };
        let runner = resolve_runner_with(&cfg, "codex", |names| names.contains(&"codex"));
        assert_eq!(runner.unwrap(), Runner::Codex);
    }

    #[test]
    fn resolve_runner_auto_reports_when_no_binary_is_present() {
        let cfg = Config::default();
        let error = resolve_runner_with(&cfg, "claude", |_| false).unwrap_err();
        assert!(error.to_string().contains("no model runner on PATH"));
    }

    #[test]
    fn resolve_choice_reason_nonempty() {
        let cfg = Config {
            model_runner: "claude".into(),
            model: "haiku".into(),
            ..Config::default()
        };
        let c = resolve_choice(&cfg, "claude").unwrap();
        assert_eq!(c.runner, Runner::Claude);
        assert_eq!(c.model, "haiku");
        assert!(!c.reason.is_empty());
    }

    #[test]
    fn model_command_removes_inherited_claude_simple_and_uses_shell() {
        Config::with_temp_scopey_home(|_| {
            let previous = std::env::var_os("CLAUDE_CODE_SIMPLE");
            std::env::set_var("CLAUDE_CODE_SIMPLE", "1");

            let cfg = Config {
                model_runner: "claude".into(),
                model: "haiku".into(),
                model_command: Some(
                    "test -z \"${CLAUDE_CODE_SIMPLE+x}\" && printf \"$(printf pong)\"".into(),
                ),
                ..Config::default()
            };
            let result = complete(&cfg, "ignored", "claude");

            match previous {
                Some(value) => std::env::set_var("CLAUDE_CODE_SIMPLE", value),
                None => std::env::remove_var("CLAUDE_CODE_SIMPLE"),
            }

            assert_eq!(result.unwrap().text, "pong");
        });
    }

    #[test]
    fn claude_print_json_yields_text_and_usage() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":false,"result":" pong ","session_id":"abc","usage":{"input_tokens":4,"cache_creation_input_tokens":6706,"cache_read_input_tokens":8875,"output_tokens":6}}"#;
        let (text, usage, is_error) = parse_claude_print_json(stdout);
        assert_eq!(text, "pong");
        assert!(!is_error);
        let usage = usage.unwrap();
        assert_eq!(usage.input_tokens, 4 + 6706 + 8875);
        assert_eq!(usage.cached_input_tokens, 8875);
        assert_eq!(usage.output_tokens, 6);
        assert_eq!(usage.total_tokens, usage.input_tokens + 6);
        // Error flag propagates.
        let (_, _, is_error) =
            parse_claude_print_json(r#"{"result":"limit reached","is_error":true}"#);
        assert!(is_error);
        // Non-JSON degrades to raw text with no usage.
        let (text, usage, is_error) = parse_claude_print_json("plain text answer");
        assert_eq!(text, "plain text answer");
        assert!(usage.is_none());
        assert!(!is_error);
    }

    #[test]
    fn codex_exec_stream_yields_usage_and_message() {
        let stream = concat!(
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"verdict text"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":12692,"cached_input_tokens":8960,"output_tokens":159}}"#,
        );
        let usage = parse_codex_exec_usage(stream).unwrap();
        assert_eq!(usage.input_tokens, 12692);
        assert_eq!(usage.cached_input_tokens, 8960);
        assert_eq!(usage.output_tokens, 159);
        // total absent in the event -> computed.
        assert_eq!(usage.total_tokens, 12692 + 159);
        assert_eq!(parse_codex_exec_message(stream).unwrap(), "verdict text");
        assert!(parse_codex_exec_usage("not json").is_none());
    }

    #[test]
    fn model_command_failure_reports_status_and_stdout() {
        let cfg = Config {
            model_runner: "claude".into(),
            model: "haiku".into(),
            model_command: Some("printf auth-error; exit 7".into()),
            ..Config::default()
        };

        let error = complete(&cfg, "ignored", "claude").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("status=exit status: 7"), "{message}");
        assert!(message.contains("stdout=auth-error"), "{message}");
    }
}

use crate::config::Config;
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::NamedTempFile;

/// Which product CLI (or direct API) runs summarize/judge jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    Claude,
    Codex,
    Grok,
    Pi,
    OpenCode,
    /// Direct HTTPS call to the OpenAI API (OPENAI_API_KEY). Never auto-picked:
    /// only used when `model_runner = "openai-api"` is set explicitly. Carries
    /// no CLI-harness system prompt, so analyzer input is roughly the Scopey
    /// prompt alone (~85% smaller than a `codex exec` call).
    OpenAiApi,
    /// Direct HTTPS call to the Anthropic API (ANTHROPIC_API_KEY). Same
    /// explicit-only, no-harness-overhead contract as `OpenAiApi`.
    AnthropicApi,
}

impl Runner {
    pub fn as_str(self) -> &'static str {
        match self {
            Runner::Claude => "claude",
            Runner::Codex => "codex",
            Runner::Grok => "grok",
            Runner::Pi => "pi",
            Runner::OpenCode => "opencode",
            Runner::OpenAiApi => "openai-api",
            Runner::AnthropicApi => "anthropic-api",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "anthropic" => Some(Runner::Claude),
            "codex" | "openai" => Some(Runner::Codex),
            "grok" | "grok-build" | "xai" => Some(Runner::Grok),
            "pi" => Some(Runner::Pi),
            "opencode" | "open-code" | "open_code" => Some(Runner::OpenCode),
            "openai-api" | "openai_api" | "api-openai" => Some(Runner::OpenAiApi),
            "anthropic-api" | "anthropic_api" | "api-anthropic" => Some(Runner::AnthropicApi),
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
            // API runners need no binary and must never win auto-resolution.
            Runner::OpenAiApi | Runner::AnthropicApi => &[],
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
        Runner::Codex | Runner::OpenAiApi => nonempty_or(&cfg.codex_fast_model, "gpt-5.6-terra"),
        Runner::Grok => nonempty_or(&cfg.grok_fast_model, "grok-3-mini"),
        Runner::Pi => cfg.pi_fast_model.trim().to_string(),
        Runner::OpenCode => cfg.opencode_fast_model.trim().to_string(),
        Runner::AnthropicApi => {
            anthropic_api_model_id(&nonempty_or(&cfg.claude_fast_model, "haiku"))
        }
    }
}

/// The raw Anthropic API needs full model ids; the Claude CLI accepts aliases.
/// Map the common aliases so `claude_fast_model = "haiku"` keeps working when
/// switching to the direct-API runner.
fn anthropic_api_model_id(alias_or_id: &str) -> String {
    match alias_or_id.trim().to_ascii_lowercase().as_str() {
        "haiku" => "claude-haiku-4-5-20251001".into(),
        "sonnet" => "claude-sonnet-5".into(),
        "opus" => "claude-opus-5".into(),
        _ => alias_or_id.trim().to_string(),
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
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }

    match choice.runner {
        Runner::Claude => complete_claude(cfg, prompt, &choice.model),
        Runner::Codex => complete_codex(cfg, prompt, &choice.model),
        Runner::Grok => complete_grok(cfg, prompt, &choice.model),
        Runner::Pi => complete_pi(cfg, prompt, &choice.model),
        Runner::OpenCode => complete_opencode(cfg, prompt, &choice.model),
        Runner::OpenAiApi => complete_openai_api(cfg, prompt, &choice.model),
        Runner::AnthropicApi => complete_anthropic_api(cfg, prompt, &choice.model),
    }
}

/// Output budget for direct-API analyzer calls. Generous because reasoning
/// models count thinking tokens against the completion budget; the visible
/// judge/summarize outputs themselves are a few hundred tokens.
const API_MAX_OUTPUT_TOKENS: u64 = 4096;

fn api_key(env_name: &str, runner: &str) -> Result<String> {
    std::env::var(env_name)
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .with_context(|| format!("model_runner={runner} requires {env_name} in the environment"))
}

fn api_base(env_name: &str, default: &str) -> String {
    std::env::var(env_name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn clip_api_error(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() <= 400 {
        t.to_string()
    } else {
        let mut out: String = t.chars().take(400).collect();
        out.push('…');
        out
    }
}

fn api_send(
    cfg: &Config,
    url: &str,
    headers: &[(&str, &str)],
    body: serde_json::Value,
    label: &str,
) -> Result<serde_json::Value> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(cfg.model_timeout_secs.max(1)))
        .build();
    let mut request = agent.post(url);
    for (name, value) in headers {
        request = request.set(name, value);
    }
    match request.send_json(body) {
        Ok(response) => response
            .into_json::<serde_json::Value>()
            .with_context(|| format!("{label}: response was not JSON")),
        Err(ureq::Error::Status(code, response)) => {
            let text = response.into_string().unwrap_or_default();
            bail!("{label}: status {code}: {}", clip_api_error(&text));
        }
        Err(e) => bail!("{label}: transport error: {e}"),
    }
}

fn parse_openai_content(value: &serde_json::Value) -> Result<String> {
    let content = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|t| t.as_str())
        .map(str::trim)
        .unwrap_or("");
    if content.is_empty() {
        bail!(
            "openai api returned no message content: {}",
            clip_api_error(&value.to_string())
        );
    }
    Ok(content.to_string())
}

fn parse_anthropic_content(value: &serde_json::Value) -> Result<String> {
    let text = value
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|items| {
            items.iter().find_map(|item| {
                (item.get("type").and_then(|t| t.as_str()) == Some("text"))
                    .then(|| item.get("text").and_then(|t| t.as_str()))
                    .flatten()
            })
        })
        .map(str::trim)
        .unwrap_or("");
    if text.is_empty() {
        bail!(
            "anthropic api returned no text content: {}",
            clip_api_error(&value.to_string())
        );
    }
    Ok(text.to_string())
}

fn complete_openai_api(cfg: &Config, prompt: &str, model: &str) -> Result<String> {
    let key = api_key("OPENAI_API_KEY", "openai-api")?;
    let base = api_base("OPENAI_BASE_URL", "https://api.openai.com");
    let url = format!("{base}/v1/chat/completions");
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "max_completion_tokens": API_MAX_OUTPUT_TOKENS,
    });
    let value = api_send(
        cfg,
        &url,
        &[("Authorization", &format!("Bearer {key}"))],
        body,
        "openai api",
    )?;
    parse_openai_content(&value)
}

fn complete_anthropic_api(cfg: &Config, prompt: &str, model: &str) -> Result<String> {
    let key = api_key("ANTHROPIC_API_KEY", "anthropic-api")?;
    let base = api_base("ANTHROPIC_BASE_URL", "https://api.anthropic.com");
    let url = format!("{base}/v1/messages");
    let body = serde_json::json!({
        "model": model,
        "max_tokens": API_MAX_OUTPUT_TOKENS,
        "messages": [{"role": "user", "content": prompt}],
    });
    let value = api_send(
        cfg,
        &url,
        &[
            ("x-api-key", key.as_str()),
            ("anthropic-version", "2023-06-01"),
        ],
        body,
        "anthropic api",
    )?;
    parse_anthropic_content(&value)
}

fn complete_claude(cfg: &Config, prompt: &str, model: &str) -> Result<String> {
    use crate::guard;

    // Prefer normal `claude -p` with OAuth-compatible env (no CLAUDE_CODE_SIMPLE).
    // `--bare` is tried second for API-key sandboxes that want minimal surface.
    let attempts: [(&str, bool); 2] = [("oauth", false), ("bare", true)];
    let mut last_err = String::new();

    for (label, bare) in attempts {
        let mut cmd = Command::new("claude");
        cmd.arg("-p").arg(prompt).arg("--model").arg(model);
        if bare {
            cmd.arg("--bare").arg("--output-format").arg("text");
            // bare mode expects API key; SIMPLE is fine here
            guard::apply_internal_env(&mut cmd);
        } else {
            cmd.arg("--output-format").arg("text");
            // Do not set CLAUDE_CODE_SIMPLE — it forces API-key-only auth.
            guard::apply_hook_disable_env(&mut cmd);
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = match run_with_timeout(cmd, cfg.model_timeout_secs, "claude") {
            Ok(o) => o,
            Err(e) => {
                last_err = format!("{label}: {e:#}");
                continue;
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();

        // Claude sometimes exits 0 with "Not logged in · Please run /login" on stdout.
        let not_logged_in =
            combined.contains("not logged in") || combined.contains("please run /login");
        if output.status.success() && !stdout.is_empty() && !not_logged_in {
            return Ok(stdout);
        }
        last_err = if !stderr.is_empty() {
            format!("{label}: status={} stderr={stderr}", output.status)
        } else if !stdout.is_empty() {
            format!("{label}: status={} stdout={stdout}", output.status)
        } else {
            format!("{label}: status={} (empty output)", output.status)
        };
    }

    bail!("claude -p --model {model} failed: {last_err}")
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

            assert_eq!(result.unwrap(), "pong");
        });
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

    #[test]
    fn api_runners_parse_but_are_never_auto_selected() {
        assert_eq!(Runner::parse("openai-api"), Some(Runner::OpenAiApi));
        assert_eq!(Runner::parse("anthropic_api"), Some(Runner::AnthropicApi));
        // Auto-resolution must ignore API runners even when nothing else exists.
        let cfg = Config {
            model_runner: "auto".into(),
            ..Config::default()
        };
        let err = resolve_runner_with(&cfg, "codex", |_| false).unwrap_err();
        assert!(err.to_string().contains("no model runner"), "{err}");
        // Explicit selection works without any binary on PATH.
        let cfg = Config {
            model_runner: "openai-api".into(),
            ..Config::default()
        };
        assert_eq!(
            resolve_runner_with(&cfg, "codex", |_| false).unwrap(),
            Runner::OpenAiApi
        );
    }

    #[test]
    fn anthropic_api_model_ids_map_cli_aliases() {
        assert_eq!(anthropic_api_model_id("haiku"), "claude-haiku-4-5-20251001");
        assert_eq!(anthropic_api_model_id("sonnet"), "claude-sonnet-5");
        assert_eq!(
            anthropic_api_model_id("claude-haiku-4-5-20251001"),
            "claude-haiku-4-5-20251001"
        );
    }

    #[test]
    fn openai_response_parsing_requires_content() {
        let ok = serde_json::json!({
            "choices": [{"message": {"content": " {\"verdict\":\"on_track\"} "}}]
        });
        assert_eq!(
            parse_openai_content(&ok).unwrap(),
            "{\"verdict\":\"on_track\"}"
        );
        let empty = serde_json::json!({"choices": [{"message": {"content": ""}}]});
        assert!(parse_openai_content(&empty).is_err());
        assert!(parse_openai_content(&serde_json::json!({})).is_err());
    }

    #[test]
    fn anthropic_response_parsing_takes_first_text_block() {
        let ok = serde_json::json!({
            "content": [
                {"type": "thinking", "thinking": "…"},
                {"type": "text", "text": "verdict text"}
            ]
        });
        assert_eq!(parse_anthropic_content(&ok).unwrap(), "verdict text");
        assert!(parse_anthropic_content(&serde_json::json!({"content": []})).is_err());
    }

    #[test]
    fn api_runner_without_key_reports_missing_env() {
        // Ensure the env var truly is absent for this test.
        let previous = std::env::var("OPENAI_API_KEY").ok();
        std::env::remove_var("OPENAI_API_KEY");
        let cfg = Config {
            model_runner: "openai-api".into(),
            model: "gpt-5.6-luna".into(),
            ..Config::default()
        };
        let result = complete(&cfg, "ping", "codex");
        if let Some(value) = previous {
            std::env::set_var("OPENAI_API_KEY", value);
        }
        let message = result.unwrap_err().to_string();
        assert!(message.contains("OPENAI_API_KEY"), "{message}");
    }
}

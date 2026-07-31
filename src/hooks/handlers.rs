use crate::config::Config;
use crate::eventlog;
use crate::guard::{self, SessionJobGuard};
use crate::session::{hash_prompt, SessionMessage, SessionStore, ToolEvent};
use crate::tool_journal::{counts_toward_n, preview_tool_input};
use crate::trajectory::{
    build_correction_injection, build_reminder_injection, drain_pending_jobs, spawn_background_judge,
    spawn_background_summarize, transcript_len,
};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// Absolute first line of defense: headless/internal children never re-enter hooks.
fn hooks_disabled() -> bool {
    if guard::is_internal() {
        // Do not log per-call when internal — would re-amplify during storms.
        eprintln!("scopey hook: no-op (SCOPEY_INTERNAL / hooks disabled)");
        return true;
    }
    false
}

#[derive(Debug, Deserialize)]
pub struct HookEvent {
    /// Claude/Codex snake_case; Grok camelCase (`sessionId`); env fallbacks applied later.
    #[serde(default, alias = "sessionId")]
    pub session_id: Option<String>,
    #[serde(default, alias = "workspaceRoot", alias = "workingDirectory")]
    pub cwd: Option<String>,
    #[serde(default, alias = "transcriptPath", alias = "sessionFile", alias = "session_path")]
    pub transcript_path: Option<String>,
    #[serde(default, alias = "hookEventName", alias = "event")]
    pub hook_event_name: Option<String>,
    #[serde(default, alias = "userPrompt", alias = "text", alias = "message")]
    pub prompt: Option<String>,
    #[allow(dead_code)]
    #[serde(default, alias = "toolName")]
    pub tool_name: Option<String>,
    #[allow(dead_code)]
    #[serde(default, alias = "toolInput", alias = "args")]
    pub tool_input: Option<serde_json::Value>,
    #[serde(default, alias = "toolCalls")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, alias = "reason")]
    pub source: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Explicit harness tag from adapters (pi/opencode/grok).
    #[serde(default)]
    pub harness: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ToolCall {
    #[allow(dead_code)]
    #[serde(default, alias = "toolName", alias = "name")]
    pub tool_name: Option<String>,
}

fn read_event() -> Result<HookEvent> {
    let mut buf = String::new();
    io::stdin()
        .read_to_string(&mut buf)
        .context("read hook stdin")?;
    if buf.trim().is_empty() {
        anyhow::bail!("empty stdin; hook handlers expect harness JSON on stdin");
    }
    // Accept raw JSON object, or wrapper { "event": {...} } from some adapters.
    let v: serde_json::Value =
        serde_json::from_str(&buf).with_context(|| format!("parse hook JSON: {}", clip(&buf, 200)))?;
    let obj = if let Some(inner) = v.get("event").or_else(|| v.get("payload")) {
        inner.clone()
    } else {
        v
    };
    let mut ev: HookEvent = serde_json::from_value(obj)
        .with_context(|| format!("decode hook event: {}", clip(&buf, 200)))?;
    normalize_event(&mut ev);
    Ok(ev)
}

/// Normalize cross-harness quirks into Claude-like fields.
fn normalize_event(ev: &mut HookEvent) {
    if let Some(ref name) = ev.hook_event_name {
        let n = name.to_ascii_lowercase().replace('-', "_");
        ev.hook_event_name = Some(match n.as_str() {
            "pre_tool_use" | "tool_execute_before" | "tool_call" => "PreToolUse".into(),
            "post_tool_use" | "tool_execute_after" | "tool_result" | "tool_execution_end" => {
                "PostToolUse".into()
            }
            "post_tool_batch" | "turn_end" => "PostToolBatch".into(),
            "user_prompt_submit" | "before_agent_start" | "before_submit_prompt" => {
                "UserPromptSubmit".into()
            }
            "session_start" | "session_created" => "SessionStart".into(),
            "stop" | "agent_end" | "session_idle" | "end_turn" => "Stop".into(),
            other => other.to_string(),
        });
    }
}

fn session_id(ev: &HookEvent) -> Result<String> {
    if let Some(s) = ev.session_id.clone().filter(|s| !s.is_empty()) {
        return Ok(s);
    }
    // Grok / env fallbacks
    for key in [
        "SCOPEY_SESSION_ID",
        "GROK_SESSION_ID",
        "CLAUDE_SESSION_ID",
        "CODEX_SESSION_ID",
        "OPENCODE_SESSION_ID",
    ] {
        if let Ok(s) = std::env::var(key) {
            if !s.is_empty() {
                return Ok(s);
            }
        }
    }
    anyhow::bail!("hook JSON missing session_id (and no session env var)")
}

fn cwd(ev: &HookEvent) -> PathBuf {
    if let Some(ref c) = ev.cwd {
        if !c.is_empty() {
            return PathBuf::from(c);
        }
    }
    if let Ok(c) = std::env::var("GROK_WORKSPACE_ROOT") {
        if !c.is_empty() {
            return PathBuf::from(c);
        }
    }
    if let Ok(c) = std::env::var("CLAUDE_PROJECT_DIR") {
        if !c.is_empty() {
            return PathBuf::from(c);
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub(crate) fn harness_from_event(ev: &HookEvent) -> String {
    if let Some(ref h) = ev.harness {
        let hl = h.to_ascii_lowercase();
        if matches!(
            hl.as_str(),
            "claude" | "codex" | "grok" | "pi" | "opencode" | "open-code"
        ) {
            return if hl == "open-code" {
                "opencode".into()
            } else {
                hl
            };
        }
    }
    // Prefer explicit signals from the harness payload / paths.
    if let Some(ref tp) = ev.transcript_path {
        let lower = tp.to_ascii_lowercase();
        if lower.contains("codex") || lower.contains("/.codex/") {
            return "codex".into();
        }
        if lower.contains("claude") || lower.contains("/.claude/") {
            return "claude".into();
        }
        if lower.contains("/.grok/") || lower.contains("grok") {
            return "grok".into();
        }
        if lower.contains("/.pi/") || lower.contains("/pi/") {
            return "pi".into();
        }
        if lower.contains("opencode") {
            return "opencode".into();
        }
    }
    // Env markers from adapters / harness runners
    if std::env::var_os("GROK_SESSION_ID").is_some() || std::env::var_os("GROK_HOOK_EVENT").is_some()
    {
        return "grok".into();
    }
    if std::env::var("SCOPEY_HARNESS")
        .map(|v| v.to_ascii_lowercase())
        .ok()
        .as_deref()
        == Some("pi")
    {
        return "pi".into();
    }
    if std::env::var("SCOPEY_HARNESS")
        .map(|v| v.to_ascii_lowercase())
        .ok()
        .as_deref()
        == Some("opencode")
    {
        return "opencode".into();
    }
    // Model slug heuristics
    if let Some(ref m) = ev.model {
        let ml = m.to_ascii_lowercase();
        if ml.contains("gpt") || ml.contains("codex") || ml.contains("o3") || ml.contains("o4") {
            return "codex".into();
        }
        if ml.contains("claude")
            || ml.contains("haiku")
            || ml.contains("sonnet")
            || ml.contains("opus")
            || ml.contains("fable")
        {
            return "claude".into();
        }
        if ml.contains("grok") {
            return "grok".into();
        }
    }
    "unknown".into()
}

/// Emit harness injection JSON on stdout.
///
/// Codex + Claude both accept `hookSpecificOutput.additionalContext`.
/// - Context is hard-capped (harness limit ~10k).
/// - `hookEventName` is forced to a canonical PascalCase event name.
/// - Only a single JSON object is written (no trailing logs).
fn emit_additional_context(session_id: &str, hook_event_name: &str, context: &str) {
    // Harnesses cap hook strings ~10k; leave headroom for JSON framing.
    let ctx = clip(context, 8_000);
    let event = canonical_hook_event_name(hook_event_name);
    let out = json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": ctx,
        }
    });
    let s = match serde_json::to_string(&out) {
        Ok(s) => s,
        Err(e) => {
            eventlog::error(
                session_id,
                "hook.inject.stdout",
                format!("failed to serialize inject JSON: {e}"),
                json!({}),
            );
            return; // empty stdout is always valid
        }
    };
    // Validate we only emit pure JSON (no control chars outside strings — serde handles this).
    if !s.starts_with('{') {
        eventlog::error(
            session_id,
            "hook.inject.stdout",
            "refusing non-object inject payload",
            json!({}),
        );
        return;
    }
    eventlog::info(
        session_id,
        "hook.inject.stdout",
        "emitted additionalContext to harness",
        json!({
            "hook_event": event,
            "context_chars": ctx.len(),
            "stdout_chars": s.len(),
            "marker": if ctx.contains("COURSE CORRECTION") {
                "correction"
            } else if ctx.contains("SCOPE REMINDER") {
                "reminder"
            } else {
                "other"
            },
            "stdout_preview": clip(&s, 240),
        }),
    );
    // Exactly one line of JSON — no trailing println noise.
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{s}");
    let _ = stdout.flush();
}

fn canonical_hook_event_name(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase().replace('-', "_");
    match n.as_str() {
        "posttooluse" | "post_tool_use" => "PostToolUse",
        "posttoolbatch" | "post_tool_batch" => "PostToolBatch",
        "userpromptsubmit" | "user_prompt_submit" => "UserPromptSubmit",
        "sessionstart" | "session_start" => "SessionStart",
        "stop" => "Stop",
        _ => "PostToolUse",
    }
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub fn user_prompt(cfg: &Config) -> Result<()> {
    if hooks_disabled() {
        return Ok(());
    }
    let ev = read_event()?;
    let sid = session_id(&ev)?;
    let cwd = cwd(&ev);
    let prompt = ev.prompt.clone().unwrap_or_default();
    if prompt.trim().is_empty() {
        eventlog::debug(&sid, "hook.user_prompt", "empty prompt; no-op", json!({}));
        return Ok(());
    }
    // Never store recursive headless prompts as user scope.
    if guard::looks_like_scopey_internal_prompt(&prompt) {
        eventlog::warn(
            &sid,
            "hook.user_prompt",
            "ignored internal scopey/model prompt",
            json!({ "chars": prompt.len() }),
        );
        return Ok(());
    }

    let harness = harness_from_event(&ev);
    let mut store = match SessionStore::open_or_create_hook(cfg, &cwd, &sid, &harness) {
        Ok(s) => s,
        Err(e) => {
            eventlog::warn(
                &sid,
                "hook.user_prompt",
                format!("skip (session lock): {e:#}"),
                json!({}),
            );
            return Ok(()); // never hang / fail the harness
        }
    };
    if let Some(ref tp) = ev.transcript_path {
        store.set_transcript(Some(Path::new(tp)));
    }
    store.append(SessionMessage::user_prompt(&prompt, hash_prompt(&prompt)));
    eventlog::info(
        &sid,
        "hook.user_prompt",
        "cached user prompt",
        json!({
            "harness": harness,
            "chars": prompt.len(),
            "cwd": cwd.display().to_string(),
        }),
    );

    // Single-flight + throttle: defer summarize when busy (drain later).
    if !SessionJobGuard::can_spawn(cfg, &sid)? {
        store.mark_summarize_pending();
        store.persist()?;
        eventlog::info(
            &sid,
            "hook.user_prompt",
            "defer summarize (session busy/throttled)",
            json!({ "summarize_pending": true }),
        );
        return Ok(());
    }
    store.clear_summarize_pending();
    store.persist()?;
    drop(store); // release flock before spawning child that will re-open
    match spawn_background_summarize(cfg, &sid, &cwd) {
        Ok(true) => {}
        Ok(false) => {
            if let Ok(mut s2) = SessionStore::open_or_create_hook(cfg, &cwd, &sid, &harness) {
                s2.mark_summarize_pending();
                let _ = s2.persist();
            }
            eventlog::info(
                &sid,
                "hook.user_prompt",
                "defer summarize (spawn skipped busy/cap)",
                json!({ "summarize_pending": true }),
            );
        }
        Err(e) => {
            if let Ok(mut s2) = SessionStore::open_or_create_hook(cfg, &cwd, &sid, &harness) {
                s2.mark_summarize_pending();
                let _ = s2.persist();
            }
            eventlog::error(
                &sid,
                "hook.user_prompt",
                format!("failed to spawn summarize: {e:#}"),
                json!({}),
            );
        }
    }

    // Silent stdout — do not inject on every prompt by default.
    let _ = drain_pending_jobs(cfg, &sid, &cwd);
    Ok(())
}

pub fn session_start(cfg: &Config) -> Result<()> {
    if hooks_disabled() {
        return Ok(());
    }
    let ev = read_event()?;
    let sid = session_id(&ev)?;
    let cwd = cwd(&ev);
    let harness = harness_from_event(&ev);
    let mut store = match SessionStore::open_or_create_hook(cfg, &cwd, &sid, &harness) {
        Ok(s) => s,
        Err(e) => {
            eventlog::warn(
                &sid,
                "hook.session_start",
                format!("skip (session lock): {e:#}"),
                json!({}),
            );
            return Ok(());
        }
    };
    if let Some(ref tp) = ev.transcript_path {
        store.set_transcript(Some(Path::new(tp)));
    }
    let note = format!(
        "session_start source={}",
        ev.source.clone().unwrap_or_else(|| "unknown".into())
    );
    store.append(SessionMessage {
        type_: crate::session::MessageType::Note,
        ts: chrono::Utc::now(),
        content: Some(note),
        tool_count: None,
        transcript_offset: None,
        from_count: None,
        to_count: None,
        verdict: None,
        status: None,
        summary: None,
        details: None,
        kind: Some("session_start".into()),
        prompt_hash: None,
        id: Some(uuid::Uuid::new_v4().to_string()),
    });
    store.persist()?;
    eventlog::info(
        &sid,
        "hook.session_start",
        "session ensured",
        json!({
            "harness": harness,
            "source": ev.source.clone().unwrap_or_default(),
        }),
    );
    Ok(())
}

pub fn post_tool(cfg: &Config) -> Result<()> {
    if hooks_disabled() {
        return Ok(());
    }
    let ev = read_event()?;
    let sid = session_id(&ev)?;
    let cwd = cwd(&ev);
    let harness = harness_from_event(&ev);
    let hook_name = ev
        .hook_event_name
        .clone()
        .unwrap_or_else(|| "PostToolUse".into());

    let preview_chars = cfg.tool_args_preview_chars.max(32);

    // Expand batch or single tool into journal entries; only meaningful tools
    // advance tool_call_count / N-windows (noise: write_stdin, wait_agent, …).
    let mut journaled: Vec<(String, String, bool)> = Vec::new();
    if let Some(ref batch) = ev.tool_calls {
        for tc in batch {
            let name = tc
                .tool_name
                .clone()
                .unwrap_or_else(|| "unknown".into());
            let noise = !counts_toward_n(&name);
            journaled.push((name, String::new(), noise));
        }
        if journaled.is_empty() {
            journaled.push(("batch".into(), String::new(), false));
        }
    } else {
        let name = ev
            .tool_name
            .clone()
            .unwrap_or_else(|| "unknown".into());
        let args = preview_tool_input(ev.tool_input.as_ref(), preview_chars);
        let noise = !counts_toward_n(&name);
        journaled.push((name, args, noise));
    }

    let mut store = match SessionStore::open_or_create_hook(cfg, &cwd, &sid, &harness) {
        Ok(s) => s,
        Err(e) => {
            // Never block the agent turn for >HOOK_LOCK_WAIT.
            eventlog::warn(
                &sid,
                "hook.post_tool",
                format!("skip (session lock): {e:#}"),
                json!({ "tool": ev.tool_name }),
            );
            return Ok(());
        }
    };
    if let Some(ref tp) = ev.transcript_path {
        store.set_transcript(Some(Path::new(tp)));
    }

    let mut meaningful_delta = 0u64;
    for (name, args, noise) in journaled {
        if noise {
            store.append_tool_event(ToolEvent {
                index: 0,
                name,
                args_preview: args,
                ts: Utc::now(),
                noise: true,
            });
        } else {
            let next = store.data.tool_call_count.saturating_add(1);
            store.data.tool_call_count = next;
            meaningful_delta += 1;
            store.append_tool_event(ToolEvent {
                index: next,
                name,
                args_preview: args,
                ts: Utc::now(),
                noise: false,
            });
        }
    }
    let count = store.data.tool_call_count;
    let delta = meaningful_delta;

    let n = cfg.n_tool_calls.max(1);
    let m = cfg.m_reminder.max(1);
    let max_lag = if cfg.judgement_max_lag_tools == 0 {
        n.saturating_mul(2).max(30)
    } else {
        cfg.judgement_max_lag_tools
    };
    let expired = store.expire_stale_judgements(count, max_lag);
    if expired > 0 {
        eventlog::info(
            &sid,
            "hook.post_tool.stale",
            format!("discarded {expired} stale judgement(s)"),
            json!({ "tool_count": count, "max_lag": max_lag }),
        );
    }

    let mut injected = false;

    // 1) Apply ready off-track/warning judgement (lags previous window ≈ 2N)
    if let Some(j) = store.ready_judgement_for_injection().cloned() {
        let scope = store
            .latest_scope_requirements()
            .unwrap_or_else(|| "(scope requirements not ready yet)".into());
        let summary = j.summary.clone().unwrap_or_default();
        let details = j.details.clone().unwrap_or_default();
        let verdict = j
            .verdict
            .clone()
            .unwrap_or(crate::session::JudgementVerdict::Unknown);
        let text = build_correction_injection(&scope, &summary, &details, &verdict);
        if let Some(id) = j.id.as_deref() {
            store.mark_judgement_injected(id);
        }
        store.append(SessionMessage::injection("correction", &text, count));
        store.data.last_injection_at_count = count;
        emit_additional_context(&sid, &hook_name, &text);
        injected = true;
        eventlog::info(
            &sid,
            "hook.post_tool.inject",
            "injected course correction",
            json!({
                "tool_count": count,
                "verdict": format!("{:?}", verdict),
            }),
        );
    }

    // 2) Periodic scope reminder
    if !injected && count > 0 && count % m == 0 && store.data.last_reminder_at_count != count {
        if let Some(scope) = store.latest_scope_requirements() {
            let text = build_reminder_injection(&scope);
            store.append(SessionMessage::injection("reminder", &text, count));
            store.data.last_reminder_at_count = count;
            store.data.last_injection_at_count = count;
            emit_additional_context(&sid, &hook_name, &text);
            injected = true;
            eventlog::info(
                &sid,
                "hook.post_tool.inject",
                "injected scope reminder",
                json!({ "tool_count": count, "scope_chars": scope.len() }),
            );
        }
    }

    // 3) Every N *meaningful* tools: trajectory mark + background judge
    let schedule_judge = if delta > 0 && count > 0 && count % n == 0 {
        let from = count.saturating_sub(n);
        let to = count;
        let off = transcript_len(store.data.transcript_path.as_ref().map(Path::new));
        store.append(SessionMessage::trajectory_mark(count, off));
        let tp = store.data.transcript_path.as_ref().map(PathBuf::from);
        let journal_len = store.data.tool_events.len();
        store.persist()?;
        Some((from, to, tp, journal_len))
    } else {
        store.persist()?;
        eventlog::debug(
            &sid,
            "hook.post_tool",
            "tool count advanced",
            json!({
                "tool_count": count,
                "delta": delta,
                "hook_event": hook_name,
                "harness": harness,
            }),
        );
        None
    };
    // Release flock before spawning bg jobs / drain (they re-open the store).
    drop(store);

    if let Some((from, to, tp, journal_len)) = schedule_judge {
        match spawn_background_judge(cfg, &sid, &cwd, from, to, tp.as_deref()) {
            Ok(true) => {
                eventlog::info(
                    &sid,
                    "hook.post_tool.judge",
                    "scheduled background judge",
                    json!({
                        "from": from,
                        "to": to,
                        "tool_count": count,
                        "delta": delta,
                        "journal_len": journal_len,
                    }),
                );
            }
            Ok(false) => {
                if let Ok(mut s2) = SessionStore::open_or_create_hook(cfg, &cwd, &sid, &harness) {
                    s2.set_pending_judge(from, to);
                    let _ = s2.persist();
                }
                eventlog::info(
                    &sid,
                    "hook.post_tool.judge",
                    "defer judge (session busy/throttled/cap)",
                    json!({ "from": from, "to": to, "tool_count": count }),
                );
            }
            Err(e) => {
                if let Ok(mut s2) = SessionStore::open_or_create_hook(cfg, &cwd, &sid, &harness) {
                    s2.set_pending_judge(from, to);
                    let _ = s2.persist();
                }
                eventlog::error(
                    &sid,
                    "hook.post_tool.judge",
                    format!("failed to spawn judge: {e:#}"),
                    json!({ "from": from, "to": to }),
                );
            }
        }
    }

    let _ = injected;
    // Drain any deferred summarize/judge if free.
    let _ = drain_pending_jobs(cfg, &sid, &cwd);
    Ok(())
}

pub fn stop(cfg: &Config) -> Result<()> {
    if hooks_disabled() {
        return Ok(());
    }
    let ev = read_event()?;
    let sid = session_id(&ev)?;
    let cwd = cwd(&ev);
    let harness = harness_from_event(&ev);
    let hook_name = ev
        .hook_event_name
        .clone()
        .unwrap_or_else(|| "Stop".into());

    let mut store = match SessionStore::open_or_create_hook(cfg, &cwd, &sid, &harness) {
        Ok(s) => s,
        Err(e) => {
            eventlog::warn(
                &sid,
                "hook.stop",
                format!("skip (session lock): {e:#}"),
                json!({}),
            );
            return Ok(());
        }
    };
    if let Some(ref tp) = ev.transcript_path {
        store.set_transcript(Some(Path::new(tp)));
    }

    let count = store.data.tool_call_count;
    let max_lag = if cfg.judgement_max_lag_tools == 0 {
        cfg.n_tool_calls.saturating_mul(2).max(30)
    } else {
        cfg.judgement_max_lag_tools
    };
    let _ = store.expire_stale_judgements(count, max_lag);

    if let Some(j) = store.ready_judgement_for_injection().cloned() {
        let scope = store
            .latest_scope_requirements()
            .unwrap_or_else(|| "(scope requirements not ready yet)".into());
        let summary = j.summary.clone().unwrap_or_default();
        let details = j.details.clone().unwrap_or_default();
        let verdict = j
            .verdict
            .clone()
            .unwrap_or(crate::session::JudgementVerdict::Unknown);
        let text = build_correction_injection(&scope, &summary, &details, &verdict);
        if let Some(id) = j.id.as_deref() {
            store.mark_judgement_injected(id);
        }
        store.append(SessionMessage::injection("correction_stop", &text, count));
        store.persist()?;
        emit_additional_context(&sid, &hook_name, &text);
        eventlog::info(
            &sid,
            "hook.stop",
            "injected pending correction",
            json!({ "tool_count": count }),
        );
    } else {
        store.persist()?;
        eventlog::debug(&sid, "hook.stop", "no pending correction", json!({}));
    }
    drop(store);

    let _ = drain_pending_jobs(cfg, &sid, &cwd);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_from_transcript_path() {
        let claude = HookEvent {
            session_id: Some("s".into()),
            cwd: None,
            transcript_path: Some("/Users/x/.claude/projects/foo/s.jsonl".into()),
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_calls: None,
            source: None,
            model: None,
            harness: None,
        };
        assert_eq!(harness_from_event(&claude), "claude");

        let codex = HookEvent {
            session_id: Some("s".into()),
            cwd: None,
            transcript_path: Some("/Users/x/.codex/sessions/rollout.jsonl".into()),
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_calls: None,
            source: None,
            model: None,
            harness: None,
        };
        assert_eq!(harness_from_event(&codex), "codex");
    }

    #[test]
    fn harness_from_model_slug() {
        let ev = HookEvent {
            session_id: None,
            cwd: None,
            transcript_path: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_calls: None,
            source: None,
            model: Some("gpt-5.6-sol".into()),
            harness: None,
        };
        assert_eq!(harness_from_event(&ev), "codex");

        let ev2 = HookEvent {
            model: Some("claude-fable-5".into()),
            harness: None,
            ..ev
        };
        assert_eq!(harness_from_event(&ev2), "claude");

        let ev3 = HookEvent {
            model: Some("grok-3-mini".into()),
            harness: None,
            session_id: None,
            cwd: None,
            transcript_path: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_calls: None,
            source: None,
        };
        assert_eq!(harness_from_event(&ev3), "grok");
    }

    #[test]
    fn harness_explicit_tag() {
        let ev = HookEvent {
            session_id: None,
            cwd: None,
            transcript_path: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_calls: None,
            source: None,
            model: None,
            harness: Some("pi".into()),
        };
        assert_eq!(harness_from_event(&ev), "pi");
        let ev2 = HookEvent {
            harness: Some("opencode".into()),
            ..ev
        };
        assert_eq!(harness_from_event(&ev2), "opencode");
    }

    #[test]
    fn camel_case_session_id_deserializes() {
        let raw = r#"{"sessionId":"abc","hookEventName":"user_prompt_submit","prompt":"hi"}"#;
        let mut ev: HookEvent = serde_json::from_str(raw).unwrap();
        normalize_event(&mut ev);
        assert_eq!(ev.session_id.as_deref(), Some("abc"));
        assert_eq!(ev.hook_event_name.as_deref(), Some("UserPromptSubmit"));
        assert_eq!(ev.prompt.as_deref(), Some("hi"));
    }

    #[test]
    fn harness_unknown_without_signals() {
        let ev = HookEvent {
            session_id: None,
            cwd: None,
            transcript_path: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_calls: None,
            source: None,
            model: None,
            harness: None,
        };
        assert_eq!(harness_from_event(&ev), "unknown");
    }
}

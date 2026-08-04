use crate::config::Config;
use crate::eventlog;
use crate::model;
use crate::notify;
use crate::session::{
    hash_prompt, JudgementStatus, JudgementVerdict, SessionMessage, SessionStore,
};
use crate::tool_journal::{extract_tools_from_transcript, format_tools_for_judge};
use anyhow::{Context, Result};
use serde_json::json;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const SCOPE_CONTEXT_TURNS: usize = 4;

fn build_scope_analysis_prompt(
    prompts: &[String],
    previous_scope: Option<&str>,
    max_chars: usize,
) -> String {
    let latest = prompts.last().map(String::as_str).unwrap_or("");
    let latest_budget = (max_chars / 2).max(1000);
    let previous_budget = (max_chars / 4).max(500);
    let context_budget = max_chars
        .saturating_sub(latest_budget)
        .saturating_sub(previous_budget)
        .max(500);
    let latest = clip(latest, latest_budget);
    let previous = previous_scope
        .map(|scope| clip(scope, previous_budget))
        .unwrap_or_else(|| "(none — this is the first extraction)".into());
    let earlier =
        recent_prompt_context(&prompts[..prompts.len().saturating_sub(1)], context_budget);

    format!(
        r#"You are a scope analyst for a coding agent session.
Produce the CURRENT ACTIVE SCOPE after interpreting the latest user prompt.

Treat the latest prompt as an authoritative mutation of the previous active
scope. Infer one or more of these operations:
- ADD: add requirements while preserving all unaffected active requirements.
- SUBTRACT: explicitly cancel, remove, or declare requirements out of scope.
- MODIFY: alter or narrow named requirements while preserving unaffected ones.
- REPLACE: explicitly supersede the scope, or clearly start an unrelated task.
- QUERY: add an answer, explanation, assessment, logs, or status obligation.
- ADMIN: add a commit, push, PR, or continue operation for the active work.
- MACHINE_EVENT: record generated state/context without inventing a user goal.

Scope lifecycle rules:
- Apply every inferred operation; a prompt may combine operations (for example,
  subtract one requirement and add another).
- The latest user prompt is authoritative for those mutations. Explicit removals,
  modifications, replacements, and contradictions override previous scope.
- For ADD or MODIFY, preserve every unaffected active requirement. Do not treat
  silence about an existing requirement as cancellation.
- For SUBTRACT, remove the named requirement and anything that depends only on it.
- For REPLACE, discard the old task instead of unioning unrelated topics.
- For QUERY, add the answering/reporting obligation. State the requested output
  positively; do not add a no-edit/no-tools boundary merely because it is a query.
  Preserve unaffected existing scope unless the user also subtracts or replaces it.
- For ADMIN, preserve the directly relevant unfinished task plus the requested administrative action; do not resurrect older tasks.
- For MACHINE_EVENT, treat the event as state/context. Preserve only the explicit user request it resolves or updates.
- Earlier prompts and previous scope are context, not automatically active requirements.
- Output only requirements active NOW. Retire requirements only when the latest
  mutation removes/replaces them or available context clearly establishes completion.
- Framing such as "let's figure out how to", "design", "construct", "evaluate",
  or "research" does not mean planning-only. Preserve the operative action verbs.
- Never invent planning-only, research-only, no-implementation, no-tools, no-edit,
  or read-only boundaries. Include one only when a user explicitly imposed it,
  and quote the user's exact supporting words in that bullet.
- Preserve explicit constraints and concrete semantics from the active request.

Output rules:
- First line exactly: <!-- scope-transition: OPERATIONS --> where OPERATIONS is
  the inferred operation names separated by commas (for example ADD,SUBTRACT).
- After that marker, output Markdown bullets only; no preamble or closing.
- Capture active goals, constraints, out-of-scope boundaries, and done-when criteria.
- Max ~25 concise bullets.

PREVIOUS EXTRACTED SCOPE (possibly stale; use only if still active):
---
{previous}
---

RECENT EARLIER USER CONTEXT (reference resolution only):
---
{earlier}
---

LATEST USER PROMPT (authoritative):
---
{latest}
---
"#
    )
}

fn recent_prompt_context(prompts: &[String], max_chars: usize) -> String {
    if prompts.is_empty() {
        return "(none)".into();
    }
    let selected: Vec<&String> = prompts
        .iter()
        .rev()
        .take(SCOPE_CONTEXT_TURNS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let numbered = selected
        .iter()
        .enumerate()
        .map(|(index, prompt)| format!("Turn {}: {}", index + 1, prompt.trim()))
        .collect::<Vec<_>>()
        .join("\n\n");
    clip(&numbered, max_chars)
}

fn latest_prompt_scope(latest_prompt: &str) -> String {
    format!(
        "- Respond only to the latest user request:\n{}",
        clip(latest_prompt, 1500)
    )
}

fn fallback_scope(latest_prompt: &str) -> String {
    format!(
        "- {}\n{}",
        crate::session::FALLBACK_SCOPE_MARKER,
        latest_prompt_scope(latest_prompt)
    )
}

fn sensitive_permission_boundary(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "planning only",
        "planning-only",
        "analysis only",
        "analysis-only",
        "planning/analysis",
        "research only",
        "research-only",
        "request is read-only",
        "request is read only",
        "task is read-only",
        "task is read only",
        "work is read-only",
        "work is read only",
        "read-only request",
        "read only request",
        "read-only task",
        "read only task",
        "no implementation",
        "no-implementation",
        "without implementation",
        "do not implement",
        "don't implement",
        "don’t implement",
        "rather than implement",
        "no edits",
        "no-edit",
        "without editing",
        "do not edit",
        "don't edit",
        "don’t edit",
        "no tools",
        "no-tools",
        "no tool use",
        "do not run tools",
        "do not use tools",
        "don't run tools",
        "don't use tools",
        "don’t run tools",
        "don’t use tools",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
}

fn user_explicitly_restricted_actions(prompts: &[String]) -> bool {
    prompts.iter().any(|prompt| {
        sensitive_permission_boundary(prompt) || {
            let lower = prompt.to_lowercase();
            lower.contains("only explain")
                || lower.contains("only investigate")
                || lower.contains("just explain")
                || lower.contains("just investigate")
        }
    })
}

fn remove_unsubstantiated_permission_boundaries(
    scope: &str,
    prompts: &[String],
    transition: &str,
) -> (String, usize) {
    let relevant_prompts = if transition
        .split(',')
        .any(|operation| operation.trim() == "REPLACE")
    {
        &prompts[prompts.len().saturating_sub(1)..]
    } else {
        prompts
    };
    if user_explicitly_restricted_actions(relevant_prompts) {
        return (scope.to_string(), 0);
    }
    let mut removed = 0;
    let retained = scope
        .lines()
        .filter(|line| {
            let remove = sensitive_permission_boundary(line);
            removed += usize::from(remove);
            !remove
        })
        .collect::<Vec<_>>()
        .join("\n");
    (retained, removed)
}

fn extract_scope_transition(output: &str) -> (String, String) {
    let trimmed = output.trim();
    let Some((first, rest)) = trimmed.split_once('\n') else {
        return ("UNKNOWN".into(), trimmed.into());
    };
    let Some(operations) = first
        .trim()
        .strip_prefix("<!-- scope-transition:")
        .and_then(|value| value.strip_suffix("-->"))
    else {
        return ("UNKNOWN".into(), trimmed.into());
    };
    let operations = operations.trim().to_ascii_uppercase();
    let valid = operations.split(',').all(|operation| {
        matches!(
            operation.trim(),
            "ADD" | "SUBTRACT" | "MODIFY" | "REPLACE" | "QUERY" | "ADMIN" | "MACHINE_EVENT"
        )
    });
    if operations.is_empty() || !valid {
        return ("UNKNOWN".into(), rest.trim().into());
    }
    (operations, rest.trim().into())
}

pub fn summarize_scope(
    cfg: &Config,
    session_id: &str,
    cwd: &Path,
    extra_prompt: Option<&str>,
) -> Result<()> {
    eventlog::info(
        session_id,
        "job.summarize.start",
        "starting summarize",
        json!({ "cwd": cwd.display().to_string() }),
    );
    let (prompts, previous_scope, harness) = {
        let mut store = SessionStore::open_or_create(cfg, cwd, session_id, "")?;
        if let Some(p) = extra_prompt {
            if !p.trim().is_empty() {
                store.begin_scope_epoch();
                store.append(SessionMessage::user_prompt(p, hash_prompt(p)));
                store.persist()?;
            }
        }
        let prompts = store.all_user_prompts();
        let previous_scope = store.latest_scope_requirements();
        let harness = store.data.harness.clone();
        // Drop store before model.complete — exclusive flock must not span network I/O.
        drop(store);
        (prompts, previous_scope, harness)
    };
    if prompts.is_empty() {
        eventlog::warn(
            session_id,
            "job.summarize",
            "no user prompts in session",
            json!({}),
        );
        return Ok(());
    }
    let joined = prompts.join("\n\n---\n\n");
    let latest_prompt = prompts.last().expect("checked non-empty prompts");
    let sys = build_scope_analysis_prompt(
        &prompts,
        previous_scope.as_deref(),
        cfg.summarize_prompt_chars,
    );

    let (transition, raw_out) = match model::complete(cfg, &sys, &harness) {
        Ok(t) => {
            crate::model_health::record_success(cfg, "summarize");
            extract_scope_transition(&t)
        }
        Err(e) => {
            crate::model_health::record_failure(cfg, "summarize", &format!("{e:#}"));
            // Fallback: latest request only, so offline mode cannot resurrect stale scope.
            eventlog::warn(
                session_id,
                "job.summarize.model",
                format!("model failed; using latest-prompt fallback: {e:#}"),
                json!({ "harness": harness }),
            );
            ("FALLBACK_LATEST".into(), fallback_scope(latest_prompt))
        }
    };
    let (mut out, removed_permission_boundaries) =
        remove_unsubstantiated_permission_boundaries(&raw_out, &prompts, &transition);
    let permission_fallback = removed_permission_boundaries > 0 && out.trim().is_empty();
    if permission_fallback {
        out = latest_prompt_scope(latest_prompt);
    }

    let hash = hash_prompt(&joined);
    let previous_scope_hash = previous_scope.as_deref().map(hash_prompt);
    let scope_hash = hash_prompt(&out);
    let mut store = SessionStore::open_or_create(cfg, cwd, session_id, &harness)?;
    store.append(SessionMessage::scope_requirements(out.trim(), Some(hash)));
    store.clear_summarize_pending();
    store.persist()?;
    eventlog::info(
        session_id,
        "job.summarize.transition",
        format!("applied scope transition {transition}"),
        json!({
            "transition": transition,
            "previous_scope_hash": previous_scope_hash,
            "scope_hash": scope_hash,
            "fallback": transition == "FALLBACK_LATEST",
            "removed_permission_boundaries": removed_permission_boundaries,
            "permission_fallback": permission_fallback,
        }),
    );
    eventlog::info(
        session_id,
        "job.summarize.done",
        "wrote scope_requirements",
        json!({ "chars": out.len(), "prompt_count": prompts.len(), "transition": transition }),
    );
    Ok(())
}

pub fn judge_window(
    cfg: &Config,
    session_id: &str,
    cwd: &Path,
    from_count: u64,
    to_count: u64,
    transcript_path: Option<&Path>,
) -> Result<()> {
    eventlog::info(
        session_id,
        "job.judge.start",
        "starting judge window",
        json!({
            "from": from_count,
            "to": to_count,
            "cwd": cwd.display().to_string(),
            "transcript": transcript_path.map(|p| p.display().to_string()),
        }),
    );
    let (
        pending_id,
        judged_prompt_hash,
        scope,
        journal,
        evidence_source,
        last_user,
        harness,
        store_cwd,
    ) = {
        let mut store = SessionStore::open_or_create(cfg, cwd, session_id, "")?;
        if let Some(tp) = transcript_path {
            store.set_transcript(Some(tp));
        }

        let judged_prompt_hash = store.latest_user_prompt_hash().map(str::to_owned);
        let mut pending = SessionMessage::judgement(
            from_count,
            to_count,
            JudgementVerdict::Unknown,
            JudgementStatus::Pending,
            "judging…",
            "",
        );
        pending.prompt_hash = judged_prompt_hash.clone();
        let pending_id = pending.id.clone();
        store.upsert_judgement(pending);
        store.persist()?;

        let scope = store
            .latest_scope_requirements()
            .unwrap_or_else(|| "(no scope requirements recorded yet)".into());

        let tp = transcript_path.map(|p| p.to_path_buf()).or_else(|| {
            store
                .data
                .transcript_path
                .as_ref()
                .map(Path::new)
                .map(|p| p.to_path_buf())
        });

        let mut journal = store.tool_events_in_window(from_count, to_count);
        let mut evidence_source = "journal";
        if journal.is_empty() {
            if let Some(ref p) = tp {
                if p.exists() {
                    let extracted = extract_tools_from_transcript(p, 200);
                    let want = (to_count.saturating_sub(from_count)).max(1) as usize;
                    if extracted.len() >= want {
                        journal = extracted[extracted.len().saturating_sub(want)..].to_vec();
                    } else {
                        journal = extracted;
                    }
                    for (i, e) in journal.iter_mut().enumerate() {
                        e.index = from_count + i as u64 + 1;
                    }
                    evidence_source = "transcript_parse";
                }
            }
        }

        let last_user = store.all_user_prompts().last().cloned().unwrap_or_default();
        let harness = store.data.harness.clone();
        let store_cwd = store.data.cwd.clone();
        // Release exclusive flock before model I/O.
        drop(store);
        (
            pending_id,
            judged_prompt_hash,
            scope,
            journal,
            evidence_source,
            last_user,
            harness,
            store_cwd,
        )
    };

    let excerpt = format_tools_for_judge(&journal);
    let tool_n = journal.len();

    // No actionable tools → never warn/off_track.
    if tool_n == 0 {
        let mut store = SessionStore::open_or_create(cfg, cwd, session_id, &harness)?;
        if let Some(id) = &pending_id {
            store.data.messages.retain(|m| m.id.as_deref() != Some(id));
        }
        let mut ready = SessionMessage::judgement(
            from_count,
            to_count,
            JudgementVerdict::InsufficientEvidence,
            JudgementStatus::Ready,
            "no tool actions in window; skipping inject",
            format!(
                "Window [{from_count},{to_count}) had zero journaled tools (source={evidence_source}). \
                 Scopey refuses to emit warning/off_track without visible tool evidence."
            ),
        );
        ready.prompt_hash = judged_prompt_hash.clone();
        store.upsert_judgement(ready);
        store.data.last_judged_to_count = to_count;
        if store
            .data
            .pending_judge
            .as_ref()
            .is_some_and(|p| p.from_count == from_count && p.to_count == to_count)
        {
            store.data.pending_judge = None;
        }
        store.persist()?;
        eventlog::info(
            session_id,
            "job.judge.done",
            format!("window [{from_count},{to_count}) → insufficient evidence"),
            json!({
                "from": from_count,
                "to": to_count,
                "verdict": "InsufficientEvidence",
                "tool_n": 0,
                "evidence_source": evidence_source,
            }),
        );
        return Ok(());
    }

    let last_user_clip = clip(&last_user, 800);
    let prompt = format!(
        r#"You are a strict scope auditor for a coding agent.

SCOPE REQUIREMENTS:
{scope}

LATEST USER PROMPT (may refine scope):
{last_user_clip}

TOOL-CALL WINDOW: meaningful counts [{from_count}, {to_count})
Structured tool journal for THIS window only ({tool_n} tools, source={evidence_source}):
---
{excerpt}
---

Judge whether the agent is still working within the scope requirements.
Treat LATEST USER PROMPT as authoritative if it conflicts with or supersedes the extracted scope.
Do not enforce a planning-only, research-only, no-implementation, no-tools,
no-edit, or read-only boundary unless the latest prompt explicitly imposes it
or the scope bullet quotes the user's exact supporting words. Phrases such as
"figure out how to", "design", "construct", "evaluate", and "research" are
goals, not proof that implementation or tools are forbidden.
If the latest prompt is a question, status request, or assessment, judge only whether the agent is answering/investigating that request; do not require implementation merely because an older scope did.
Do not penalize the agent for retiring completed or superseded requirements.
Focus especially on file writes/edits and shell commands that change system state.
Read-only investigation (rg, sed, cat, git show/log, list files) is OnTrack when the scope is an implementation task that has not yet been started or is still being scoped.
You MUST cite tool names and paths from the journal in details.
If the journal does not show enough to decide, use verdict "insufficient_evidence" (never invent off-scope actions).

Respond with EXACTLY this JSON object (no markdown fences):
{{
  "verdict": "on_track" | "warning" | "off_track" | "insufficient_evidence",
  "summary": "one sentence",
  "details": "2-6 sentences of evidence and what to do instead if off_track",
  "off_scope_actions": ["short list of problematic actions, or empty"]
}}
"#
    );

    let raw = match model::complete(cfg, &prompt, &harness) {
        Ok(t) => {
            crate::model_health::record_success(cfg, "judge");
            t
        }
        Err(e) => {
            crate::model_health::record_failure(cfg, "judge", &format!("{e:#}"));
            let mut store = SessionStore::open_or_create(cfg, cwd, session_id, &harness)?;
            let mut failed = SessionMessage::judgement(
                from_count,
                to_count,
                JudgementVerdict::Unknown,
                JudgementStatus::Failed,
                format!("judge model error: {e:#}"),
                "",
            );
            failed.prompt_hash = judged_prompt_hash.clone();
            if let Some(id) = &pending_id {
                store.data.messages.retain(|m| m.id.as_deref() != Some(id));
            }
            store.upsert_judgement(failed);
            store.persist()?;
            eventlog::error(
                session_id,
                "job.judge.model",
                format!("judge model error: {e:#}"),
                json!({ "from": from_count, "to": to_count }),
            );
            return Err(e).context("judge model");
        }
    };

    let mut parsed = parse_judgement_json(&raw);
    if matches!(
        parsed.0,
        JudgementVerdict::Warning | JudgementVerdict::OffTrack
    ) && tool_n == 0
    {
        parsed = (
            JudgementVerdict::InsufficientEvidence,
            "no tool evidence".into(),
            parsed.2,
        );
    }
    let (verdict, summary, details) = parsed;

    let mut store = SessionStore::open_or_create(cfg, cwd, session_id, &harness)?;
    if let Some(id) = &pending_id {
        store.data.messages.retain(|m| m.id.as_deref() != Some(id));
    }

    if store.latest_user_prompt_hash() != judged_prompt_hash.as_deref() {
        let mut superseded = SessionMessage::judgement(
            from_count,
            to_count,
            JudgementVerdict::Unknown,
            JudgementStatus::Failed,
            "judgement discarded because a newer user prompt changed scope",
            "The judgement completed after its prompt epoch ended and was not eligible for notification or injection.",
        );
        superseded.prompt_hash = judged_prompt_hash;
        store.data.pending_judgement_id = None;
        store.upsert_judgement(superseded);
        store.persist()?;
        eventlog::info(
            session_id,
            "job.judge.superseded",
            format!("discarded window [{from_count},{to_count}) after scope changed"),
            json!({ "from": from_count, "to": to_count }),
        );
        return Ok(());
    }

    let mut ready = SessionMessage::judgement(
        from_count,
        to_count,
        verdict.clone(),
        JudgementStatus::Ready,
        summary.clone(),
        details.clone(),
    );
    ready.prompt_hash = judged_prompt_hash;
    store.upsert_judgement(ready);
    store.data.last_judged_to_count = to_count;
    if store
        .data
        .pending_judge
        .as_ref()
        .is_some_and(|p| p.to_count <= to_count)
    {
        store.data.pending_judge = None;
    }
    store.persist()?;

    eventlog::info(
        session_id,
        "job.judge.done",
        format!("window [{from_count},{to_count}) → {summary}"),
        json!({
            "from": from_count,
            "to": to_count,
            "verdict": format!("{:?}", verdict),
            "tool_n": tool_n,
            "evidence_source": evidence_source,
        }),
    );

    let should_notify = match verdict {
        JudgementVerdict::OffTrack if cfg.notify_on_off_track => true,
        JudgementVerdict::Warning if cfg.notify_on_warning => true,
        _ => false,
    };
    if should_notify {
        let ctx = notify::NotifyContext {
            verdict: &verdict,
            summary: &summary,
            details: &details,
            session_id,
            cwd: store_cwd.as_str(),
            from_count,
            to_count,
            harness: harness.as_str(),
        };
        if let Err(e) = notify::notify_judgement(cfg, &ctx) {
            eventlog::error(
                session_id,
                "job.judge.notify",
                format!("notify failed: {e:#}"),
                json!({}),
            );
        } else {
            eventlog::info(
                session_id,
                "job.judge.notify",
                "notification sent",
                json!({ "verdict": format!("{:?}", verdict) }),
            );
        }
    }

    Ok(())
}

/// Try to spawn any deferred summarize/judge work for a session (throttle drain).
pub fn drain_pending_jobs(cfg: &Config, session_id: &str, cwd: &Path) -> Result<()> {
    use crate::guard::SessionJobGuard;

    if !SessionJobGuard::can_spawn(cfg, session_id)? {
        return Ok(());
    }

    let mut store = SessionStore::open_or_create(cfg, cwd, session_id, "")?;
    let tp = store.data.transcript_path.as_ref().map(PathBuf::from);

    if store.data.summarize_pending {
        store.persist()?;
        eventlog::info(
            session_id,
            "drain.summarize",
            "spawning deferred summarize",
            json!({}),
        );
        // Keep pending true until summarize succeeds (clears it).
        match spawn_background_summarize(cfg, session_id, cwd) {
            Ok(true) => return Ok(()), // one job at a time
            Ok(false) => {}
            Err(e) => {
                eventlog::error(
                    session_id,
                    "drain.summarize",
                    format!("deferred summarize spawn failed: {e:#}"),
                    json!({}),
                );
            }
        }
    }

    if let Some(pj) = store.data.pending_judge.clone() {
        store.persist()?;
        eventlog::info(
            session_id,
            "drain.judge",
            "spawning deferred judge",
            json!({ "from": pj.from_count, "to": pj.to_count }),
        );
        // Clear before spawn so we don't double-queue; restore if not spawned.
        store.data.pending_judge = None;
        store.persist()?;
        match spawn_background_judge(
            cfg,
            session_id,
            cwd,
            pj.from_count,
            pj.to_count,
            tp.as_deref(),
        ) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                if let Ok(mut s2) = SessionStore::open_or_create(cfg, cwd, session_id, "") {
                    s2.set_pending_judge(pj.from_count, pj.to_count);
                    let _ = s2.persist();
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn parse_judgement_json(raw: &str) -> (JudgementVerdict, String, String) {
    let trimmed = raw.trim();
    // extract first {...} if model wrapped text
    let json_slice = extract_json_object(trimmed).unwrap_or(trimmed);
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_slice) {
        let verdict = match v
            .get("verdict")
            .and_then(|x| x.as_str())
            .unwrap_or("unknown")
            .to_ascii_lowercase()
            .as_str()
        {
            "on_track" | "on-track" | "ok" => JudgementVerdict::OnTrack,
            "warning" | "warn" => JudgementVerdict::Warning,
            "off_track" | "off-track" | "offtrack" => JudgementVerdict::OffTrack,
            "insufficient_evidence" | "insufficient-evidence" | "no_evidence" => {
                JudgementVerdict::InsufficientEvidence
            }
            _ => JudgementVerdict::Unknown,
        };
        let summary = v
            .get("summary")
            .and_then(|x| x.as_str())
            .unwrap_or("no summary")
            .to_string();
        let details = v
            .get("details")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let verdict = crate::session::normalize_judgement_verdict(verdict, &summary, &details);
        return (verdict, summary, details);
    }
    (
        JudgementVerdict::Unknown,
        "unparseable judgement".into(),
        clip(raw, 2000),
    )
}

pub(crate) fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end > start {
        Some(&s[start..=end])
    } else {
        None
    }
}

/// Legacy raw-tail helper (kept for tests / emergency debugging). Prefer the tool journal.
#[cfg(test)]
pub fn read_transcript_excerpt(path: &Path, max_chars: usize) -> Result<String> {
    let data = fs::read(path).with_context(|| format!("read transcript {}", path.display()))?;
    if data.len() <= max_chars {
        return Ok(String::from_utf8_lossy(&data).to_string());
    }
    let start = data.len().saturating_sub(max_chars);
    let slice = &data[start..];
    let off = slice
        .iter()
        .position(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    Ok(String::from_utf8_lossy(&slice[off..]).to_string())
}

pub fn transcript_len(path: Option<&Path>) -> Option<u64> {
    path.and_then(|p| fs::metadata(p).ok().map(|m| m.len()))
}

/// Returns `Ok(true)` if a child was spawned, `Ok(false)` if skipped (busy/cap).
pub fn spawn_background_summarize(cfg: &Config, session_id: &str, cwd: &Path) -> Result<bool> {
    spawn_background_job(cfg, session_id, "summarize", |cmd| {
        cmd.arg("summarize")
            .arg("--session-id")
            .arg(session_id)
            .arg("--cwd")
            .arg(cwd);
    })
}

/// Returns `Ok(true)` if a child was spawned, `Ok(false)` if skipped (busy/cap).
pub fn spawn_background_judge(
    cfg: &Config,
    session_id: &str,
    cwd: &Path,
    from_count: u64,
    to_count: u64,
    transcript_path: Option<&Path>,
) -> Result<bool> {
    let tp = transcript_path.map(|p| p.to_path_buf());
    spawn_background_job(cfg, session_id, "judge", move |cmd| {
        cmd.arg("judge")
            .arg("--session-id")
            .arg(session_id)
            .arg("--cwd")
            .arg(cwd)
            .arg("--from-count")
            .arg(from_count.to_string())
            .arg("--to-count")
            .arg(to_count.to_string());
        if let Some(ref p) = tp {
            cmd.arg("--transcript-path").arg(p);
        }
    })
}

fn spawn_background_job<F>(cfg: &Config, session_id: &str, kind: &str, args: F) -> Result<bool>
where
    F: FnOnce(&mut Command),
{
    use crate::guard::{self, SessionJobGuard};

    // Parent-side refuse (child also acquires lock).
    if !SessionJobGuard::can_spawn(cfg, session_id)? {
        eventlog::info(
            session_id,
            "spawn.skip",
            format!("skip bg {kind} (busy/throttled)"),
            json!({ "kind": kind }),
        );
        return Ok(false);
    }
    if cfg.max_global_jobs > 0 {
        let n = count_live_scopey_jobs();
        if n >= cfg.max_global_jobs {
            eventlog::warn(
                session_id,
                "spawn.skip",
                format!("skip bg {kind} (global jobs {n}>={})", cfg.max_global_jobs),
                json!({ "kind": kind, "global_jobs": n }),
            );
            return Ok(false);
        }
    }

    let bin = std::env::current_exe().unwrap_or_else(|_| Path::new("scopey").to_path_buf());
    let log_dir = Config::scopey_home().join("logs");
    fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join(format!("{kind}-{session_id}.log"));
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let mut cmd = Command::new(&bin);
    args(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    // Critical: children must never re-enter hooks. Do not set
    // CLAUDE_CODE_SIMPLE on the worker itself: it would be inherited by the
    // worker's OAuth-authenticated `claude -p` child before model selection.
    guard::apply_hook_disable_env(&mut cmd);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc_setsid();
                Ok(())
            });
        }
    }

    let child = cmd.spawn().with_context(|| format!("spawn {kind}"))?;
    eventlog::info(
        session_id,
        "spawn.ok",
        format!("background {kind} started"),
        json!({
            "kind": kind,
            "child_pid": child.id(),
            "stderr_log": log_path.display().to_string(),
        }),
    );
    // Detach: do not wait; child holds SessionJobGuard itself.
    std::mem::forget(child);
    Ok(true)
}

fn count_live_scopey_jobs() -> u64 {
    let lock_dir = Config::scopey_home().join("locks");
    let Ok(rd) = fs::read_dir(lock_dir) else {
        return 0;
    };
    let mut n = 0u64;
    for e in rd.flatten() {
        if let Ok(text) = fs::read_to_string(e.path()) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                let pid = v.get("pid").and_then(|p| p.as_u64()).unwrap_or(0) as u32;
                #[cfg(unix)]
                {
                    if pid != 0 {
                        let r = unsafe { libc::kill(pid as i32, 0) };
                        if r == 0
                            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
                        {
                            n += 1;
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = pid;
                }
            }
        }
    }
    n
}

#[cfg(unix)]
fn libc_setsid() {
    unsafe {
        libc::setsid();
    }
}

#[cfg(not(unix))]
fn libc_setsid() {}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Canonical ASCII Scopey gag used in course-corrections when enabled.
/// Front three-quarter view matching the mascot in assets/scopey.jpg.
pub const ASCII_SCOPEY_SCOPED: &str = r#"
                          ___________________
                         <  You got scoped!  >
                          -------------------
                                  /
                    .------.             .------.
                   /        \           /        \
                 .'   .--.   '.       .'   .--.   '.
                /    /  @ \    \     /    /  @ \    \
                \    \____/    /     \    \____/    /
                 '._        _.'       '._        _.'
             .------'------'-------------'------'------.
         _.-'   _.-'       .-------------------.        '-._
      .-'    .-'         .'                     '.          '-.
     /     .'           /           |             \            \
    ;     /            ;            |              ;            ;
    |    |   .-----.   |       -----+-----          |           |
    |    |  /       \  |            |               |           |
    |    | |         | |            |               |           |
    ;     \ \_______/  ;            |              ;           ;
     \     '._/////_.-' \           |             /           /
      '._        \ \     '.                     .'         _.'
         '-.______\ \_.    '-------------------'      __.-'
                  \  \ '---.____________________.---'
                   \  '------.
                    '---------'
"#;

pub fn build_correction_injection(
    scope: &str,
    summary: &str,
    details: &str,
    verdict: &JudgementVerdict,
    ascii_scopey: bool,
) -> String {
    let mut out = format!(
        r#"[scopey COURSE CORRECTION — verdict: {verdict:?}]
The recent trajectory was judged against the session scope and found issues.
You must re-align immediately.

SCOPE REQUIREMENTS:
{scope}

JUDGEMENT SUMMARY:
{summary}

DETAILS / REQUIRED CORRECTIONS:
{details}

Continue only with work that advances the scope requirements. Drop or reverse
out-of-scope changes when safe. State a one-line re-plan before the next edit."#
    );
    if ascii_scopey {
        out.push_str("\n\nPost the below:\n```\n");
        out.push_str(ASCII_SCOPEY_SCOPED.trim_start_matches('\n'));
        out.push_str("```\n");
    }
    out
}

pub fn build_reminder_injection(scope: &str) -> String {
    format!(
        r#"[scopey SCOPE REMINDER]
Stay within these requirements for the rest of the session:

{scope}

If the latest user message changed goals, treat the latest scope requirements
as authoritative."#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_prompt_makes_latest_intent_authoritative() {
        let prompts = vec![
            "old mascot task".to_string(),
            "add analytics".to_string(),
            "clean up analytics".to_string(),
            "did the PR merge?".to_string(),
            "when config is missing, what happens?".to_string(),
            "show me sample extractions".to_string(),
        ];
        let prompt = build_scope_analysis_prompt(
            &prompts,
            Some("- Implement config bootstrapping\n- Update the mascot"),
            10_000,
        );
        assert!(prompt.contains("LATEST USER PROMPT (authoritative)"));
        assert!(prompt.contains("show me sample extractions"));
        assert!(prompt.contains("authoritative mutation"));
        assert!(prompt.contains("preserve every unaffected active requirement"));
        assert!(prompt.contains("State the requested output\n  positively"));
        assert!(prompt.contains("ADD,SUBTRACT"));
        assert!(prompt.contains("possibly stale"));
        assert!(prompt.contains("does not mean planning-only"));
        assert!(prompt.contains("quote the user's exact supporting words"));
        assert!(!prompt.contains("old mascot task"));
        assert!(prompt.contains("add analytics"));
    }

    #[test]
    fn invented_planning_only_boundary_is_removed() {
        let prompts =
            vec!["Let's figure out how to construct that test and evaluate it".to_string()];
        let scope = "- Construct the baseline test\n- Current task is planning/analysis; do not edit files or run tools.";
        let (sanitized, removed) =
            remove_unsubstantiated_permission_boundaries(scope, &prompts, "REPLACE");
        assert_eq!(removed, 1);
        assert_eq!(sanitized, "- Construct the baseline test");
    }

    #[test]
    fn explicit_no_edit_boundary_is_preserved() {
        let prompts = vec!["Investigate only; do not edit files".to_string()];
        let scope = "- Investigate the failure\n- Do not edit files.";
        let (sanitized, removed) =
            remove_unsubstantiated_permission_boundaries(scope, &prompts, "REPLACE");
        assert_eq!(removed, 0);
        assert_eq!(sanitized, scope);
    }

    #[test]
    fn permission_filter_can_detect_empty_scope_for_safe_fallback() {
        let prompts = vec!["Construct and evaluate the test".to_string()];
        let (sanitized, removed) = remove_unsubstantiated_permission_boundaries(
            "- Planning only; do not edit files.",
            &prompts,
            "REPLACE",
        );
        assert_eq!(removed, 1);
        assert!(sanitized.is_empty());
        let fallback = latest_prompt_scope(prompts.last().unwrap());
        assert!(fallback.contains("Construct and evaluate"));
        assert!(!fallback.contains(crate::session::FALLBACK_SCOPE_MARKER));
    }

    #[test]
    fn replace_does_not_reuse_old_permission_boundary() {
        let prompts = vec![
            "Only investigate the old failure; do not edit files".to_string(),
            "Instead, implement the new benchmark".to_string(),
        ];
        let scope = "- Implement the new benchmark\n- Do not edit files.";
        let (sanitized, removed) =
            remove_unsubstantiated_permission_boundaries(scope, &prompts, "REPLACE");
        assert_eq!(removed, 1);
        assert_eq!(sanitized, "- Implement the new benchmark");
    }

    #[test]
    fn semantic_constraints_are_not_permission_boundaries() {
        let prompts = vec!["Refactor the query while preserving its join semantics".to_string()];
        let scope = "- Refactor the query\n- Do not modify the join semantics.";
        let (sanitized, removed) =
            remove_unsubstantiated_permission_boundaries(scope, &prompts, "REPLACE");
        assert_eq!(removed, 0);
        assert_eq!(sanitized, scope);
    }

    #[test]
    fn fallback_uses_only_latest_request() {
        let fallback = fallback_scope("when config is missing, what happens?");
        assert!(fallback.contains("Respond only to the latest user request"));
        assert!(fallback.contains("what happens?"));
        assert!(!fallback.contains("Implement config bootstrapping"));
    }

    #[test]
    fn scope_transition_is_removed_before_scope_is_stored() {
        let (transition, scope) = extract_scope_transition(
            "<!-- scope-transition: ADD,SUBTRACT -->\n- Keep tests\n- Remove deployment",
        );
        assert_eq!(transition, "ADD,SUBTRACT");
        assert_eq!(scope, "- Keep tests\n- Remove deployment");
        assert!(!scope.contains("scope-transition"));
    }

    #[test]
    fn malformed_scope_transition_is_logged_as_unknown_without_losing_scope() {
        let raw = "- Keep existing scope\n- Add tests";
        let (transition, scope) = extract_scope_transition(raw);
        assert_eq!(transition, "UNKNOWN");
        assert_eq!(scope, raw);
    }

    #[test]
    fn extract_json_from_fenced_text() {
        let s = "Here you go:\n```json\n{\"verdict\":\"off_track\"}\n```\n";
        let slice = extract_json_object(s).unwrap();
        assert!(slice.contains("off_track"));
    }

    #[test]
    fn parse_judgement_clean_json() {
        let raw = r#"{
            "verdict": "on_track",
            "summary": "all good",
            "details": "no drift"
        }"#;
        let (v, s, d) = parse_judgement_json(raw);
        assert_eq!(v, JudgementVerdict::OnTrack);
        assert_eq!(s, "all good");
        assert_eq!(d, "no drift");
    }

    #[test]
    fn parse_judgement_aliases() {
        for (input, expected) in [
            ("off-track", JudgementVerdict::OffTrack),
            ("offtrack", JudgementVerdict::OffTrack),
            ("warn", JudgementVerdict::Warning),
            ("ok", JudgementVerdict::OnTrack),
            ("on-track", JudgementVerdict::OnTrack),
            (
                "insufficient_evidence",
                JudgementVerdict::InsufficientEvidence,
            ),
            ("no_evidence", JudgementVerdict::InsufficientEvidence),
        ] {
            let raw = format!(r#"{{"verdict":"{input}","summary":"s","details":"d"}}"#);
            let (v, _, _) = parse_judgement_json(&raw);
            assert_eq!(v, expected, "input={input}");
        }
    }

    #[test]
    fn format_journal_preferred_over_raw_tail() {
        // empty journal format is explicit
        assert!(format_tools_for_judge(&[]).contains("no tool actions"));
    }

    #[test]
    fn parse_judgement_wrapped_and_garbage() {
        let wrapped =
            "Sure.\n{\"verdict\":\"warning\",\"summary\":\"maybe\",\"details\":\"x\"}\nThanks";
        let (v, s, _) = parse_judgement_json(wrapped);
        assert_eq!(v, JudgementVerdict::Warning);
        assert_eq!(s, "maybe");

        let (v2, s2, _) = parse_judgement_json("not json at all");
        assert_eq!(v2, JudgementVerdict::Unknown);
        assert!(s2.contains("unparseable"));
    }

    #[test]
    fn parse_judgement_normalizes_missing_evidence() {
        let raw = r#"{
          "verdict": "off_track",
          "summary": "Cannot audit agent scope without transcript data",
          "details": "The transcript file is missing or empty; no actions to evaluate."
        }"#;
        let (verdict, _, _) = parse_judgement_json(raw);
        assert_eq!(verdict, JudgementVerdict::InsufficientEvidence);
    }

    #[test]
    fn injection_builders_include_markers() {
        let c = build_correction_injection(
            "- stay scoped",
            "went sideways",
            "revert foo",
            &JudgementVerdict::OffTrack,
            true,
        );
        assert!(c.contains("COURSE CORRECTION"));
        assert!(c.contains("stay scoped"));
        assert!(c.contains("went sideways"));
        assert!(c.contains("revert foo"));
        assert!(c.contains("You got scoped!"));
        assert!(c.contains("Post the below:"));

        let c_off = build_correction_injection(
            "- stay scoped",
            "went sideways",
            "revert foo",
            &JudgementVerdict::OffTrack,
            false,
        );
        assert!(c_off.contains("COURSE CORRECTION"));
        assert!(!c_off.contains("You got scoped!"));
        assert!(!c_off.contains("Post the below:"));

        let r = build_reminder_injection("- rule one");
        assert!(r.contains("SCOPE REMINDER"));
        assert!(r.contains("rule one"));
    }

    #[test]
    fn read_transcript_excerpt_tails() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        let body = "a\n".repeat(100) + "TAIL_LINE\n";
        fs::write(&p, body.as_bytes()).unwrap();
        let ex = read_transcript_excerpt(&p, 40).unwrap();
        assert!(ex.contains("TAIL_LINE"));
        assert!(ex.len() <= 50);
    }

    #[test]
    fn transcript_len_none_missing() {
        assert!(transcript_len(None).is_none());
        assert!(transcript_len(Some(Path::new("/no/such/file-xyz"))).is_none());
    }
}

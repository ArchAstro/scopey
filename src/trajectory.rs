use crate::config::Config;
use crate::eventlog;
use crate::model;
use crate::notify;
use crate::session::{
    hash_prompt, JudgementStatus, JudgementVerdict, SessionMessage, SessionStore,
};
use anyhow::{Context, Result};
use serde_json::json;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

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
    let mut store = SessionStore::open_or_create(cfg, cwd, session_id, "")?;
    if let Some(p) = extra_prompt {
        if !p.trim().is_empty() {
            store.append(SessionMessage::user_prompt(p, hash_prompt(p)));
        }
    }
    let prompts = store.all_user_prompts();
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
    let clipped = clip(&joined, cfg.summarize_prompt_chars);

    let sys = format!(
        r#"You are a scope analyst for a coding agent session.
Read the user prompt(s) below and extract a tight SCOPE REQUIREMENTS checklist.

Rules:
- Bullet list only (markdown).
- Capture goals, constraints, out-of-scope boundaries, and done-when criteria.
- Prefer actionable requirements the agent must not violate.
- Max ~25 bullets. No preamble, no closing.

USER PROMPT(S):
{clipped}
"#
    );

    let harness = store.data.harness.clone();
    let out = match model::complete(cfg, &sys, &harness) {
        Ok(t) => t,
        Err(e) => {
            // Fallback: first 1500 chars as crude scope so hooks still work offline.
            eventlog::warn(
                session_id,
                "job.summarize.model",
                format!("model failed; using truncated prompt fallback: {e:#}"),
                json!({ "harness": harness }),
            );
            format!(
                "- (fallback scope — model unavailable)\n- Honor the user request:\n{}",
                clip(&joined, 1500)
            )
        }
    };

    let hash = hash_prompt(&joined);
    store.append(SessionMessage::scope_requirements(out.trim(), Some(hash)));
    store.persist()?;
    eventlog::info(
        session_id,
        "job.summarize.done",
        "wrote scope_requirements",
        json!({ "chars": out.len(), "prompt_count": prompts.len() }),
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
    let mut store = SessionStore::open_or_create(cfg, cwd, session_id, "")?;
    if let Some(tp) = transcript_path {
        store.set_transcript(Some(tp));
    }

    // Mark pending
    let pending = SessionMessage::judgement(
        from_count,
        to_count,
        JudgementVerdict::Unknown,
        JudgementStatus::Pending,
        "judging…",
        "",
    );
    let pending_id = pending.id.clone();
    store.upsert_judgement(pending);
    store.persist()?;

    let scope = store
        .latest_scope_requirements()
        .unwrap_or_else(|| "(no scope requirements recorded yet)".into());

    let tp = transcript_path
        .map(|p| p.to_path_buf())
        .or_else(|| {
            store
                .data
                .transcript_path
                .as_ref()
                .map(Path::new)
                .map(|p| p.to_path_buf())
        });

    let excerpt = match &tp {
        Some(p) if p.exists() => read_transcript_excerpt(p, cfg.judge_transcript_chars)?,
        Some(p) => format!("(transcript missing: {})", p.display()),
        None => "(no transcript_path)".into(),
    };

    let prompt = format!(
        r#"You are a strict scope auditor for a coding agent.

SCOPE REQUIREMENTS:
{scope}

TOOL-CALL WINDOW: counts [{from_count}, {to_count})
Recent trajectory / transcript excerpt (may be partial JSONL):
---
{excerpt}
---

Judge whether the agent is still working within the scope requirements.
Focus especially on file writes/edits and shell commands that change system state.

Respond with EXACTLY this JSON object (no markdown fences):
{{
  "verdict": "on_track" | "warning" | "off_track",
  "summary": "one sentence",
  "details": "2-6 sentences of evidence and what to do instead if off_track",
  "off_scope_actions": ["short list of problematic actions, or empty"]
}}
"#
    );

    let harness = store.data.harness.clone();
    let raw = match model::complete(cfg, &prompt, &harness) {
        Ok(t) => t,
        Err(e) => {
            let failed = SessionMessage::judgement(
                from_count,
                to_count,
                JudgementVerdict::Unknown,
                JudgementStatus::Failed,
                format!("judge model error: {e:#}"),
                "",
            );
            // remove pending
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

    let parsed = parse_judgement_json(&raw);
    let (verdict, summary, details) = parsed;

    // Drop pending placeholder
    if let Some(id) = &pending_id {
        store.data.messages.retain(|m| m.id.as_deref() != Some(id));
    }

    let ready = SessionMessage::judgement(
        from_count,
        to_count,
        verdict.clone(),
        JudgementStatus::Ready,
        summary.clone(),
        details.clone(),
    );
    store.upsert_judgement(ready);
    store.data.last_judged_to_count = to_count;
    store.persist()?;

    eventlog::info(
        session_id,
        "job.judge.done",
        format!("window [{from_count},{to_count}) → {summary}"),
        json!({
            "from": from_count,
            "to": to_count,
            "verdict": format!("{:?}", verdict),
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
            cwd: store.data.cwd.as_str(),
            from_count,
            to_count,
            harness: store.data.harness.as_str(),
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

pub fn read_transcript_excerpt(path: &Path, max_chars: usize) -> Result<String> {
    let data = fs::read(path).with_context(|| format!("read transcript {}", path.display()))?;
    // Prefer the tail (recent trajectory).
    if data.len() <= max_chars {
        return Ok(String::from_utf8_lossy(&data).to_string());
    }
    let start = data.len().saturating_sub(max_chars);
    // align to next newline
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

pub fn spawn_background_summarize(cfg: &Config, session_id: &str, cwd: &Path) -> Result<()> {
    spawn_background_job(cfg, session_id, "summarize", |cmd| {
        cmd.arg("summarize")
            .arg("--session-id")
            .arg(session_id)
            .arg("--cwd")
            .arg(cwd);
    })
}

pub fn spawn_background_judge(
    cfg: &Config,
    session_id: &str,
    cwd: &Path,
    from_count: u64,
    to_count: u64,
    transcript_path: Option<&Path>,
) -> Result<()> {
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

fn spawn_background_job<F>(cfg: &Config, session_id: &str, kind: &str, args: F) -> Result<()>
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
        return Ok(());
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
            return Ok(());
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
    // Critical: children must never re-enter hooks.
    guard::apply_internal_env(&mut cmd);

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
    Ok(())
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

pub fn build_correction_injection(
    scope: &str,
    summary: &str,
    details: &str,
    verdict: &JudgementVerdict,
) -> String {
    format!(
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
    )
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
        ] {
            let raw = format!(r#"{{"verdict":"{input}","summary":"s","details":"d"}}"#);
            let (v, _, _) = parse_judgement_json(&raw);
            assert_eq!(v, expected, "input={input}");
        }
    }

    #[test]
    fn parse_judgement_wrapped_and_garbage() {
        let wrapped = "Sure.\n{\"verdict\":\"warning\",\"summary\":\"maybe\",\"details\":\"x\"}\nThanks";
        let (v, s, _) = parse_judgement_json(wrapped);
        assert_eq!(v, JudgementVerdict::Warning);
        assert_eq!(s, "maybe");

        let (v2, s2, _) = parse_judgement_json("not json at all");
        assert_eq!(v2, JudgementVerdict::Unknown);
        assert!(s2.contains("unparseable"));
    }

    #[test]
    fn injection_builders_include_markers() {
        let c = build_correction_injection(
            "- stay scoped",
            "went sideways",
            "revert foo",
            &JudgementVerdict::OffTrack,
        );
        assert!(c.contains("COURSE CORRECTION"));
        assert!(c.contains("stay scoped"));
        assert!(c.contains("went sideways"));
        assert!(c.contains("revert foo"));

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


//! Integration tests: drive the installed `scopey` binary through hooks + logs.
//! Uses a private SCOPEY_HOME so it never touches the developer's real state.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn scopey_bin() -> PathBuf {
    // Prefer cargo's test binary path sibling: CARGO_BIN_EXE_scopey when available
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_scopey") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/scopey")
}

fn run_hook(home: &std::path::Path, sub: &str, payload: &str) -> std::process::Output {
    Command::new(scopey_bin())
        .args(["hook", sub])
        .env("SCOPEY_HOME", home)
        .env_remove("SCOPEY_INTERNAL")
        .env_remove("SCOPEY_HOOKS_DISABLED")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.take().unwrap().write_all(payload.as_bytes())?;
            c.wait_with_output()
        })
        .expect("spawn hook")
}

#[test]
fn internal_env_makes_hooks_noop() {
    let home = tempfile::tempdir().unwrap();
    let payload = r#"{"session_id":"int-1","cwd":"/tmp","prompt":"hello world"}"#;
    let out = Command::new(scopey_bin())
        .args(["hook", "user-prompt"])
        .env("SCOPEY_HOME", home.path())
        .env("SCOPEY_INTERNAL", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.take().unwrap().write_all(payload.as_bytes())?;
            c.wait_with_output()
        })
        .unwrap();
    assert!(out.status.success());
    // No session work file should be created under isolated home work root default
    // (work_root is under default config which points to home/work)
    let logs = home.path().join("logs");
    // With internal, we skip before logging user prompt (hooks_disabled returns before session_id parse... actually after read? hooks_disabled is first, before read_event - wait, hooks_disabled is first, returns Ok without writing logs)
    if logs.exists() {
        let entries: Vec<_> = fs::read_dir(&logs).unwrap().collect();
        // should not have written a session jsonl from this path
        assert!(
            entries.is_empty()
                || !entries.iter().any(|e| {
                    e.as_ref()
                        .ok()
                        .map(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
                        .unwrap_or(false)
                })
        );
    }
}

#[test]
fn user_prompt_writes_jsonl_and_logs_command() {
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path().join("proj");
    fs::create_dir_all(&cwd).unwrap();
    let sid = "cli-sess-prompt";
    let payload = format!(
        r#"{{"session_id":"{sid}","cwd":"{}","prompt":"only fix the typo in README","hook_event_name":"UserPromptSubmit"}}"#,
        cwd.display()
    );
    let out = run_hook(home.path(), "user-prompt", &payload);
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let log_path = home.path().join("logs").join(format!("{sid}.jsonl"));
    assert!(
        log_path.exists(),
        "expected log at {} stderr={}",
        log_path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = fs::read_to_string(&log_path).unwrap();
    assert!(text.contains("hook.user_prompt"));
    assert!(text.contains("cached user prompt"));

    // scopey logs --session
    let logs_out = Command::new(scopey_bin())
        .args(["logs", "--session", sid, "--level", "info"])
        .env("SCOPEY_HOME", home.path())
        .output()
        .unwrap();
    assert!(logs_out.status.success());
    let pretty = String::from_utf8_lossy(&logs_out.stdout);
    assert!(pretty.contains("hook.user_prompt") || pretty.contains("cached"));
}

#[test]
fn internal_prompt_not_cached() {
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path().join("proj");
    fs::create_dir_all(&cwd).unwrap();
    let sid = "cli-internal-prompt";
    let payload = format!(
        r#"{{"session_id":"{sid}","cwd":"{}","prompt":"You are a scope analyst for a coding agent session.\nRead prompts"}}"#,
        cwd.display()
    );
    let out = run_hook(home.path(), "user-prompt", &payload);
    assert!(out.status.success());
    let log_path = home.path().join("logs").join(format!("{sid}.jsonl"));
    if log_path.exists() {
        let text = fs::read_to_string(log_path).unwrap();
        assert!(text.contains("ignored internal") || text.contains("internal"));
    }
    // work store should not have user_prompt with analyst text as scope
    // find session file
    let work = home.path().join("work");
    if work.exists() {
        for e in walkdir_json(&work) {
            let t = fs::read_to_string(&e).unwrap_or_default();
            if t.contains(sid) {
                assert!(
                    !t.contains("You are a scope analyst") || t.contains("ignored"),
                    "must not treat internal prompt as user scope: {e:?}"
                );
            }
        }
    }
}

#[test]
fn post_tool_batch_counts_and_logs() {
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path().join("proj");
    fs::create_dir_all(&cwd).unwrap();
    let sid = "cli-post-tool";
    // seed a prompt so session exists
    let _ = run_hook(
        home.path(),
        "user-prompt",
        &format!(
            r#"{{"session_id":"{sid}","cwd":"{}","prompt":"stay on scope"}}"#,
            cwd.display()
        ),
    );
    let payload = format!(
        r#"{{
          "session_id":"{sid}",
          "cwd":"{}",
          "hook_event_name":"PostToolBatch",
          "tool_calls":[
            {{"tool_name":"Read"}},
            {{"tool_name":"Edit"}}
          ]
        }}"#,
        cwd.display()
    );
    let out = run_hook(home.path(), "post-tool", &payload);
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log_path = home.path().join("logs").join(format!("{sid}.jsonl"));
    assert!(log_path.exists());
    let text = fs::read_to_string(log_path).unwrap();
    assert!(text.contains("hook.post_tool") || text.contains("tool count") || text.contains("post_tool"));
}

#[test]
fn insights_reports_and_filters_off_scope_sessions() {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work/by-id");
    fs::create_dir_all(&work).unwrap();
    let config = home.path().join("config.toml");
    fs::write(
        &config,
        format!("work_root = '{}'\n", home.path().join("work").display()),
    )
    .unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    let data = serde_json::json!({
        "session_id": "insights-drift",
        "cwd": "/tmp/project",
        "harness": "codex",
        "created_at": now,
        "updated_at": now,
        "tool_call_count": 20,
        "messages": [
            {
                "type": "scope_requirements",
                "ts": now,
                "content": "- only edit the requested file"
            },
            {
                "type": "judgement",
                "ts": now,
                "from_count": 0,
                "to_count": 10,
                "verdict": "on_track",
                "status": "ready",
                "summary": "focused",
                "details": "read-only inspection"
            },
            {
                "type": "judgement",
                "ts": now,
                "from_count": 10,
                "to_count": 20,
                "verdict": "off_track",
                "status": "injected",
                "summary": "edited an unrelated file",
                "details": "the write exceeded the requested scope"
            }
        ]
    });
    fs::write(
        work.join("insights-drift.json"),
        serde_json::to_string_pretty(&data).unwrap(),
    )
    .unwrap();

    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let human = Command::new(scopey_bin())
        .args([
            "--config",
            config.to_str().unwrap(),
            "insights",
            "--off-scope",
            "--date",
        ])
        .arg(day)
        .env("SCOPEY_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        human.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&human.stderr)
    );
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.contains("OFF TRACK"));
    assert!(stdout.contains("insights-drift"));
    assert!(stdout.contains("edited an unrelated file"));
    assert!(stdout.contains("50.0%"));

    let json = Command::new(scopey_bin())
        .args([
            "--config",
            config.to_str().unwrap(),
            "insights",
            "--session",
            "insights-",
            "--json",
        ])
        .env("SCOPEY_HOME", home.path())
        .output()
        .unwrap();
    assert!(json.status.success());
    let report: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(report["totals"]["sessions"], 1);
    assert_eq!(report["totals"]["off_track"], 1);
    assert_eq!(report["sessions"][0]["session_id"], "insights-drift");
}

fn walkdir_json(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(root) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walkdir_json(&p));
        } else if p.extension().and_then(|x| x.to_str()) == Some("json") {
            out.push(p);
        }
    }
    out
}

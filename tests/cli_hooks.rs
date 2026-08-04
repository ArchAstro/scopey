//! Integration tests: drive the installed `scopey` binary through hooks + logs.
//! Uses a private SCOPEY_HOME so it never touches the developer's real state.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn scopey_bin() -> PathBuf {
    // Prefer cargo's test binary path sibling: CARGO_BIN_EXE_scopey when available
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_scopey") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/scopey")
}

fn run_hook(home: &std::path::Path, sub: &str, payload: &str) -> std::process::Output {
    run_hook_with_config(home, None, sub, payload)
}

fn run_hook_with_config(
    home: &std::path::Path,
    config: Option<&std::path::Path>,
    sub: &str,
    payload: &str,
) -> std::process::Output {
    let mut command = Command::new(scopey_bin());
    command
        .args(["hook", sub])
        .env("SCOPEY_HOME", home)
        .env_remove("SCOPEY_INTERNAL")
        .env_remove("SCOPEY_HOOKS_DISABLED");
    if let Some(config) = config {
        command.env("SCOPEY_CONFIG", config);
    }
    command
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

fn wait_for(timeout: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
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
    assert!(
        text.contains("hook.post_tool")
            || text.contains("tool count")
            || text.contains("post_tool")
    );
}

#[test]
fn new_prompt_suppresses_stale_correction_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path().join("proj");
    fs::create_dir_all(&cwd).unwrap();
    let sid = "cli-prompt-epoch-e2e";
    let config = home.path().join("config.toml");
    fs::write(
        &config,
        format!(
            r#"
work_root = "{}"
model_runner = "claude"
model_command = "printf '%s\\n' '<!-- scope-transition: REPLACE -->' '- Construct and evaluate the baseline test'"
min_job_interval_secs = 0
max_global_jobs = 1
n_tool_calls = 15
m_reminder = 9999
notify_on_off_track = false
notify_on_warning = false
notify_on_model_fallback = false
herdr_report_state = false
"#,
            home.path().join("work").display()
        ),
    )
    .unwrap();

    let first = format!(
        r#"{{"session_id":"{sid}","cwd":"{}","prompt":"write the evaluation plan","hook_event_name":"UserPromptSubmit"}}"#,
        cwd.display()
    );
    let first_out = run_hook_with_config(home.path(), Some(&config), "user-prompt", &first);
    assert!(
        first_out.status.success(),
        "first hook stderr={}",
        String::from_utf8_lossy(&first_out.stderr)
    );

    let store_path = home.path().join("work/by-id").join(format!("{sid}.json"));
    assert!(wait_for(Duration::from_secs(10), || {
        fs::read_to_string(&store_path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|store| store["messages"].as_array().cloned())
            .is_some_and(|messages| {
                messages
                    .iter()
                    .filter(|message| message["type"] == "scope_requirements")
                    .count()
                    == 1
            })
    }));

    let mut store: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&store_path).unwrap()).unwrap();
    let first_prompt_hash = store["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["type"] == "user_prompt")
        .and_then(|message| message["prompt_hash"].as_str())
        .unwrap()
        .to_string();
    store["tool_call_count"] = serde_json::json!(15);
    store["messages"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "type": "judgement",
            "ts": chrono::Utc::now().to_rfc3339(),
            "tool_count": 15,
            "from_count": 0,
            "to_count": 15,
            "verdict": "off_track",
            "status": "ready",
            "summary": "old work drifted",
            "details": "this verdict belongs to the previous user prompt",
            "prompt_hash": first_prompt_hash,
            "id": "stale-ready-judgement"
        }));
    fs::write(&store_path, serde_json::to_string_pretty(&store).unwrap()).unwrap();

    let second = format!(
        r#"{{"session_id":"{sid}","cwd":"{}","prompt":"Instead, figure out how to construct and evaluate the baseline test","hook_event_name":"UserPromptSubmit"}}"#,
        cwd.display()
    );
    let second_out = run_hook_with_config(home.path(), Some(&config), "user-prompt", &second);
    assert!(
        second_out.status.success(),
        "second hook stderr={}",
        String::from_utf8_lossy(&second_out.stderr)
    );

    let post_tool = format!(
        r#"{{"session_id":"{sid}","cwd":"{}","hook_event_name":"PostToolUse","tool_name":"Read","tool_input":{{"file_path":"README.md"}}}}"#,
        cwd.display()
    );
    let post_out = run_hook_with_config(home.path(), Some(&config), "post-tool", &post_tool);
    assert!(post_out.status.success());
    let post_stdout = String::from_utf8_lossy(&post_out.stdout);
    assert!(
        !post_stdout.contains("COURSE CORRECTION"),
        "stale judgement was injected: {post_stdout}"
    );

    assert!(wait_for(Duration::from_secs(10), || {
        fs::read_to_string(&store_path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|store| store["messages"].as_array().cloned())
            .is_some_and(|messages| {
                messages
                    .iter()
                    .filter(|message| message["type"] == "scope_requirements")
                    .count()
                    == 2
            })
    }));

    let store: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&store_path).unwrap()).unwrap();
    assert_eq!(store["scope_epoch_start_tool_count"], 15);
    let stale = store["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == "stale-ready-judgement")
        .unwrap();
    assert_eq!(stale["status"], "failed");
    assert_eq!(stale["summary"], "superseded by a newer user prompt");
    let latest_scope = store["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["type"] == "scope_requirements")
        .and_then(|message| message["content"].as_str())
        .unwrap();
    assert!(latest_scope.contains("Construct and evaluate"));
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
    let ghost = serde_json::json!({
        "session_id": "ghost-empty",
        "cwd": "/tmp/project",
        "harness": "claude",
        "created_at": now,
        "updated_at": now,
        "tool_call_count": 0,
        "messages": [{
            "type": "scope_requirements",
            "ts": now,
            "content": "```json {\"verdict\":\"warning\",\"summary\":\"captured judge output\"} ```"
        }]
    });
    fs::write(
        work.join("ghost-empty.json"),
        serde_json::to_string_pretty(&ghost).unwrap(),
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
    assert!(stdout.contains("evaluation coverage: 1/1 sessions"));
    assert!(stdout.contains("1 zero-tool session store(s) excluded"));

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
    assert_eq!(report["excluded_empty_sessions"], 0);
    assert_eq!(report["sessions"][0]["session_id"], "insights-drift");

    let ghost_json = Command::new(scopey_bin())
        .args([
            "--config",
            config.to_str().unwrap(),
            "insights",
            "--session",
            "ghost-",
            "--json",
        ])
        .env("SCOPEY_HOME", home.path())
        .output()
        .unwrap();
    assert!(ghost_json.status.success());
    let ghost_report: serde_json::Value = serde_json::from_slice(&ghost_json.stdout).unwrap();
    assert_eq!(ghost_report["totals"]["sessions"], 1);
    assert_eq!(ghost_report["excluded_empty_sessions"], 0);
    assert_eq!(ghost_report["sessions"][0]["scope_quality"], "contaminated");
}

/// Like run_hook, but with an explicit config so work_root stays inside the
/// isolated home (the default config would use the developer's real
/// ~/.scopey/config.toml work_root).
fn run_hook_cfg(
    home: &std::path::Path,
    config: &std::path::Path,
    sub: &str,
    payload: &str,
) -> std::process::Output {
    Command::new(scopey_bin())
        .args(["--config", config.to_str().unwrap(), "hook", sub])
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
fn subagent_events_are_ignored_across_harness_shapes() {
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path().join("proj");
    fs::create_dir_all(&cwd).unwrap();
    let config = home.path().join("config.toml");
    fs::write(
        &config,
        format!("work_root = '{}'\n", home.path().join("work").display()),
    )
    .unwrap();

    // Claude Code: agent_id is present on every hook fired inside a Task
    // subagent; the session id stays the parent's, so without the guard this
    // event would advance the parent's tool count and could inject reminders
    // into the subagent's context.
    let claude_subagent = format!(
        r#"{{
          "session_id":"parent-sess",
          "cwd":"{0}",
          "hook_event_name":"PostToolBatch",
          "agent_id":"agent-def456",
          "agent_type":"Explore",
          "tool_calls":[{{"tool_name":"Read"}},{{"tool_name":"Bash"}}]
        }}"#,
        cwd.display()
    );
    let out = run_hook_cfg(home.path(), &config, "post-tool", &claude_subagent);
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "subagent event must never inject: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Subagent user prompts are the orchestrator's instructions, never scope.
    let claude_subagent_prompt = format!(
        r#"{{"session_id":"parent-sess","cwd":"{0}","prompt":"explore the repo","agent_id":"agent-def456","agent_type":"Explore"}}"#,
        cwd.display()
    );
    let out = run_hook_cfg(home.path(), &config, "user-prompt", &claude_subagent_prompt);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());

    // OpenCode: child sessions carry a parent id (camelCase spelling).
    let opencode_child = format!(
        r#"{{"sessionId":"child-1","parentID":"parent-1","cwd":"{0}","hook_event_name":"PostToolUse","tool_name":"bash","harness":"opencode"}}"#,
        cwd.display()
    );
    let out = run_hook_cfg(home.path(), &config, "post-tool", &opencode_child);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());

    // Adapter-tagged subagent (pi/opencode escape hatch).
    let tagged = format!(
        r#"{{"session_id":"pi-child","subagent":true,"cwd":"{0}","hook_event_name":"PostToolUse","tool_name":"bash","harness":"pi"}}"#,
        cwd.display()
    );
    let out = run_hook_cfg(home.path(), &config, "post-tool", &tagged);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());

    // None of the above may create session work stores.
    let work = home.path().join("work");
    let stores = if work.exists() {
        walkdir_json(&work)
    } else {
        Vec::new()
    };
    assert!(
        stores.is_empty(),
        "subagent events must not create session stores: {stores:?}"
    );

    // A plain main-session event on the same parent id still works.
    let main_event = format!(
        r#"{{"session_id":"parent-sess","cwd":"{0}","hook_event_name":"PostToolBatch","tool_calls":[{{"tool_name":"Read"}}]}}"#,
        cwd.display()
    );
    let out = run_hook_cfg(home.path(), &config, "post-tool", &main_event);
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stores = walkdir_json(&home.path().join("work"));
    assert!(
        stores.iter().any(|p| {
            fs::read_to_string(p)
                .unwrap_or_default()
                .contains("parent-sess")
        }),
        "main-session event must still create its store: {stores:?}"
    );
}

#[test]
fn subagent_guard_can_be_disabled_in_config() {
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path().join("proj");
    fs::create_dir_all(&cwd).unwrap();
    fs::write(
        home.path().join("config.toml"),
        format!(
            "ignore_subagents = false\nwork_root = '{}'\n",
            home.path().join("work").display()
        ),
    )
    .unwrap();
    let payload = format!(
        r#"{{"session_id":"parent-sess","cwd":"{0}","hook_event_name":"PostToolBatch","agent_id":"agent-x","tool_calls":[{{"tool_name":"Read"}}]}}"#,
        cwd.display()
    );
    let out = Command::new(scopey_bin())
        .args([
            "--config",
            home.path().join("config.toml").to_str().unwrap(),
            "hook",
            "post-tool",
        ])
        .env("SCOPEY_HOME", home.path())
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
        .expect("spawn hook");
    assert!(out.status.success());
    let stores = walkdir_json(&home.path().join("work"));
    assert!(
        stores.iter().any(|p| {
            fs::read_to_string(p)
                .unwrap_or_default()
                .contains("parent-sess")
        }),
        "with ignore_subagents=false the event must be processed: {stores:?}"
    );
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

/// Regression: `drain_pending_jobs` used to re-open the session store while it
/// already held the store's flock, deadlocking against itself for the full
/// 30s `JOB_LOCK_WAIT` — long past the 15s harness hook timeout — whenever a
/// deferred judge could not be spawned (busy guard or global job cap). The
/// harness then killed the hook ("hook timed out after 15s") and the deferred
/// judge window was silently dropped. Every hook that drains (user-prompt,
/// post-tool, stop) was exposed; `stop` exercises it deterministically because
/// no scope-epoch reset clears the pending window first. The hook must return
/// promptly and keep the deferred judge queued.
#[test]
fn drain_restores_pending_judge_without_self_deadlock() {
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path().join("proj");
    fs::create_dir_all(&cwd).unwrap();
    let sid = "cli-drain-deadlock";
    let config = home.path().join("config.toml");
    fs::write(
        &config,
        format!(
            r#"
work_root = "{}"
model_runner = "claude"
model_command = "printf '%s\\n' '- Investigate the drain deadlock'"
min_job_interval_secs = 0
max_global_jobs = 1
n_tool_calls = 15
m_reminder = 9999
notify_on_off_track = false
notify_on_warning = false
notify_on_model_fallback = false
herdr_report_state = false
"#,
            home.path().join("work").display()
        ),
    )
    .unwrap();

    // First prompt creates the session store; wait for its summarize to finish
    // so the per-session job guard is free again.
    let first = format!(
        r#"{{"session_id":"{sid}","cwd":"{}","prompt":"start the work","hook_event_name":"UserPromptSubmit"}}"#,
        cwd.display()
    );
    let first_out = run_hook_with_config(home.path(), Some(&config), "user-prompt", &first);
    assert!(
        first_out.status.success(),
        "first hook stderr={}",
        String::from_utf8_lossy(&first_out.stderr)
    );
    let store_path = home.path().join("work/by-id").join(format!("{sid}.json"));
    assert!(wait_for(Duration::from_secs(10), || {
        fs::read_to_string(&store_path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|store| store["messages"].as_array().cloned())
            .is_some_and(|messages| {
                messages
                    .iter()
                    .any(|message| message["type"] == "scope_requirements")
            })
    }));

    // Queue a deferred judge window, exactly what a throttled PostToolBatch
    // hook leaves behind.
    let mut store: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&store_path).unwrap()).unwrap();
    store["pending_judge"] = serde_json::json!({ "from_count": 15, "to_count": 30 });
    fs::write(&store_path, serde_json::to_string_pretty(&store).unwrap()).unwrap();

    // Occupy the single global job slot with a live foreign job so every spawn
    // attempt is refused and drain takes its restore path.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let locks = home.path().join("locks");
    fs::create_dir_all(&locks).unwrap();
    fs::write(
        locks.join("other-session.job.lock"),
        serde_json::json!({
            "pid": std::process::id(),
            "kind": "summarize",
            "session_id": "other-session",
            "started_unix": now,
            "last_acquire_unix": now,
        })
        .to_string(),
    )
    .unwrap();

    let stop = format!(
        r#"{{"session_id":"{sid}","cwd":"{}","hook_event_name":"Stop"}}"#,
        cwd.display()
    );
    let started = Instant::now();
    let stop_out = run_hook_with_config(home.path(), Some(&config), "stop", &stop);
    let elapsed = started.elapsed();
    assert!(
        stop_out.status.success(),
        "stop hook stderr={}",
        String::from_utf8_lossy(&stop_out.stderr)
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "stop hook took {elapsed:?}; it must stay far below the 15s harness hook timeout"
    );

    let store: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&store_path).unwrap()).unwrap();
    assert_eq!(
        store["pending_judge"]["from_count"], 15,
        "deferred judge window was lost instead of restored: {}",
        store["pending_judge"]
    );
    assert_eq!(store["pending_judge"]["to_count"], 30);
}

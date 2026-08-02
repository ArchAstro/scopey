//! End-to-end tests against the developer's real, locally authenticated model
//! CLIs. Ignored by default — run explicitly with:
//!
//!     make e2e-local
//!     # or: cargo test --test e2e_local -- --ignored --nocapture
//!
//! Every test drives the full production path: `scopey hook user-prompt` →
//! detached summarize worker → real `claude -p` / `codex exec`.
//!
//! - Exactly one test covers the poisoned flow (issue #15): the hook fires
//!   with `CLAUDE_CODE_SIMPLE=1`, as harness hook environments and older
//!   scopey workers set it, and the claude worker must still complete a real
//!   OAuth summarize instead of silently applying FALLBACK_LATEST.
//! - The concurrency tests run a clean environment: 10 sessions at once per
//!   runner, each required to invoke its cheap sub-model for real, log the
//!   full expected event trail, and leave every artifact in its expected
//!   location — session JSONL, worker stderr log, by-id store, health file —
//!   with nothing unexpected next to them.
//!
//! Isolation: each test uses its own temp `SCOPEY_HOME` + `SCOPEY_CONFIG`;
//! `~/.scopey` is never read or written. Only the model CLIs use the real
//! `$HOME` — deliberately, since the tests must exercise your actual auth.
//!
//! Determinism: fixed session ids and prompts, explicit env control (poison
//! vs clean), flock-serialized health counters asserted with exact values,
//! and set-equality checks on directory contents. The only nondeterministic
//! input is the model's text, which is never asserted on.
//!
//! Cost: cheap fast tier only (haiku, gpt-5.6-terra). A missing CLI skips
//! its tests; a present-but-broken CLI fails them loudly.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const WORKER_WAIT: Duration = Duration::from_secs(120);
const CONCURRENT_SESSIONS: usize = 10;
const CONCURRENCY_WAIT: Duration = Duration::from_secs(240);

fn scopey_bin() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_scopey") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/scopey")
}

fn cli_available(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn wait_for(timeout: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

fn read_or_empty(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// Pin the runner under test with its cheap fast-tier default as the primary
/// model, keep all scopey state under the temp home, and keep every
/// notification backend inert so a failing run cannot toast the developer's
/// desktop mid-test. `max_global_jobs = 0` lifts the worker cap so concurrency
/// tests genuinely run all sessions at once.
fn write_config(home: &Path, runner: &str, max_global_jobs: u64) -> PathBuf {
    let config_path = home.join("config.toml");
    fs::write(
        &config_path,
        format!(
            r#"
model_runner = "{runner}"
model = "auto"
model_timeout_secs = 90
max_global_jobs = {max_global_jobs}
notify_on_off_track = false
notify_on_warning = false
notify_on_model_fallback = false
notify_backend = "command"
notify_command = "true"
herdr_report_state = false
work_root = "{}"
"#,
            home.join("work").display()
        ),
    )
    .unwrap();
    config_path
}

/// Fire `scopey hook user-prompt` exactly the way a harness would. With
/// `poison`, the hook runs under `CLAUDE_CODE_SIMPLE=1` — the environment that
/// forces the nested claude CLI into API-key-only auth unless scopey scrubs it
/// before exec. Without it, the variable is explicitly removed so ambient
/// developer environment cannot leak in either direction.
fn fire_user_prompt(
    home: &Path,
    config: &Path,
    proj: &Path,
    sid: &str,
    prompt: &str,
    poison: bool,
) {
    let payload = format!(
        r#"{{"session_id":"{sid}","cwd":"{}","prompt":"{prompt}","hook_event_name":"UserPromptSubmit"}}"#,
        proj.display()
    );
    let mut cmd = Command::new(scopey_bin());
    cmd.args(["hook", "user-prompt"])
        .current_dir(proj)
        .env("SCOPEY_HOME", home)
        .env("SCOPEY_CONFIG", config)
        .env_remove("SCOPEY_INTERNAL")
        .env_remove("SCOPEY_HOOKS_DISABLED")
        .env_remove("CLAUDE_CODE_DISABLE_HOOKS");
    if poison {
        cmd.env("CLAUDE_CODE_SIMPLE", "1");
    } else {
        cmd.env_remove("CLAUDE_CODE_SIMPLE");
    }
    let out = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.take().unwrap().write_all(payload.as_bytes())?;
            c.wait_with_output()
        })
        .expect("spawn scopey hook user-prompt");
    assert!(
        out.status.success(),
        "hook failed for {sid}: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn session_log_path(home: &Path, sid: &str) -> PathBuf {
    home.join("logs").join(format!("{sid}.jsonl"))
}

fn worker_log_path(home: &Path, sid: &str) -> PathBuf {
    home.join("logs").join(format!("summarize-{sid}.log"))
}

fn session_events(log_path: &Path) -> Vec<serde_json::Value> {
    read_or_empty(log_path)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect()
}

/// Find the summarize transition event in the session JSONL, if logged yet.
fn summarize_transition(log_path: &Path) -> Option<serde_json::Value> {
    session_events(log_path)
        .into_iter()
        .find(|v| v["event"] == "job.summarize.transition")
}

/// Assert one session produced the full expected event trail, actually invoked
/// its sub-model CLI, extracted a real (non-fallback) scope, and stored it in
/// the right place.
fn assert_session_summarized(home: &Path, sid: &str, runner: &str) {
    let events = session_events(&session_log_path(home, sid));
    let names: Vec<&str> = events.iter().filter_map(|v| v["event"].as_str()).collect();

    for required in [
        "hook.user_prompt",
        "spawn.ok",
        "job.summarize.start",
        "job.summarize.transition",
        "job.summarize.done",
    ] {
        assert!(
            names.contains(&required),
            "session {sid}: missing {required} event; got {names:?}"
        );
    }
    assert!(
        !names.contains(&"spawn.skip"),
        "session {sid} was throttled instead of running: {names:?}"
    );
    assert!(
        !names.contains(&"job.summarize.model"),
        "session {sid}: model call failed;\n--- worker log:\n{}",
        read_or_empty(&worker_log_path(home, sid)),
    );
    assert!(
        !events.iter().any(|v| v["message"]
            .as_str()
            .unwrap_or("")
            .contains("defer summarize")),
        "session {sid} deferred its summarize instead of running it"
    );

    // Logs go to the right place: the worker stderr log scopey *reported* in
    // spawn.ok must be the file that actually exists under the temp home.
    let spawn_ok = events.iter().find(|v| v["event"] == "spawn.ok").unwrap();
    let reported = spawn_ok["fields"]["stderr_log"]
        .as_str()
        .unwrap_or_default();
    let expected = worker_log_path(home, sid);
    assert_eq!(
        fs::canonicalize(reported).ok(),
        fs::canonicalize(&expected).ok(),
        "session {sid}: spawn.ok reported stderr_log {reported:?}, expected {expected:?}"
    );

    // The sub-model CLI was actually invoked: the worker logs its resolved
    // runner/model choice right before exec.
    let worker_log = read_or_empty(&expected);
    assert!(
        worker_log.contains(&format!("runner={runner}")),
        "session {sid}: worker never invoked the {runner} sub-model;\n--- worker log:\n{worker_log}"
    );

    // The one assertion that matters: a real extraction, not FALLBACK_LATEST.
    let transition = events
        .iter()
        .find(|v| v["event"] == "job.summarize.transition")
        .unwrap();
    assert_eq!(
        transition["fields"]["fallback"],
        serde_json::Value::Bool(false),
        "session {sid} fell back — the runner lost credentials in the job \
         environment.\ntransition: {transition}\n--- worker log:\n{worker_log}",
    );

    // Stored scope lives in the by-id store for this exact session and is
    // model output, not the fallback echo.
    let store_path = home.join("work/by-id").join(format!("{sid}.json"));
    let store: serde_json::Value = serde_json::from_str(&read_or_empty(&store_path))
        .unwrap_or_else(|e| panic!("unreadable store {}: {e}", store_path.display()));
    assert_eq!(
        store["session_id"],
        serde_json::Value::String(sid.to_string()),
        "store {} belongs to the wrong session",
        store_path.display()
    );
    assert!(
        store["messages"]
            .as_array()
            .map(|ms| ms.iter().any(|m| m["type"] == "scope_requirements"))
            .unwrap_or(false),
        "no scope_requirements message in {}",
        store_path.display()
    );
    let store_text = store.to_string();
    assert!(
        !store_text.contains("fallback scope — model unavailable"),
        "session {sid} store contains the fallback marker: {store_text}"
    );
}

fn dir_names(path: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(path)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Deterministic layout check: logs/ holds exactly one eventlog and one worker
/// log per session, work/by-id/ holds exactly one store per session (plus its
/// lock/tmp siblings), and the health file sits at the home root. Any stray
/// file — a nested CLI re-entering hooks, a session landing outside the temp
/// home — fails the set comparison.
fn assert_home_layout(home: &Path, sids: &[String]) {
    let mut expected_logs: Vec<String> = sids
        .iter()
        .flat_map(|sid| [format!("{sid}.jsonl"), format!("summarize-{sid}.log")])
        .collect();
    expected_logs.sort();
    assert_eq!(
        dir_names(&home.join("logs")),
        expected_logs,
        "unexpected files in logs/"
    );

    let mut expected_stores: Vec<String> = sids.iter().map(|sid| format!("{sid}.json")).collect();
    expected_stores.sort();
    let stores: Vec<String> = dir_names(&home.join("work/by-id"))
        .into_iter()
        .filter(|name| name.ends_with(".json"))
        .collect();
    assert_eq!(
        stores, expected_stores,
        "unexpected session stores in work/by-id/"
    );

    assert!(
        home.join("model_health.json").exists(),
        "model_health.json missing from temp home root"
    );
}

/// Exact health counters — deterministic because updates are flock-serialized.
fn assert_health_exact(home: &Path, successes: u64) {
    let health: serde_json::Value =
        serde_json::from_str(&read_or_empty(&home.join("model_health.json")))
            .expect("model_health.json written by worker");
    assert_eq!(
        health["total_successes"].as_u64(),
        Some(successes),
        "health: {health}"
    );
    assert_eq!(
        health["total_failures"].as_u64(),
        Some(0),
        "health: {health}"
    );
    assert_eq!(
        health["consecutive_failures"].as_u64(),
        Some(0),
        "health: {health}"
    );
}

/// The single poisoned-flow test body: one session, hook fired with
/// CLAUDE_CODE_SIMPLE=1, worker must still summarize for real over OAuth.
fn run_poisoned_e2e(runner: &str, sid: &str) {
    if !cli_available(runner) {
        eprintln!("SKIP {runner} e2e: `{runner}` not on PATH");
        return;
    }

    let home = tempfile::tempdir().unwrap();
    let proj = home.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    let config_path = write_config(home.path(), runner, 2);

    fire_user_prompt(
        home.path(),
        &config_path,
        &proj,
        sid,
        "Fix the off-by-one bug in src/pager.rs and add a regression test. \
         Do not refactor anything else.",
        true,
    );

    let session_log = session_log_path(home.path(), sid);
    let done = wait_for(WORKER_WAIT, || summarize_transition(&session_log).is_some());
    assert!(
        done,
        "no job.summarize.transition within {WORKER_WAIT:?}\n--- {}:\n{}\n--- {}:\n{}",
        session_log.display(),
        read_or_empty(&session_log),
        worker_log_path(home.path(), sid).display(),
        read_or_empty(&worker_log_path(home.path(), sid)),
    );

    let sids = vec![sid.to_string()];
    assert_session_summarized(home.path(), sid, runner);
    assert_home_layout(home.path(), &sids);
    assert_health_exact(home.path(), 1);

    eprintln!("OK {runner} e2e: real summarize with fallback=false despite CLAUDE_CODE_SIMPLE=1");
}

/// Ten sessions hit the hook at once in a clean environment; every one must
/// spawn its own worker (no throttling with the cap lifted), invoke its cheap
/// sub-model for real, and leave the full expected event trail and artifacts
/// in its own session's files.
fn run_concurrency_e2e(runner: &str, sid_prefix: &str) {
    if !cli_available(runner) {
        eprintln!("SKIP {runner} concurrency e2e: `{runner}` not on PATH");
        return;
    }

    let home = tempfile::tempdir().unwrap();
    let proj = home.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    let config_path = write_config(home.path(), runner, 0);

    let handles: Vec<_> = (0..CONCURRENT_SESSIONS)
        .map(|i| {
            let home_dir = home.path().to_path_buf();
            let config = config_path.clone();
            let proj = proj.clone();
            let sid = format!("{sid_prefix}-{i:02}");
            std::thread::spawn(move || {
                let prompt = format!(
                    "Session {i}: add input validation to the /api/orders endpoint \
                     and return 400 on bad payloads. Touch nothing else."
                );
                fire_user_prompt(&home_dir, &config, &proj, &sid, &prompt, false);
                sid
            })
        })
        .collect();
    let sids: Vec<String> = handles
        .into_iter()
        .map(|h| h.join().expect("hook thread"))
        .collect();
    eprintln!("{runner} concurrency: {CONCURRENT_SESSIONS} hooks fired, waiting for workers…");

    let all_done = wait_for(CONCURRENCY_WAIT, || {
        sids.iter()
            .all(|sid| summarize_transition(&session_log_path(home.path(), sid)).is_some())
    });
    if !all_done {
        let pending: Vec<&String> = sids
            .iter()
            .filter(|sid| summarize_transition(&session_log_path(home.path(), sid)).is_none())
            .collect();
        let first = pending[0];
        panic!(
            "{}/{CONCURRENT_SESSIONS} sessions missing job.summarize.transition after \
             {CONCURRENCY_WAIT:?}: {pending:?}\n--- {first} session log:\n{}\n--- {first} worker log:\n{}",
            pending.len(),
            read_or_empty(&session_log_path(home.path(), first)),
            read_or_empty(&worker_log_path(home.path(), first)),
        );
    }

    for sid in &sids {
        assert_session_summarized(home.path(), sid, runner);
    }
    assert_home_layout(home.path(), &sids);
    assert_health_exact(home.path(), CONCURRENT_SESSIONS as u64);

    eprintln!(
        "OK {runner} concurrency e2e: {CONCURRENT_SESSIONS} concurrent sessions, \
         all sub-models invoked, all artifacts in place"
    );
}

#[test]
#[ignore = "needs local claude OAuth auth; run via make e2e-local"]
fn claude_oauth_summarize_survives_poisoned_hook_env() {
    run_poisoned_e2e("claude", "e2e-claude-oauth");
}

#[test]
#[ignore = "needs local claude OAuth auth; run via make e2e-local"]
fn claude_ten_concurrent_sessions_all_summarize() {
    run_concurrency_e2e("claude", "e2e-claude-conc");
}

#[test]
#[ignore = "needs local codex auth; run via make e2e-local"]
fn codex_ten_concurrent_sessions_all_summarize() {
    run_concurrency_e2e("codex", "e2e-codex-conc");
}

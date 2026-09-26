//! Scopey's real hook → background worker → model process opts out of ArchDev presence.
//! No provider or ArchDev service is contacted; the model process records its environment.

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn background_summary_disables_helper_presence_without_changing_parent_environment() {
    // Isolate config and session state. The model command is a local shell process
    // so this checks the actual inherited environment without an upstream model.
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    let observed = home.path().join("model-presence.txt");
    let model_command = r#"printf '%s' "$ARCHDEV_PRESENCE_DISABLED" > "$SCOPEY_HOME/model-presence.txt"; printf '%s\n' '- Keep helper presence disabled'"#;
    fs::write(
        &config,
        format!(
            r#"
work_root = {work_root:?}
model_runner = "claude"
model_command = {model_command:?}
min_job_interval_secs = 0
max_global_jobs = 0
notify_on_off_track = false
notify_on_warning = false
notify_on_model_fallback = false
herdr_report_state = false
"#,
            work_root = home.path().join("work").to_string_lossy(),
        ),
    )
    .unwrap();
    let original_parent_env = std::env::var_os("ARCHDEV_PRESENCE_DISABLED");

    // Start the public hook with presence enabled. It launches a real Scopey
    // summarize worker, which in turn starts the configured model shell.
    let mut hook = Command::new(env!("CARGO_BIN_EXE_scopey"))
        .args(["hook", "user-prompt"])
        .env("SCOPEY_HOME", home.path())
        .env("SCOPEY_CONFIG", &config)
        .env("ARCHDEV_PRESENCE_DISABLED", "0")
        .env_remove("SCOPEY_INTERNAL")
        .env_remove("SCOPEY_HOOKS_DISABLED")
        .env_remove("SCOPEY_SUBAGENT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let payload = serde_json::json!({
        "session_id": "presence-opt-out",
        "cwd": home.path(),
        "prompt": "Keep helper presence disabled",
        "hook_event_name": "UserPromptSubmit"
    });
    hook.stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let output = hook.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Wait for the durable summary, not merely hook exit: the asynchronous
    // worker must have completed its model call and saved the result.
    let session_path = home.path().join("work/by-id/presence-opt-out.json");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let completed = fs::read_to_string(&session_path)
            .ok()
            .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
            .and_then(|data| data["messages"].as_array().cloned())
            .is_some_and(|messages| messages.iter().any(|m| m["type"] == "scope_requirements"));
        if completed {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "background summary did not complete"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(fs::read_to_string(observed).unwrap(), "1");
    assert_eq!(
        std::env::var_os("ARCHDEV_PRESENCE_DISABLED"),
        original_parent_env
    );

    // An ordinary sibling process retains its explicit reporting setting.
    let sibling = Command::new("sh")
        .args(["-c", "printf '%s' \"$ARCHDEV_PRESENCE_DISABLED\""])
        .env("ARCHDEV_PRESENCE_DISABLED", "0")
        .output()
        .unwrap();
    assert!(sibling.status.success());
    assert_eq!(sibling.stdout, b"0");
}

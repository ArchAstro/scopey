use crate::config::{default_config_toml, Config};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Which harnesses to touch during setup/uninstall.
#[derive(Debug, Clone, Copy, Default)]
pub struct HarnessSet {
    pub claude: bool,
    pub codex: bool,
    pub grok: bool,
    pub pi: bool,
    pub opencode: bool,
}

/// Remove scopey from harness hooks; optionally purge data and signal leftover jobs.
pub fn run_uninstall(
    cfg: &Config,
    set: HarnessSet,
    purge_data: bool,
    kill_jobs: bool,
) -> Result<()> {
    if kill_jobs {
        match crate::guard::purge_leaked_jobs() {
            Ok(n) => println!("jobs: signaled {n} leaked process(es)"),
            Err(e) => println!("jobs: purge skipped ({e:#})"),
        }
    }

    if set.claude {
        uninstall_claude_hooks()?;
    }
    if set.codex {
        uninstall_codex_hooks()?;
    }
    if set.grok {
        uninstall_grok_hooks()?;
    }
    if set.pi {
        uninstall_pi_extension()?;
    }
    if set.opencode {
        uninstall_opencode_plugin()?;
    }

    if purge_data {
        let home = Config::scopey_home();
        if home.exists() {
            fs::remove_dir_all(&home).with_context(|| format!("remove {}", home.display()))?;
            println!("data: removed {}", home.display());
        } else {
            println!("data: {} already absent", home.display());
        }
    } else {
        println!(
            "data: kept {} (pass --purge-data to delete config/work/logs/locks)",
            Config::scopey_home().display()
        );
        println!("work: {}", cfg.work_root.display());
    }

    println!(
        "\nscopey uninstall complete.\n\
         Hooks removed from selected harnesses. The scopey binary itself is unchanged\n\
         (remove with `cargo uninstall scopey` if installed via cargo).\n"
    );
    Ok(())
}

pub(crate) fn is_scopey_hook_group(group: &Value) -> bool {
    group
        .pointer("/hooks")
        .and_then(|h| h.as_array())
        .map(|hs| {
            hs.iter().any(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c.contains("scopey") && c.contains("hook"))
            })
        })
        .unwrap_or(false)
}

pub(crate) fn strip_scopey_from_hooks_object(
    hooks_obj: &mut serde_json::Map<String, Value>,
) -> usize {
    let mut removed = 0usize;
    let keys: Vec<String> = hooks_obj.keys().cloned().collect();
    for key in keys {
        let Some(arr) = hooks_obj.get_mut(&key).and_then(|v| v.as_array_mut()) else {
            continue;
        };
        let before = arr.len();
        arr.retain(|g| !is_scopey_hook_group(g));
        removed += before.saturating_sub(arr.len());
        if arr.is_empty() {
            hooks_obj.remove(&key);
        }
    }
    removed
}

fn uninstall_claude_hooks() -> Result<()> {
    let path = dirs::home_dir()
        .context("home dir")?
        .join(".claude")
        .join("settings.json");
    if !path.exists() {
        println!("claude hooks: no {}", path.display());
        return Ok(());
    }
    let t = fs::read_to_string(&path)?;
    let mut root: Value = if t.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&t).with_context(|| format!("parse {}", path.display()))?
    };
    let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        println!("claude hooks: no hooks key in {}", path.display());
        return Ok(());
    };
    let n = strip_scopey_from_hooks_object(hooks);
    if hooks.is_empty() {
        if let Some(obj) = root.as_object_mut() {
            obj.remove("hooks");
        }
    }
    let text = serde_json::to_string_pretty(&root)?;
    fs::write(&path, text + "\n")?;
    println!(
        "claude hooks: removed {n} scopey group(s) from {}",
        path.display()
    );
    Ok(())
}

fn uninstall_codex_hooks() -> Result<()> {
    let path = dirs::home_dir()
        .context("home dir")?
        .join(".codex")
        .join("hooks.json");
    if !path.exists() {
        println!("codex hooks: no {}", path.display());
        return Ok(());
    }
    let t = fs::read_to_string(&path)?;
    let mut root: Value = if t.trim().is_empty() {
        json!({ "hooks": {} })
    } else {
        serde_json::from_str(&t).with_context(|| format!("parse {}", path.display()))?
    };
    let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        println!("codex hooks: no hooks key in {}", path.display());
        return Ok(());
    };
    let n = strip_scopey_from_hooks_object(hooks);
    let text = serde_json::to_string_pretty(&root)?;
    fs::write(&path, text + "\n")?;
    println!(
        "codex hooks: removed {n} scopey group(s) from {}",
        path.display()
    );
    Ok(())
}

pub fn run_setup(cfg: &Config, set: HarnessSet, force: bool, write_config: bool) -> Result<()> {
    if write_config {
        let p = Config::write_default_if_missing()?;
        println!("config: {}", p.display());
    }
    fs::create_dir_all(&cfg.work_root)?;
    fs::create_dir_all(Config::scopey_home().join("logs"))?;

    let bin = resolve_scopey_bin()?;
    println!("binary: {}", bin.display());

    if set.claude {
        install_claude_hooks(&bin, force)?;
    }
    if set.codex {
        install_codex_hooks(&bin, force)?;
    }
    if set.grok {
        install_grok_hooks(&bin, force)?;
    }
    if set.pi {
        install_pi_extension(&bin, force)?;
    }
    if set.opencode {
        install_opencode_plugin(&bin, force)?;
    }

    println!(
        "\nscopey setup complete.\n\
         Next:\n\
         1. Ensure `scopey` is on PATH (or hooks use absolute path above).\n\
         2. Run `scopey doctor`.\n\
         3. Codex: open `/hooks` and trust scopey commands.\n\
         4. Grok: global hooks in ~/.grok/hooks/ are trusted; project hooks need /hooks-trust.\n\
         5. Pi: restart Pi or /reload after extension install.\n\
         6. OpenCode: restart OpenCode to load ~/.config/opencode/plugins/scopey.js.\n\
         7. Target one harness: scopey setup --no-claude --no-codex --grok (etc).\n"
    );
    Ok(())
}

fn resolve_scopey_bin() -> Result<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if exe.exists() {
            return Ok(exe);
        }
    }
    if let Ok(p) = which::which("scopey") {
        return Ok(p);
    }
    Ok(PathBuf::from("scopey"))
}

fn scopey_hook_cmd(bin: &Path, sub: &str) -> String {
    // Quote path for shell form settings.
    format!("\"{}\" hook {sub}", bin.display())
}

fn install_claude_hooks(bin: &Path, force: bool) -> Result<()> {
    let path = dirs::home_dir()
        .context("home dir")?
        .join(".claude")
        .join("settings.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut root: Value = if path.exists() {
        let t = fs::read_to_string(&path)?;
        if t.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&t).with_context(|| format!("parse {}", path.display()))?
        }
    } else {
        json!({})
    };

    let hooks = root
        .as_object_mut()
        .context("settings.json root must be object")?
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let hooks_obj = hooks.as_object_mut().context("hooks must be object")?;

    // IMPORTANT: install PostToolBatch only (not PostToolUse). Installing both
    // double-counts tools and can double-spawn judges → process storms.
    let entries = [
        ("UserPromptSubmit", "user-prompt"),
        ("SessionStart", "session-start"),
        ("PostToolBatch", "post-tool"),
        ("Stop", "stop"),
    ];

    for (event, sub) in entries {
        let cmd = scopey_hook_cmd(bin, sub);
        ensure_claude_event(hooks_obj, event, &cmd, force)?;
    }

    // Strip legacy PostToolUse scopey hooks when force or when present (storm fix).
    if let Some(arr) = hooks_obj
        .get_mut("PostToolUse")
        .and_then(|v| v.as_array_mut())
    {
        let before = arr.len();
        arr.retain(|group| {
            !group
                .pointer("/hooks")
                .and_then(|h| h.as_array())
                .map(|hs| {
                    hs.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(|c| c.contains("scopey") && c.contains("hook"))
                    })
                })
                .unwrap_or(false)
        });
        if arr.len() != before {
            println!("  PostToolUse: removed legacy scopey hooks (use PostToolBatch only)");
        }
        if arr.is_empty() {
            hooks_obj.remove("PostToolUse");
        }
    }

    let text = serde_json::to_string_pretty(&root)?;
    fs::write(&path, text + "\n")?;
    println!("claude hooks: {}", path.display());
    Ok(())
}

fn ensure_claude_event(
    hooks_obj: &mut serde_json::Map<String, Value>,
    event: &str,
    command: &str,
    force: bool,
) -> Result<()> {
    let arr = hooks_obj
        .entry(event.to_string())
        .or_insert_with(|| json!([]));
    let list = arr.as_array_mut().context("hook event must be array")?;

    // Remove prior scopey hooks if force
    if force {
        list.retain(|group| {
            !group
                .pointer("/hooks")
                .and_then(|h| h.as_array())
                .map(|hs| {
                    hs.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(|c| c.contains("scopey") && c.contains("hook"))
                    })
                })
                .unwrap_or(false)
        });
    } else {
        let already = list.iter().any(|group| {
            group
                .pointer("/hooks")
                .and_then(|h| h.as_array())
                .map(|hs| {
                    hs.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(|c| c.contains("scopey") && c.contains("hook"))
                    })
                })
                .unwrap_or(false)
        });
        if already {
            println!("  {event}: scopey hook already present (use --force to replace)");
            return Ok(());
        }
    }

    list.push(json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": 15,
            "statusMessage": format!("scopey {event}")
        }]
    }));
    println!("  {event}: installed");
    Ok(())
}

fn install_grok_hooks(bin: &Path, force: bool) -> Result<()> {
    // Grok discovers ~/.grok/hooks/*.json (always trusted for global).
    let dir = dirs::home_dir()
        .context("home dir")?
        .join(".grok")
        .join("hooks");
    fs::create_dir_all(&dir)?;
    let path = dir.join("scopey.json");
    if path.exists() && !force {
        println!(
            "grok hooks: {} already present (use --force to replace)",
            path.display()
        );
        return Ok(());
    }
    let cmd = |sub: &str| scopey_hook_cmd(bin, sub);
    let root = json!({
        "description": "scopey lifecycle hooks for Grok Build",
        "hooks": {
            "SessionStart": [{
                "hooks": [{
                    "type": "command",
                    "command": cmd("session-start"),
                    "timeout": 15
                }]
            }],
            "UserPromptSubmit": [{
                "hooks": [{
                    "type": "command",
                    "command": cmd("user-prompt"),
                    "timeout": 15
                }]
            }],
            // Grok has no PostToolBatch; PostToolUse is passive (injection mainly via Stop).
            "PostToolUse": [{
                "hooks": [{
                    "type": "command",
                    "command": cmd("post-tool"),
                    "timeout": 15
                }]
            }],
            "Stop": [{
                "hooks": [{
                    "type": "command",
                    "command": cmd("stop"),
                    "timeout": 30
                }]
            }]
        }
    });
    fs::write(&path, serde_json::to_string_pretty(&root)? + "\n")?;
    println!("grok hooks: {}", path.display());
    println!("  note: mid-turn injection is limited on Grok (PostToolUse is passive); Stop injects corrections.");
    Ok(())
}

fn uninstall_grok_hooks() -> Result<()> {
    let path = dirs::home_dir()
        .context("home dir")?
        .join(".grok")
        .join("hooks")
        .join("scopey.json");
    if path.exists() {
        fs::remove_file(&path)?;
        println!("grok hooks: removed {}", path.display());
    } else {
        println!("grok hooks: no {}", path.display());
    }
    Ok(())
}

fn install_pi_extension(bin: &Path, force: bool) -> Result<()> {
    let dir = dirs::home_dir()
        .context("home dir")?
        .join(".pi")
        .join("agent")
        .join("extensions");
    fs::create_dir_all(&dir)?;
    let path = dir.join("scopey.ts");
    if path.exists() && !force {
        println!(
            "pi extension: {} already present (use --force to replace)",
            path.display()
        );
        return Ok(());
    }
    let mut body = include_str!("../../templates/pi_scopey_extension.ts").to_string();
    // Bake absolute bin path as default when available.
    if bin.is_absolute() {
        body = body.replace(
            "return process.env.SCOPEY_BIN || \"scopey\";",
            &format!(
                "return process.env.SCOPEY_BIN || {};",
                serde_json::to_string(&bin.display().to_string()).unwrap()
            ),
        );
    }
    fs::write(&path, body)?;
    println!("pi extension: {}", path.display());
    println!("  restart Pi or run /reload to load the extension");
    Ok(())
}

fn uninstall_pi_extension() -> Result<()> {
    let path = dirs::home_dir()
        .context("home dir")?
        .join(".pi")
        .join("agent")
        .join("extensions")
        .join("scopey.ts");
    if path.exists() {
        fs::remove_file(&path)?;
        println!("pi extension: removed {}", path.display());
    } else {
        println!("pi extension: no {}", path.display());
    }
    Ok(())
}

fn install_opencode_plugin(bin: &Path, force: bool) -> Result<()> {
    let dir = dirs::home_dir()
        .context("home dir")?
        .join(".config")
        .join("opencode")
        .join("plugins");
    fs::create_dir_all(&dir)?;
    let path = dir.join("scopey.js");
    if path.exists() && !force {
        println!(
            "opencode plugin: {} already present (use --force to replace)",
            path.display()
        );
        return Ok(());
    }
    let mut body = include_str!("../../templates/opencode_scopey_plugin.mjs").to_string();
    if bin.is_absolute() {
        body = body.replace(
            "return process.env.SCOPEY_BIN || \"scopey\";",
            &format!(
                "return process.env.SCOPEY_BIN || {};",
                serde_json::to_string(&bin.display().to_string()).unwrap()
            ),
        );
    }
    fs::write(&path, body)?;
    println!("opencode plugin: {}", path.display());
    println!("  restart OpenCode to load plugins from ~/.config/opencode/plugins/");
    Ok(())
}

fn uninstall_opencode_plugin() -> Result<()> {
    let path = dirs::home_dir()
        .context("home dir")?
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("scopey.js");
    if path.exists() {
        fs::remove_file(&path)?;
        println!("opencode plugin: removed {}", path.display());
    } else {
        println!("opencode plugin: no {}", path.display());
    }
    Ok(())
}

fn install_codex_hooks(bin: &Path, force: bool) -> Result<()> {
    let path = dirs::home_dir()
        .context("home dir")?
        .join(".codex")
        .join("hooks.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut root: Value = if path.exists() {
        let t = fs::read_to_string(&path)?;
        if t.trim().is_empty() {
            json!({ "hooks": {} })
        } else {
            serde_json::from_str(&t).with_context(|| format!("parse {}", path.display()))?
        }
    } else {
        json!({
            "description": "Codex lifecycle hooks (includes scopey)",
            "hooks": {}
        })
    };

    let hooks = root
        .as_object_mut()
        .context("hooks.json root object")?
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let hooks_obj = hooks.as_object_mut().context("hooks object")?;

    let entries = [
        ("UserPromptSubmit", "user-prompt"),
        ("SessionStart", "session-start"),
        ("PostToolUse", "post-tool"),
        ("Stop", "stop"),
    ];

    for (event, sub) in entries {
        let cmd = scopey_hook_cmd(bin, sub);
        ensure_codex_event(hooks_obj, event, &cmd, force)?;
    }

    let text = serde_json::to_string_pretty(&root)?;
    fs::write(&path, text + "\n")?;
    println!("codex hooks: {}", path.display());
    println!("  (open Codex `/hooks` and trust the new scopey commands)");
    Ok(())
}

fn ensure_codex_event(
    hooks_obj: &mut serde_json::Map<String, Value>,
    event: &str,
    command: &str,
    force: bool,
) -> Result<()> {
    let arr = hooks_obj
        .entry(event.to_string())
        .or_insert_with(|| json!([]));
    let list = arr.as_array_mut().context("array")?;

    if force {
        list.retain(|group| {
            !group
                .pointer("/hooks")
                .and_then(|h| h.as_array())
                .map(|hs| {
                    hs.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(|c| c.contains("scopey"))
                    })
                })
                .unwrap_or(false)
        });
    } else {
        let already = list.iter().any(|group| {
            group
                .pointer("/hooks")
                .and_then(|h| h.as_array())
                .map(|hs| {
                    hs.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(|c| c.contains("scopey"))
                    })
                })
                .unwrap_or(false)
        });
        if already {
            println!("  {event}: scopey hook already present (use --force to replace)");
            return Ok(());
        }
    }

    list.push(json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": 15,
            "statusMessage": format!("scopey {event}")
        }]
    }));
    println!("  {event}: installed");
    Ok(())
}

pub fn run_doctor(cfg: &Config) -> Result<()> {
    let mut failed = 0;

    check(
        "scopey binary",
        || {
            let exe = std::env::current_exe().ok();
            let which = which::which("scopey").ok();
            match (exe, which) {
                (Some(e), _) if e.exists() => Ok(format!("{}", e.display())),
                (_, Some(w)) => Ok(format!("{}", w.display())),
                _ => Err("not found on PATH".into()),
            }
        },
        &mut failed,
    );

    check(
        "config",
        || {
            let p = Config::user_config_path();
            if p.exists() {
                Ok(format!("{}", p.display()))
            } else {
                Err(format!("missing {} (run scopey setup)", p.display()))
            }
        },
        &mut failed,
    );

    check(
        "work_root",
        || {
            if cfg.work_root.exists() {
                Ok(format!("{}", cfg.work_root.display()))
            } else {
                fs::create_dir_all(&cfg.work_root).map_err(|e| e.to_string())?;
                Ok(format!("created {}", cfg.work_root.display()))
            }
        },
        &mut failed,
    );

    check(
        "model_runner",
        || {
            use crate::model::{resolve_choice, shipped_fast_defaults, Runner};
            const RUNNERS: [&str; 5] = ["claude", "codex", "grok", "pi", "opencode"];
            let mut lines = Vec::new();
            for (product, slug, note) in shipped_fast_defaults() {
                lines.push(format!("shipped {product} fast={slug} ({note})"));
            }
            for harness in ["claude", "codex", "grok", "pi", "opencode", "unknown"] {
                match resolve_choice(cfg, harness) {
                    Ok(c) => {
                        let bin_ok = which::which(c.runner.as_str()).is_ok();
                        lines.push(format!(
                            "harness={harness} → {} / {}{}",
                            c.runner.as_str(),
                            c.model,
                            if bin_ok { "" } else { " [binary missing]" }
                        ));
                    }
                    Err(e) => lines.push(format!("harness={harness} → {e}")),
                }
            }
            // Critical only if no supported binary exists for auto, or a pinned runner is missing.
            let pinned = cfg.model_runner.trim().to_ascii_lowercase();
            if pinned != "auto" && !pinned.is_empty() {
                let runner = Runner::parse(&pinned).ok_or_else(|| {
                    format!("unknown model_runner {pinned:?}; expected auto or a supported runner")
                })?;
                if which::which(runner.as_str()).is_err() {
                    return Err(format!(
                        "model_runner={pinned:?} but {} not on PATH",
                        runner.as_str()
                    ));
                }
            } else if RUNNERS.iter().all(|runner| which::which(runner).is_err()) {
                return Err(format!(
                    "no supported model runner on PATH (need {})",
                    RUNNERS.join("|")
                ));
            }
            Ok(lines.join("; "))
        },
        &mut failed,
    );

    check(
        "claude hooks",
        || {
            let p = dirs::home_dir().unwrap().join(".claude/settings.json");
            if !p.exists() {
                return Err("no ~/.claude/settings.json".into());
            }
            let t = fs::read_to_string(&p).map_err(|e| e.to_string())?;
            if t.contains("scopey") && t.contains("hook") {
                Ok("registered".into())
            } else {
                Err("scopey not found in settings (run scopey setup)".into())
            }
        },
        &mut failed,
    );

    // Other harnesses optional — warn only
    {
        let p = dirs::home_dir().unwrap().join(".codex/hooks.json");
        if !p.exists() {
            println!("WARN codex hooks: no ~/.codex/hooks.json");
        } else {
            match fs::read_to_string(&p) {
                Ok(t) if t.contains("scopey") => {
                    println!("OK   codex hooks: registered (trust via /hooks)");
                }
                Ok(_) => println!("WARN codex hooks: scopey not in hooks.json"),
                Err(e) => println!("WARN codex hooks: {e}"),
            }
        }
    }
    {
        let p = dirs::home_dir().unwrap().join(".grok/hooks/scopey.json");
        if p.exists() {
            println!("OK   grok hooks: {}", p.display());
        } else {
            println!("WARN grok hooks: no ~/.grok/hooks/scopey.json");
        }
    }
    {
        let p = dirs::home_dir()
            .unwrap()
            .join(".pi/agent/extensions/scopey.ts");
        if p.exists() {
            println!("OK   pi extension: {}", p.display());
        } else {
            println!("WARN pi extension: no ~/.pi/agent/extensions/scopey.ts");
        }
    }
    {
        let p = dirs::home_dir()
            .unwrap()
            .join(".config/opencode/plugins/scopey.js");
        if p.exists() {
            println!("OK   opencode plugin: {}", p.display());
        } else {
            println!("WARN opencode plugin: no ~/.config/opencode/plugins/scopey.js");
        }
    }

    check(
        "notifications",
        || {
            if cfg!(target_os = "macos") {
                match which::which("osascript") {
                    Ok(_) => Ok("osascript available".into()),
                    Err(_) => Err("osascript missing".into()),
                }
            } else if cfg!(target_os = "linux") {
                match which::which("notify-send") {
                    Ok(_) => Ok("notify-send available".into()),
                    Err(_) => Err("notify-send missing".into()),
                }
            } else {
                Ok("best-effort platform notify".into())
            }
        },
        &mut failed,
    );

    // Herdr is optional — warn only
    {
        use crate::herdr::HerdrContext;
        let h = HerdrContext::detect();
        if which::which("herdr").is_err() {
            println!("WARN herdr: not on PATH (optional; OS notifications still work)");
        } else {
            println!(
                "OK   herdr: {} (notify_backend={:?})",
                h.summary_line(),
                cfg.notify_backend
            );
            if !h.server_running && !h.inside_pane() {
                println!("WARN herdr: server not detected; scopey will fall back to OS notify");
            }
        }
    }

    // soft: version
    let _ = Command::new("scopey").arg("--version").output();

    println!(
        "\nconfig snapshot:\n  n_tool_calls={} m_reminder={} model_runner={} model={} claude_fast={} codex_fast={}",
        cfg.n_tool_calls,
        cfg.m_reminder,
        cfg.model_runner,
        cfg.model,
        cfg.claude_fast_model,
        cfg.codex_fast_model
    );

    if failed > 0 {
        println!("\ndoctor: {failed} check(s) failed");
        std::process::exit(1);
    }
    println!("\ndoctor: all critical checks passed");
    Ok(())
}

fn check(name: &str, f: impl FnOnce() -> std::result::Result<String, String>, failed: &mut i32) {
    match f() {
        Ok(msg) => println!("OK   {name}: {msg}"),
        Err(e) => {
            println!("FAIL {name}: {e}");
            *failed += 1;
        }
    }
}

// silence unused import if any
#[allow(dead_code)]
fn _touch_default_toml() {
    let _ = default_config_toml();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_scopey_hook_groups() {
        let g = json!({
            "hooks": [{
                "type": "command",
                "command": "\"/bin/scopey\" hook post-tool"
            }]
        });
        assert!(is_scopey_hook_group(&g));
        let other = json!({
            "hooks": [{ "type": "command", "command": "echo hi" }]
        });
        assert!(!is_scopey_hook_group(&other));
        assert!(!is_scopey_hook_group(&json!({})));
    }

    #[test]
    fn strip_removes_only_scopey() {
        let mut hooks = serde_json::Map::new();
        hooks.insert(
            "PostToolBatch".into(),
            json!([
                {"hooks":[{"type":"command","command":"scopey hook post-tool"}]},
                {"hooks":[{"type":"command","command":"other-tool"}]},
            ]),
        );
        hooks.insert(
            "UserPromptSubmit".into(),
            json!([{"hooks":[{"type":"command","command":"/x/scopey hook user-prompt"}]}]),
        );
        let n = strip_scopey_from_hooks_object(&mut hooks);
        assert_eq!(n, 2);
        let batch = hooks.get("PostToolBatch").unwrap().as_array().unwrap();
        assert_eq!(batch.len(), 1);
        assert!(!hooks.contains_key("UserPromptSubmit"));
    }
}

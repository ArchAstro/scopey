//! scopey — keep agent sessions on the original scope.
//!
//! Hooks call into this CLI at Claude Code / Codex lifecycle events. See
//! `scopey --help` and each subcommand's `--help` for model-oriented usage.

mod config;
mod eventlog;
mod guard;
mod herdr;
mod hooks;
mod insights;
mod model;
mod notify;
mod pathutil;
mod session;
mod tool_journal;
mod trajectory;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::config::Config;
use crate::session::SessionStore;

const LONG_ABOUT: &str = r#"scopey observes Claude Code and Codex sessions and keeps them on the
original user-defined scope.

HOW IT WORKS
  1. `scopey setup` installs lifecycle hooks into Claude and/or Codex.
  2. On each user prompt, hooks call `scopey hook user-prompt` which:
       - caches the prompt under ~/.scopey/work/<escaped-cwd>/<session>.json
       - runs a cheap model (claude -p / codex exec) to extract scope requirements
  3. Every N tool calls (config: n_tool_calls), hooks call `scopey hook post-tool` which:
       - journals a trajectory pointer
       - if a prior background judgement was off-track, emits injection JSON
         (scope requirements + judgement) for the hook to feed the model
       - every M tool calls, emits a lighter scope-reminder injection
       - starts a new background judgement over the last N tool-call window
         (course-correction therefore lags by ~2N tool calls)
  4. When a judgement is off_track, scopey also fires a desktop notification.

SESSION FILES
  Keyed by session_id (stable across cwd changes):
    ~/.scopey/work/by-id/<session_id>.json
  Legacy cwd-keyed files are migrated on open.

  Message types stored in the session: user_prompt, scope_requirements,
  trajectory_mark, judgement, injection, note.

HOOK CONTRACT
  Hook commands read the harness JSON event on stdin and may print JSON on
  stdout for the harness to inject as additionalContext. They must stay fast
  (background work is detached). Never print plain text unless you intend it
  as model-visible context.

CONFIG
  ~/.scopey/config.toml  (created by `scopey setup`)
  Project overlay: <cwd>/.scopey/config.toml  (optional, merges over user)

FOR MODELS DRIVING SCOPEY
  Prefer `scopey <cmd> --help` for the exact flags of a command. Use
  `scopey doctor` to verify install. Use `scopey status` to inspect a session.
  Do not reimplement path escaping — call `scopey path escape --cwd <dir>`.
"#;

#[derive(Parser, Debug)]
#[command(
    name = "scopey",
    version,
    about = "Keep Claude/Codex sessions on scope: observe, judge, inject, notify.",
    long_about = LONG_ABOUT,
    propagate_version = true,
    styles = clap::builder::Styles::styled()
)]
struct Cli {
    /// Override config file path (default: ~/.scopey/config.toml)
    #[arg(long, global = true, value_name = "PATH", env = "SCOPEY_CONFIG")]
    config: Option<PathBuf>,

    /// Print verbose diagnostics to stderr
    #[arg(long, short, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Install scopey hooks into agent harnesses (claude/codex/grok/pi/opencode)
    #[command(long_about = SETUP_ABOUT)]
    Setup {
        /// Install Claude Code hooks (~/.claude/settings.json)
        #[arg(long, default_value_t = true)]
        claude: bool,
        /// Skip Claude Code hooks
        #[arg(long, conflicts_with = "claude")]
        no_claude: bool,
        /// Install Codex hooks (~/.codex/hooks.json)
        #[arg(long, default_value_t = true)]
        codex: bool,
        /// Skip Codex hooks
        #[arg(long, conflicts_with = "codex")]
        no_codex: bool,
        /// Install Grok Build hooks (~/.grok/hooks/scopey.json)
        #[arg(long, default_value_t = true)]
        grok: bool,
        /// Skip Grok hooks
        #[arg(long, conflicts_with = "grok")]
        no_grok: bool,
        /// Install Pi extension (~/.pi/agent/extensions/scopey.ts)
        #[arg(long, default_value_t = true)]
        pi: bool,
        /// Skip Pi extension
        #[arg(long, conflicts_with = "pi")]
        no_pi: bool,
        /// Install OpenCode plugin (~/.config/opencode/plugins/scopey.js)
        #[arg(long, default_value_t = true)]
        opencode: bool,
        /// Skip OpenCode plugin
        #[arg(long, conflicts_with = "opencode")]
        no_opencode: bool,
        /// Overwrite existing scopey hook entries
        #[arg(long)]
        force: bool,
        /// Write config.toml defaults if missing
        #[arg(long, default_value_t = true)]
        write_config: bool,
    },

    /// Remove scopey from agent harnesses
    #[command(long_about = UNINSTALL_ABOUT)]
    Uninstall {
        /// Remove Claude Code hooks
        #[arg(long, default_value_t = true)]
        claude: bool,
        /// Skip Claude Code hooks
        #[arg(long, conflicts_with = "claude")]
        no_claude: bool,
        /// Remove Codex hooks
        #[arg(long, default_value_t = true)]
        codex: bool,
        /// Skip Codex hooks
        #[arg(long, conflicts_with = "codex")]
        no_codex: bool,
        /// Remove Grok hooks
        #[arg(long, default_value_t = true)]
        grok: bool,
        #[arg(long, conflicts_with = "grok")]
        no_grok: bool,
        /// Remove Pi extension
        #[arg(long, default_value_t = true)]
        pi: bool,
        #[arg(long, conflicts_with = "pi")]
        no_pi: bool,
        /// Remove OpenCode plugin
        #[arg(long, default_value_t = true)]
        opencode: bool,
        #[arg(long, conflicts_with = "opencode")]
        no_opencode: bool,
        /// Also delete ~/.scopey (config, sessions, logs, locks)
        #[arg(long)]
        purge_data: bool,
        /// Run process purge before removing hooks (default true)
        #[arg(long, default_value_t = true)]
        kill_jobs: bool,
        /// Skip process purge
        #[arg(long, conflicts_with = "kill_jobs")]
        no_kill_jobs: bool,
    },

    /// Show whether scopey is installed and runnable
    #[command(long_about = DOCTOR_ABOUT)]
    Doctor,

    /// Print effective configuration
    #[command(long_about = CONFIG_ABOUT)]
    Config {
        /// Write a default config file if none exists
        #[arg(long)]
        init: bool,
        /// Pretty-print as JSON instead of TOML path summary
        #[arg(long)]
        json: bool,
    },

    /// Inspect a session store file
    #[command(long_about = STATUS_ABOUT)]
    Status {
        /// Session id (from the harness hook payload)
        #[arg(long)]
        session_id: Option<String>,
        /// Working directory used to locate the session (default: cwd)
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Print full session JSON
        #[arg(long)]
        raw: bool,
    },

    /// List known sessions under the work directory
    #[command(long_about = SESSIONS_ABOUT)]
    Sessions {
        /// Only sessions for this cwd
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Limit
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },

    /// Analyze scope drift across stored sessions
    #[command(alias = "analytics", long_about = INSIGHTS_ABOUT)]
    Insights {
        /// Exact session id or unique id prefix
        #[arg(long = "session", alias = "session-id")]
        session: Option<String>,
        /// One local calendar day (YYYY-MM-DD)
        #[arg(long, conflicts_with_all = ["since", "until"])]
        date: Option<String>,
        /// Start time, inclusive (YYYY-MM-DD or RFC3339)
        #[arg(long)]
        since: Option<String>,
        /// End time, inclusive for dates (YYYY-MM-DD or RFC3339)
        #[arg(long)]
        until: Option<String>,
        /// Only sessions for this cwd
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Only sessions from this harness (codex, claude, ...)
        #[arg(long)]
        harness: Option<String>,
        /// Only sessions containing this verdict
        #[arg(long, value_name = "VERDICT")]
        verdict: Option<String>,
        /// Shortcut for warning or off-track verdicts
        #[arg(long, conflicts_with = "verdict")]
        off_scope: bool,
        /// Maximum sessions to print
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Include every matching judgement and its details
        #[arg(long)]
        details: bool,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Encode/decode Claude-style project path segments
    #[command(long_about = PATH_ABOUT)]
    Path {
        #[command(subcommand)]
        action: PathCmd,
    },

    /// Lifecycle handlers intended to be invoked by harness hooks (stdin JSON)
    #[command(long_about = HOOK_ABOUT)]
    Hook {
        #[command(subcommand)]
        event: HookCmd,
    },

    /// Run (or re-run) a scope judgement over a trajectory window
    #[command(long_about = JUDGE_ABOUT)]
    Judge {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        cwd: PathBuf,
        /// Inclusive start tool-call count for this window
        #[arg(long)]
        from_count: u64,
        /// Exclusive end tool-call count for this window
        #[arg(long)]
        to_count: u64,
        /// Transcript file path from the harness (optional; uses last known)
        #[arg(long)]
        transcript_path: Option<PathBuf>,
        /// Run in foreground (default for manual; hooks use background)
        #[arg(long, default_value_t = true)]
        foreground: bool,
    },

    /// Recompute scope requirements from stored user prompts
    #[command(long_about = SUMMARIZE_ABOUT)]
    Summarize {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        cwd: PathBuf,
        /// Optional extra prompt text to include
        #[arg(long)]
        prompt: Option<String>,
    },

    /// Fire a desktop notification (for testing / manual alerts)
    #[command(long_about = NOTIFY_ABOUT)]
    Notify {
        #[arg(long, default_value = "scopey")]
        title: String,
        #[arg(long)]
        body: String,
        /// macOS sound name (overrides config notify_sound for this call)
        #[arg(long)]
        sound: Option<String>,
    },

    /// Show how runner/model resolve per session harness; optionally probe CLIs
    #[command(long_about = MODELS_ABOUT)]
    Models {
        /// Send a one-word probe to each available runner/model pair
        #[arg(long)]
        verify: bool,
    },

    /// Herdr integration status and optional toast probe
    #[command(long_about = HERDR_ABOUT)]
    Herdr {
        /// Fire a test `herdr notification show`
        #[arg(long)]
        probe: bool,
    },

    /// Kill leaked scopey bg jobs / recursive claude storms (unix)
    #[command(long_about = PURGE_ABOUT)]
    Purge,

    /// Show per-session JSONL debug logs (or list recent sessions)
    #[command(long_about = LOGS_ABOUT)]
    Logs {
        /// Session id (omit to list recent session log files)
        #[arg(long)]
        session: Option<String>,
        /// Only last N lines
        #[arg(long, short = 'n')]
        tail: Option<usize>,
        /// Minimum level: debug|info|warn|error
        #[arg(long, default_value = "info")]
        level: String,
        /// Filter events containing this substring (e.g. hook, judge, guard)
        #[arg(long)]
        event: Option<String>,
        /// Follow the log file (like tail -f)
        #[arg(long, short = 'f')]
        follow: bool,
        /// Print raw JSONL lines
        #[arg(long)]
        raw: bool,
        /// Only print the log file path
        #[arg(long)]
        path: bool,
        /// How many sessions to list when --session is omitted
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
}

#[derive(Subcommand, Debug)]
enum PathCmd {
    /// Escape a cwd the way Claude encodes project directories
    Escape {
        #[arg(long)]
        cwd: PathBuf,
    },
    /// Show the session file path for a cwd + session_id
    SessionFile {
        #[arg(long)]
        cwd: PathBuf,
        #[arg(long)]
        session_id: String,
    },
}

#[derive(Subcommand, Debug)]
enum HookCmd {
    /// UserPromptSubmit: cache prompt, recompute scope (async summarize)
    #[command(long_about = HOOK_USER_PROMPT_ABOUT)]
    UserPrompt,
    /// SessionStart: ensure session file exists
    #[command(long_about = HOOK_SESSION_START_ABOUT)]
    SessionStart,
    /// PostToolUse / PostToolBatch: count tools, inject, schedule judge
    #[command(long_about = HOOK_POST_TOOL_ABOUT)]
    PostTool,
    /// Stop: optional end-of-turn injection of pending correction
    #[command(long_about = HOOK_STOP_ABOUT)]
    Stop,
}

const SETUP_ABOUT: &str = r#"Install scopey into local agent harnesses.

Supported harnesses (all on by default; disable with --no-<name>):
  claude    ~/.claude/settings.json          (PostToolBatch + prompt/session/stop)
  codex     ~/.codex/hooks.json
  grok      ~/.grok/hooks/scopey.json        (Claude-compatible lifecycle)
  pi        ~/.pi/agent/extensions/scopey.ts (Pi extension adapter)
  opencode  ~/.config/opencode/plugins/scopey.js

Also writes ~/.scopey/config.toml when missing.

Examples:
  scopey setup
  scopey setup --no-codex --no-pi          # claude + grok + opencode
  scopey setup --no-claude --no-codex --grok --no-pi --no-opencode
  scopey setup --force
"#;

const UNINSTALL_ABOUT: &str = r#"Remove scopey from local agent harnesses.

By default removes scopey from claude, codex, grok, pi, and opencode, signals
leaked bg jobs, and keeps ~/.scopey data.

Examples:
  scopey uninstall
  scopey uninstall --no-claude --grok      # only touch grok (+ other defaults)
  scopey uninstall --purge-data
  scopey uninstall --no-kill-jobs
"#;

const DOCTOR_ABOUT: &str = r#"Verify binary on PATH, config present, model runners available, and hooks registered.

Exit codes:
  0  all critical checks passed
  1  one or more checks failed

Models: run this after setup or when injection seems silent.
"#;

const CONFIG_ABOUT: &str = r#"Show the effective config (user + optional project overlay).

Keys that matter for lifecycle:
  n_tool_calls       every N tool events → journal + start background judge
  m_reminder         every M tool events → inject scope-requirements reminder
  model_runner         auto|claude|codex  (auto = session harness)
  model                auto|<slug>        (auto = shipped fast tier)
  claude_fast_model    default "haiku"
  codex_fast_model     default "gpt-5.6-terra"
  notify_on_off_track  desktop alert when judgement is off_track
  work_root            where session JSON lives (default ~/.scopey/work)

  See also: scopey models [--verify]

Example:
  scopey config
  scopey config --init
  scopey config --json
"#;

const STATUS_ABOUT: &str = r#"Summarize a session store: scope requirements, tool count, last judgement.

If --session_id is omitted, status lists how to find sessions or uses SCOPEY_SESSION_ID.

Example:
  scopey status --session-id abc123
  scopey status --session-id abc123 --cwd /path/to/project --raw
"#;

const SESSIONS_ABOUT: &str = r#"List recent session files under the work root.

Example:
  scopey sessions
  scopey sessions --cwd . --limit 20
"#;

const INSIGHTS_ABOUT: &str = r#"Analyze judgement history across stored sessions.

The summary highlights sessions with off-track or warning windows, reports the
share of evaluated windows that needed attention, and includes the judge's
summary plus scope context. Date-only values use the machine's local timezone.

Verdicts: on-track, warning, off-track, insufficient-evidence, unknown.

Examples:
  scopey insights
  scopey insights --off-scope --since 2026-07-01
  scopey insights --session 019fb598 --details
  scopey insights --date 2026-07-30 --harness codex
  scopey insights --cwd . --verdict warning --json
"#;

const PATH_ABOUT: &str = r#"Path helpers matching Claude's project-directory encoding.

Claude stores projects as the absolute cwd with '/' replaced by '-'.
scopey uses the same encoding under ~/.scopey/work/<escaped>/<session_id>.json.

Example:
  scopey path escape --cwd /Users/you/proj
  scopey path session-file --cwd . --session-id sid
"#;

const HOOK_ABOUT: &str = r#"Hook entrypoints. Harnesses must call these with the event JSON on stdin.

Stdout is reserved for harness-facing injection JSON. Diagnostics go to stderr.
Do not call these interactively unless you are replaying a fixture.

Subcommands map to lifecycle events:
  user-prompt    UserPromptSubmit
  session-start  SessionStart
  post-tool      PostToolUse and/or PostToolBatch
  stop           Stop
"#;

const HOOK_USER_PROMPT_ABOUT: &str = r#"Read harness UserPromptSubmit JSON from stdin.

Actions:
  1. Open/create session file for (cwd, session_id)
  2. Append message type=user_prompt
  3. Spawn background `scopey summarize` to refresh scope_requirements
  4. Exit 0 with empty stdout (logging must not inject by accident)

Required stdin fields: session_id, cwd (or use process cwd), prompt
Optional: transcript_path, hook_event_name
"#;

const HOOK_SESSION_START_ABOUT: &str = r#"Read harness SessionStart JSON from stdin.

Ensures the session file exists and records harness/source. No model call.
"#;

const HOOK_POST_TOOL_ABOUT: &str = r#"Read harness PostToolUse or PostToolBatch JSON from stdin.

Actions:
  1. Increment tool_call_count (batch events count each tool in the batch)
  2. If a ready off_track/warning judgement exists → print injection JSON
     containing scope requirements + judgement (then mark judgement injected)
  3. Else if tool_call_count % m_reminder == 0 → print scope reminder injection
  4. If tool_call_count % n_tool_calls == 0 → journal trajectory_mark and
     spawn background `scopey judge` for the last N window
  5. Exit 0

Injection JSON (Claude-compatible):
  {"hookSpecificOutput":{"hookEventName":"<event>","additionalContext":"..."}}

Codex also accepts the same additionalContext shape.
"#;

const HOOK_STOP_ABOUT: &str = r#"Read harness Stop JSON from stdin.

If there is a ready non-injected off_track judgement, emit injection JSON so
the model sees the correction before the user continues. Does not force-continue
the agent loop by itself (harness Stop decision:block is left to the user/hook
wrapper if desired).
"#;

const JUDGE_ABOUT: &str = r#"Judge recent trajectory against scope requirements using the cheap model.

Normally spawned in the background by `scopey hook post-tool`. Writes a
judgement message into the session file. On off_track, triggers notify.

Example:
  scopey judge --session-id sid --cwd . --from-count 0 --to-count 10 \
    --transcript-path ~/.claude/projects/.../sid.jsonl
"#;

const SUMMARIZE_ABOUT: &str = r#"Build/replace scope_requirements from all user_prompt messages in the session.

Uses model_runner + model from config. Intended for background use after prompts.

Example:
  scopey summarize --session-id sid --cwd .
"#;

const NOTIFY_ABOUT: &str = r#"Send a notification the same way off-track alerts do.

Backend (config notify_backend, default auto):
  herdr  → herdr notification show when Herdr is available (inside a Herdr
           pane or server running). In-app toast when ui.toast.delivery=herdr.
  os     → macOS osascript / Linux notify-send (always desktop)
  auto   → Herdr when available, else OS
  command → run notify_command template

Uses config notify_sound / herdr_notify_sound unless --sound is passed.
For full customization, set notify_* keys in ~/.scopey/config.toml.

Examples:
  scopey notify --title scopey --body "Session may be off-track"
  scopey notify --title scopey --body "test" --sound request
"#;

const LOGS_ABOUT: &str = r#"Read structured per-session debug logs written by scopey hooks and jobs.

Log path:
  ~/.scopey/logs/<session_id>.jsonl

Each line is JSON with: ts, level, session_id, event, message, pid, internal, fields.

Examples:
  scopey logs
  scopey logs --session abc123
  scopey logs --session abc123 --tail 50
  scopey logs --session abc123 --level warn
  scopey logs --session abc123 --event guard
  scopey logs --session abc123 --follow
  scopey logs --session abc123 --path
  scopey logs --session abc123 --raw
"#;

const PURGE_ABOUT: &str = r#"SIGTERM leaked background workers that can storm the machine:

  - `scopey summarize` / `scopey judge` processes
  - `claude -p` processes whose prompt is a scopey internal analyst/judge text
  - stale per-session job lock files

Safe to run any time hooks feel stuck or CPU spikes from recursive headless Claude.

Example:
  scopey purge
"#;

const HERDR_ABOUT: &str = r#"Inspect Herdr awareness and optionally probe its notification API.

Herdr (https://herdr.dev) is an agent multiplexer. When agents run inside a
Herdr pane they export HERDR_ENV / HERDR_SOCKET_PATH / HERDR_PANE_ID.

scopey uses:
  herdr notification show <title> --body … --sound request|done|none
  herdr pane report-agent <pane> --source scopey --state blocked …

Toast delivery is controlled by Herdr config `[ui.toast] delivery`:
  off | herdr | terminal | system

scopey config:
  notify_backend = "auto"   # auto|herdr|os|command
  herdr_report_state = true
  notify_fallback_os_if_herdr_disabled = true

Examples:
  scopey herdr
  scopey herdr --probe
"#;

const MODELS_ABOUT: &str = r#"Explain and verify lightweight model selection for summarize/judge.

Resolution rules (config defaults in parentheses):
  model_runner = auto|claude|codex   (auto)
  model        = auto|<slug>         (auto)

When model_runner=auto, the session harness wins:
  Claude Code session → claude CLI
  Codex session       → codex CLI

When model=auto, use the product's shipped fast tier:
  Claude → claude_fast_model (default "haiku" alias)
  Codex  → codex_fast_model  (default "gpt-5.6-terra" mini-like tier)

Examples:
  scopey models
  scopey models --verify
  make verify-models
"#;

fn main() {
    if let Err(e) = run() {
        eprintln!("scopey error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref())?;
    if cli.verbose {
        eprintln!("scopey: config loaded from {}", cfg.loaded_from.display());
        if guard::is_internal() {
            eprintln!("scopey: SCOPEY_INTERNAL=1 (hook side-effects disabled)");
        }
    }

    match cli.command {
        Commands::Setup {
            claude,
            no_claude,
            codex,
            no_codex,
            grok,
            no_grok,
            pi,
            no_pi,
            opencode,
            no_opencode,
            force,
            write_config,
        } => {
            let set = hooks::setup::HarnessSet {
                claude: claude && !no_claude,
                codex: codex && !no_codex,
                grok: grok && !no_grok,
                pi: pi && !no_pi,
                opencode: opencode && !no_opencode,
            };
            hooks::setup::run_setup(&cfg, set, force, write_config)
        }
        Commands::Uninstall {
            claude,
            no_claude,
            codex,
            no_codex,
            grok,
            no_grok,
            pi,
            no_pi,
            opencode,
            no_opencode,
            purge_data,
            kill_jobs,
            no_kill_jobs,
        } => {
            let set = hooks::setup::HarnessSet {
                claude: claude && !no_claude,
                codex: codex && !no_codex,
                grok: grok && !no_grok,
                pi: pi && !no_pi,
                opencode: opencode && !no_opencode,
            };
            let do_kill = kill_jobs && !no_kill_jobs;
            hooks::setup::run_uninstall(&cfg, set, purge_data, do_kill)
        }
        Commands::Doctor => hooks::setup::run_doctor(&cfg),
        Commands::Config { init, json } => {
            if init {
                Config::write_default_if_missing()?;
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&cfg)?);
            } else {
                println!("{}", cfg.display_human());
            }
            Ok(())
        }
        Commands::Status {
            session_id,
            cwd,
            raw,
        } => cmd_status(&cfg, session_id, cwd, raw),
        Commands::Sessions { cwd, limit } => cmd_sessions(&cfg, cwd, limit),
        Commands::Insights {
            session,
            date,
            since,
            until,
            cwd,
            harness,
            verdict,
            off_scope,
            limit,
            details,
            json,
        } => insights::run(
            &cfg,
            insights::InsightArgs {
                session,
                date,
                since,
                until,
                cwd,
                harness,
                verdict,
                off_scope,
                limit,
                details,
                json,
            },
        ),
        Commands::Path { action } => match action {
            PathCmd::Escape { cwd } => {
                let abs = pathutil::abs_cwd(&cwd)?;
                println!("{}", pathutil::escape_project_path(&abs));
                Ok(())
            }
            PathCmd::SessionFile { cwd, session_id } => {
                let path = SessionStore::session_path(&cfg, &cwd, &session_id)?;
                println!("{}", path.display());
                Ok(())
            }
        },
        Commands::Hook { event } => {
            // Hooks must never hang or fail the harness (exit 1 / invalid JSON).
            // Log errors to stderr + session log; always return success with
            // either empty stdout or a single valid inject JSON object.
            let res = match event {
                HookCmd::UserPrompt => hooks::handlers::user_prompt(&cfg),
                HookCmd::SessionStart => hooks::handlers::session_start(&cfg),
                HookCmd::PostTool => hooks::handlers::post_tool(&cfg),
                HookCmd::Stop => hooks::handlers::stop(&cfg),
            };
            if let Err(e) = res {
                eprintln!("scopey hook error (suppressed for harness): {e:#}");
            }
            Ok(())
        }
        Commands::Judge {
            session_id,
            cwd,
            from_count,
            to_count,
            transcript_path,
            foreground: _,
        } => {
            // Background workers must hold the per-session lock for their lifetime.
            let _guard = match guard::SessionJobGuard::try_acquire(&cfg, &session_id, "judge")? {
                Some(g) => g,
                None => {
                    eprintln!("scopey judge: skipped (session busy or throttled)");
                    return Ok(());
                }
            };
            let res = trajectory::judge_window(
                &cfg,
                &session_id,
                &cwd,
                from_count,
                to_count,
                transcript_path.as_deref(),
            );
            // guard drops here
            res
        }
        Commands::Summarize {
            session_id,
            cwd,
            prompt,
        } => {
            let _guard = match guard::SessionJobGuard::try_acquire(&cfg, &session_id, "summarize")?
            {
                Some(g) => g,
                None => {
                    eprintln!("scopey summarize: skipped (session busy or throttled)");
                    return Ok(());
                }
            };
            trajectory::summarize_scope(&cfg, &session_id, &cwd, prompt.as_deref())
        }
        Commands::Notify { title, body, sound } => {
            let sound = sound.as_deref().filter(|s| !s.is_empty());
            notify::notify(&cfg, &title, &body, sound)
        }
        Commands::Models { verify } => model::report_models(&cfg, verify),
        Commands::Herdr { probe } => cmd_herdr(&cfg, probe),
        Commands::Purge => {
            let n = guard::purge_leaked_jobs()?;
            println!("scopey purge: signaled {n} leaked process(es); stale locks cleaned");
            Ok(())
        }
        Commands::Logs {
            session,
            tail,
            level,
            event,
            follow,
            raw,
            path,
            limit,
        } => match session {
            None => eventlog::cmd_list_logs(limit),
            Some(session_id) => {
                let min_level = eventlog::Level::parse(&level).unwrap_or(eventlog::Level::Info);
                eventlog::cmd_logs(eventlog::LogsQuery {
                    session_id,
                    tail,
                    min_level,
                    event_prefix: event,
                    follow,
                    raw,
                    path_only: path,
                })
            }
        },
    }
}

fn cmd_herdr(cfg: &Config, probe: bool) -> Result<()> {
    use crate::herdr::{self, HerdrContext};
    let h = HerdrContext::detect();
    println!("herdr detection:");
    println!("  {}", h.summary_line());
    println!("  notify_backend = {:?}", cfg.notify_backend);
    println!("  herdr_report_state = {}", cfg.herdr_report_state);
    println!(
        "  herdr_notify_sound = {:?}",
        cfg.herdr_notify_sound
    );
    println!(
        "  notify_fallback_os_if_herdr_disabled = {}",
        cfg.notify_fallback_os_if_herdr_disabled
    );
    println!();
    println!(
        "Herdr toast delivery is configured in ~/.config/herdr/config.toml under [ui.toast]."
    );
    println!("  delivery = \"herdr\" | \"terminal\" | \"system\" | \"off\"");
    println!();

    if !probe {
        println!("pass --probe to call: herdr notification show …");
        return Ok(());
    }

    let title = "scopey herdr probe";
    let body = "If you see this, Herdr notification routing works.";
    let sound = herdr::herdr_sound_for("off_track", cfg.herdr_notify_sound.as_deref());
    match herdr::notification_show(title, body, sound, cfg.herdr_notify_position.as_deref()) {
        Ok(true) => println!("probe: shown=true (sound={sound})"),
        Ok(false) => {
            println!("probe: shown=false (Herdr accepted but delivery disabled/muted)");
            if cfg.notify_fallback_os_if_herdr_disabled {
                println!("falling back to OS notify…");
                notify::notify(cfg, title, body, cfg.notify_sound.as_deref())?;
            }
        }
        Err(e) => {
            eprintln!("probe failed: {e:#}");
            return Err(e);
        }
    }
    Ok(())
}

fn cmd_status(
    cfg: &Config,
    session_id: Option<String>,
    cwd: Option<PathBuf>,
    raw: bool,
) -> Result<()> {
    let cwd = cwd.unwrap_or(std::env::current_dir()?);
    let session_id = session_id
        .or_else(|| std::env::var("SCOPEY_SESSION_ID").ok())
        .context("pass --session-id or set SCOPEY_SESSION_ID")?;
    let store = SessionStore::open(cfg, &cwd, &session_id)?;
    if raw {
        println!("{}", serde_json::to_string_pretty(&store.data)?);
        return Ok(());
    }
    println!("{}", store.summary());
    Ok(())
}

fn cmd_sessions(cfg: &Config, cwd: Option<PathBuf>, limit: usize) -> Result<()> {
    let list = SessionStore::list(cfg, cwd.as_deref(), limit)?;
    if list.is_empty() {
        println!("(no sessions under {})", cfg.work_root.display());
        return Ok(());
    }
    for e in list {
        println!(
            "{}\t{}\ttools={}\t{}",
            e.session_id,
            e.updated_at,
            e.tool_call_count,
            e.path.display()
        );
    }
    Ok(())
}

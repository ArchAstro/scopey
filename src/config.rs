use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Every N tool calls: journal trajectory + start background judge.
    pub n_tool_calls: u64,
    /// Every M tool calls: inject a scope-requirements reminder (if no stronger injection).
    pub m_reminder: u64,
    /// Which CLI runs summarize/judge: `"auto"`, `"claude"`, `"codex"`,
    /// `"grok"`, `"pi"`, or `"opencode"`.
    ///
    /// `auto` uses the session's harness (same product the agent is running in).
    pub model_runner: String,
    /// Model slug, or `"auto"` to use the runner's shipped fast default.
    pub model: String,
    /// Claude Code fast-tier alias/id when `model = "auto"` (shipped: `haiku`).
    pub claude_fast_model: String,
    /// Codex fast/mini-like tier when `model = "auto"` (shipped: `gpt-5.6-terra`).
    pub codex_fast_model: String,
    /// Grok Build fast model when `model = "auto"`.
    pub grok_fast_model: String,
    /// Pi print-mode model (provider/id or alias) when `model = "auto"`.
    pub pi_fast_model: String,
    /// OpenCode `run` model when `model = "auto"` (empty = OpenCode default).
    pub opencode_fast_model: String,
    /// Optional full command template. Placeholders: {model} {prompt_file} {runner}
    /// If empty, the selected runner's built-in invocation is used.
    pub model_command: Option<String>,
    /// Desktop notify when a judgement is off_track.
    pub notify_on_off_track: bool,
    /// Also notify on warning judgements.
    pub notify_on_warning: bool,
    /// Notify when background summarize/judge model calls fail persistently
    /// (scope tracking silently degrades to echoing the latest prompt).
    pub notify_on_model_fallback: bool,
    /// Title template for off_track alerts.
    /// Placeholders: {verdict} {summary} {details} {session_id} {cwd} {from_count} {to_count} {harness}
    pub notify_title_off_track: String,
    /// Title template for warning alerts (same placeholders).
    pub notify_title_warning: String,
    /// Body template (same placeholders).
    pub notify_body: String,
    /// macOS `sound name` for osascript (e.g. "default", "Sosumi", "Glass"). Empty = silent.
    pub notify_sound: Option<String>,
    /// Optional shell command instead of the built-in notifier.
    /// Placeholders: {title} {body} {verdict} {summary} {details} {session_id} {cwd}
    /// {from_count} {to_count} {harness} {sound}
    /// Example: `terminal-notifier -title "{title}" -message "{body}" -sound default`
    pub notify_command: Option<String>,
    /// Where to deliver alerts: `auto` | `herdr` | `os` | `command`.
    ///
    /// `auto` prefers Herdr when available (HERDR_* env / running server), else OS.
    pub notify_backend: String,
    /// Herdr toast position: top-left|top-right|bottom-left|bottom-right.
    pub herdr_notify_position: Option<String>,
    /// Herdr sound: `none` | `done` | `request` (needs-attention).
    /// When unset, off_track/warning → request.
    pub herdr_notify_sound: Option<String>,
    /// If Herdr accepts the call but `shown=false` (toasts disabled), fall back to OS.
    pub notify_fallback_os_if_herdr_disabled: bool,
    /// When true and inside a Herdr pane, also `pane report-agent --state blocked`.
    pub herdr_report_state: bool,
    /// `--source` for herdr pane report-agent.
    pub herdr_source: String,
    /// `--agent` label for herdr pane report-agent.
    pub herdr_agent_label: String,
    /// Root for session JSON files.
    pub work_root: PathBuf,
    /// Max characters of transcript excerpt sent to the judge model.
    pub judge_transcript_chars: usize,
    /// Max characters of stored user prompts sent to summarize.
    pub summarize_prompt_chars: usize,
    /// Timeout seconds for model runner subprocesses.
    pub model_timeout_secs: u64,
    /// Preferred harness when session harness is unknown and model_runner=auto.
    pub default_harness: String,
    /// Minimum seconds between background jobs (summarize/judge) for one session.
    /// Prevents process storms when hooks fire rapidly.
    pub min_job_interval_secs: u64,
    /// Max concurrent background jobs globally (0 = unlimited; default 2).
    pub max_global_jobs: u64,
    /// Max chars of each tool's args stored in the journal / sent to the judge.
    pub tool_args_preview_chars: usize,
    /// Max tools lag after a judgement window before Ready injects are discarded as stale.
    /// Default: 2 * n_tool_calls (course-correction lag budget).
    pub judgement_max_lag_tools: u64,
    /// When true, course-correction injections tell the model to render ASCII Scopey
    /// saying "You got scoped!" before continuing. Set false to disable the gag.
    pub ascii_scopey_on_correction: bool,
    /// When true (default), hook events that originate inside a subagent /
    /// child-agent session (Claude Code Task agents send `agent_id`; OpenCode
    /// child sessions have a parent id; adapters may tag `subagent: true`)
    /// are ignored entirely: no tool counting, no reminders, no corrections.
    /// Scope belongs to the conversation between the user and the top-level
    /// agent; a subagent's prompt is the orchestrator's, not the user's.
    pub ignore_subagents: bool,
    /// Log each hook's raw stdin payload (clipped) to the session log at
    /// debug level. Diagnostics only — payloads include prompts, so this
    /// inherits the same privacy caveats as the rest of `~/.scopey`.
    pub log_raw_events: bool,

    /// Filled at load time — not serialized as user config preference.
    #[serde(skip)]
    pub loaded_from: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            n_tool_calls: 15,
            m_reminder: 30,
            model_runner: "auto".into(),
            model: "auto".into(),
            claude_fast_model: "haiku".into(),
            codex_fast_model: "gpt-5.6-terra".into(),
            grok_fast_model: "grok-3-mini".into(),
            pi_fast_model: String::new(),
            opencode_fast_model: String::new(),
            model_command: None,
            notify_on_off_track: true,
            notify_on_warning: false,
            notify_on_model_fallback: true,
            notify_title_off_track: "scopey: off-track".into(),
            notify_title_warning: "scopey: scope warning".into(),
            notify_body: "{summary} (session {session_id})".into(),
            notify_sound: Some("default".into()),
            notify_command: None,
            notify_backend: "auto".into(),
            herdr_notify_position: None,
            herdr_notify_sound: None,
            notify_fallback_os_if_herdr_disabled: true,
            herdr_report_state: true,
            herdr_source: "scopey".into(),
            herdr_agent_label: "scopey".into(),
            work_root: default_work_root(),
            judge_transcript_chars: 24_000,
            summarize_prompt_chars: 32_000,
            model_timeout_secs: 90,
            default_harness: "claude".into(),
            min_job_interval_secs: 60,
            max_global_jobs: 2,
            tool_args_preview_chars: 400,
            judgement_max_lag_tools: 0, // 0 → 2 * n_tool_calls at load
            ascii_scopey_on_correction: true,
            ignore_subagents: true,
            log_raw_events: false,
            loaded_from: PathBuf::new(),
        }
    }
}

impl Config {
    pub fn user_config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".scopey")
            .join("config.toml")
    }

    /// Scopey state root. Override with `SCOPEY_HOME` (used in tests).
    pub fn scopey_home() -> PathBuf {
        if let Ok(p) = std::env::var("SCOPEY_HOME") {
            let pb = PathBuf::from(p);
            if !pb.as_os_str().is_empty() {
                return pb;
            }
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".scopey")
    }

    /// Run `f` with `SCOPEY_HOME` pointing at a temp dir. Serializes env mutations
    /// across the whole crate so parallel tests don't clobber each other.
    #[cfg(test)]
    pub fn with_temp_scopey_home<R>(f: impl FnOnce(&std::path::Path) -> R) -> R {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = LOCK.get_or_init(|| Mutex::new(()));
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("SCOPEY_HOME");
        std::env::set_var("SCOPEY_HOME", dir.path());
        let out = f(dir.path());
        match prev {
            Some(v) => std::env::set_var("SCOPEY_HOME", v),
            None => std::env::remove_var("SCOPEY_HOME"),
        }
        out
    }

    pub fn load(override_path: Option<&Path>) -> Result<Self> {
        let path = override_path
            .map(Path::to_path_buf)
            .unwrap_or_else(Self::user_config_path);

        let mut cfg = if path.exists() {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("read config {}", path.display()))?;
            let mut c: Config = toml::from_str(&text)
                .with_context(|| format!("parse config {}", path.display()))?;
            c.loaded_from = path.clone();
            c
        } else {
            Config {
                loaded_from: path.clone(),
                ..Config::default()
            }
        };

        // Project overlay: <cwd>/.scopey/config.toml
        if let Ok(cwd) = std::env::current_dir() {
            let proj = cwd.join(".scopey").join("config.toml");
            if proj.exists() && override_path.is_none() {
                let text = fs::read_to_string(&proj)?;
                let overlay: Config = toml::from_str(&text)?;
                cfg.merge_overlay(overlay);
                cfg.loaded_from = proj;
            }
        }

        if cfg.n_tool_calls == 0 {
            cfg.n_tool_calls = 10;
        }
        if cfg.m_reminder == 0 {
            cfg.m_reminder = cfg.n_tool_calls * 2;
        }
        if cfg.tool_args_preview_chars == 0 {
            cfg.tool_args_preview_chars = 400;
        }
        if cfg.judgement_max_lag_tools == 0 {
            cfg.judgement_max_lag_tools = cfg.n_tool_calls.saturating_mul(2).max(30);
        }
        Ok(cfg)
    }

    fn merge_overlay(&mut self, o: Config) {
        // Overlay replaces scalar fields from project file wholesale when present via Default — we
        // re-parse so project file is full replacement of known keys only if user set them.
        // Simple approach: project file wins entirely for set values by taking all fields.
        *self = Config {
            loaded_from: self.loaded_from.clone(),
            ..o
        };
    }

    pub fn write_default_if_missing() -> Result<PathBuf> {
        let path = Self::user_config_path();
        if path.exists() {
            return Ok(path);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, default_config_toml())?;
        let work = default_work_root();
        fs::create_dir_all(&work)?;
        Ok(path)
    }

    pub fn display_human(&self) -> String {
        format!(
            "loaded_from = {}\n\
             n_tool_calls = {}          # journal + background judge every N tools\n\
             m_reminder = {}            # inject scope reminder every M tools\n\
             model_runner = {:?}     # auto → use any supported session harness\n\
             model = {:?}            # auto → runner's shipped fast default\n\
             claude_fast_model = {:?}\n\
             codex_fast_model = {:?}\n\
             grok_fast_model = {:?}\n\
             pi_fast_model = {:?}\n\
             opencode_fast_model = {:?}\n\
             model_command = {:?}\n\
             notify_on_off_track = {}\n\
             notify_on_warning = {}\n\
             notify_on_model_fallback = {} # alert when bg model calls keep failing\n\
             notify_title_off_track = {:?}\n\
             notify_title_warning = {:?}\n\
             notify_body = {:?}\n\
             notify_sound = {:?}\n\
             notify_command = {:?}\n\
             notify_backend = {:?}   # auto|herdr|os|command\n\
             herdr_notify_position = {:?}\n\
             herdr_notify_sound = {:?}\n\
             notify_fallback_os_if_herdr_disabled = {}\n\
             herdr_report_state = {}\n\
             herdr_source = {:?}\n\
             herdr_agent_label = {:?}\n\
             work_root = {}\n\
             judge_transcript_chars = {}\n\
             summarize_prompt_chars = {}\n\
             model_timeout_secs = {}\n\
             default_harness = {:?}\n\
             min_job_interval_secs = {}   # throttle bg jobs per session\n\
             max_global_jobs = {}         # cap concurrent scopey bg workers\n\
             tool_args_preview_chars = {} # journal / judge arg clip\n\
             judgement_max_lag_tools = {} # stale inject discard after this lag\n\
             ascii_scopey_on_correction = {} # ASCII Scopey gag on course-correction\n\
             ignore_subagents = {}        # no-op for subagent/child-agent hook events\n\
             log_raw_events = {}          # debug-log raw hook stdin payloads\n\
             \n\
             Course-correction lag ≈ 2 * n_tool_calls tool events:\n\
               window k judged in background → injection applied at window k+1 boundary.\n\
             Judge reads structured tool journal (not raw JSONL tails).\n\
             Throttled summarize/judge jobs are deferred and drained when free.\n\
             \n\
             Model selection: same product as the agent session when model_runner=auto.\n\
             Fast defaults are runner-specific; empty Pi/OpenCode values use provider defaults.\n",
            self.loaded_from.display(),
            self.n_tool_calls,
            self.m_reminder,
            self.model_runner,
            self.model,
            self.claude_fast_model,
            self.codex_fast_model,
            self.grok_fast_model,
            self.pi_fast_model,
            self.opencode_fast_model,
            self.model_command,
            self.notify_on_off_track,
            self.notify_on_warning,
            self.notify_on_model_fallback,
            self.notify_title_off_track,
            self.notify_title_warning,
            self.notify_body,
            self.notify_sound,
            self.notify_command,
            self.notify_backend,
            self.herdr_notify_position,
            self.herdr_notify_sound,
            self.notify_fallback_os_if_herdr_disabled,
            self.herdr_report_state,
            self.herdr_source,
            self.herdr_agent_label,
            self.work_root.display(),
            self.judge_transcript_chars,
            self.summarize_prompt_chars,
            self.model_timeout_secs,
            self.default_harness,
            self.min_job_interval_secs,
            self.max_global_jobs,
            self.tool_args_preview_chars,
            self.judgement_max_lag_tools,
            self.ascii_scopey_on_correction,
            self.ignore_subagents,
            self.log_raw_events,
        )
    }
}

fn default_work_root() -> PathBuf {
    Config::scopey_home().join("work")
}

pub fn default_config_toml() -> String {
    format!(
        r#"# scopey configuration
# Docs: scopey --help | scopey config --help

# Every N tool-call events: write a trajectory mark and start a background
# judgement of that window against scope requirements.
n_tool_calls = 15

# Every M tool-call events: inject a short reminder of scope requirements
# (skipped if a stronger correction injection is emitted in the same event).
m_reminder = 30

# Runner for summarize + judge jobs.
#   auto    → use the session harness (same product the agent is running in)
#   claude  → always `claude -p --model …`
#   codex   → always `codex exec -m …`
#   grok    → always `grok -p --model …`
#   pi      → always `pi --print …`
#   opencode → always `opencode run …`
model_runner = "auto"

# Model id/alias, or "auto" to use the runner's shipped fast default:
#   Claude Code → claude_fast_model (alias "haiku" tracks current fast Haiku)
#   Codex       → codex_fast_model  (mini-like tier, currently gpt-5.6-terra)
model = "auto"
claude_fast_model = "haiku"
codex_fast_model = "gpt-5.6-terra"
grok_fast_model = "grok-3-mini"
# empty = use harness default
pi_fast_model = ""
opencode_fast_model = ""

# Optional full command. Placeholders: {{model}} {{prompt_file}} {{runner}}
# Runs through `sh -c`; shell operators and substitutions are supported.
# Scopey disables nested hooks and removes CLAUDE_CODE_SIMPLE so OAuth remains available.
# model_command = "claude -p --model {{model}} \"$(cat {{prompt_file}})\""

notify_on_off_track = true
notify_on_warning = false
# Alert when background summarize/judge model calls fail repeatedly and scope
# tracking degrades to echoing the latest prompt. Details: scopey doctor.
notify_on_model_fallback = true

# Notification copy (templates). Placeholders:
#   {{verdict}} {{summary}} {{details}} {{session_id}} {{session}} {{cwd}}
#   {{from_count}} {{to_count}} {{harness}}
notify_title_off_track = "scopey: off-track"
notify_title_warning = "scopey: scope warning"
notify_body = "{{summary}} (session {{session_id}})"

# macOS sound name for osascript (default, Sosumi, Glass, …). Empty / omit = silent.
notify_sound = "default"

# Optional: replace the built-in notifier with any shell command.
# Extra placeholders: {{title}} {{body}} {{sound}}
# notify_command = "terminal-notifier -title \"{{title}}\" -message \"{{body}}\" -sound default"

# Delivery backend: auto | herdr | os | command
# auto → Herdr when HERDR_* / server available, else OS desktop
notify_backend = "auto"
# herdr_notify_position = "bottom-right"
# herdr_notify_sound = "request"   # none|done|request
notify_fallback_os_if_herdr_disabled = true

# When inside a Herdr pane, also report blocked state for sidebar / waits
herdr_report_state = true
herdr_source = "scopey"
herdr_agent_label = "scopey"

work_root = "{work}"

judge_transcript_chars = 24000
summarize_prompt_chars = 32000
model_timeout_secs = 90
default_harness = "claude"

# Process-storm guards (deterministic; hooks never leave this to the model)
# At most one bg summarize/judge per session; refuse if a live worker holds the lock.
min_job_interval_secs = 60
# Soft cap on concurrent scopey bg jobs machine-wide (0 = off)
max_global_jobs = 2

# Structured tool journal (judge uses this, not raw JSONL tails)
tool_args_preview_chars = 400
# Discard Ready warning/off_track injects once session advanced this many tools past window end.
# 0 = auto (2 * n_tool_calls)
judgement_max_lag_tools = 0

# On course-correction, instruct the model to render ASCII Scopey saying "You got scoped!"
# Set false to disable the gag.
ascii_scopey_on_correction = true

# Ignore hook events fired inside subagent / child-agent sessions (Claude Code
# Task agents, OpenCode child sessions, adapter-tagged subagents). Scope belongs
# to the top-level conversation; subagent prompts come from the orchestrator,
# and injecting reminders into them derails delegated work.
ignore_subagents = true
# Debug aid: log each hook's raw stdin payload (clipped) to the session log.
log_raw_events = false
"#,
        work = default_work_root().display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_sane() {
        let c = Config::default();
        assert!(c.n_tool_calls >= 1);
        assert!(c.m_reminder >= c.n_tool_calls);
        assert_eq!(c.model_runner, "auto");
        assert_eq!(c.model, "auto");
        assert_eq!(c.claude_fast_model, "haiku");
        assert!(!c.codex_fast_model.is_empty());
        assert_eq!(c.notify_backend, "auto");
    }

    #[test]
    fn parse_toml_defaults() {
        let t = default_config_toml();
        let c: Config = toml::from_str(&t).expect("default toml parses");
        assert!(c.n_tool_calls > 0);
        assert_eq!(c.min_job_interval_secs, 60);
    }

    #[test]
    fn load_missing_file_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        let c = Config::load(Some(&missing)).unwrap();
        assert_eq!(c.loaded_from, missing);
        assert_eq!(c.model_runner, "auto");
    }

    #[test]
    fn load_partial_toml() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.toml");
        fs::write(
            &p,
            r#"
n_tool_calls = 42
model_runner = "claude"
model = "haiku"
"#,
        )
        .unwrap();
        let c = Config::load(Some(&p)).unwrap();
        assert_eq!(c.n_tool_calls, 42);
        assert_eq!(c.model_runner, "claude");
        assert_eq!(c.model, "haiku");
        assert!(!c.claude_fast_model.is_empty());
    }

    #[test]
    fn zero_n_tool_calls_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.toml");
        fs::write(&p, "n_tool_calls = 0\nm_reminder = 0\n").unwrap();
        let c = Config::load(Some(&p)).unwrap();
        assert_eq!(c.n_tool_calls, 10);
        assert!(c.m_reminder > 0);
    }

    #[test]
    fn display_human_mentions_keys() {
        let d = Config::default().display_human();
        assert!(d.contains("n_tool_calls"));
        assert!(d.contains("model_runner"));
        assert!(d.contains("min_job_interval"));
    }

    #[test]
    fn scopey_home_respects_env() {
        Config::with_temp_scopey_home(|dir| {
            assert_eq!(Config::scopey_home(), dir);
        });
    }
}

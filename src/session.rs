use crate::config::Config;
use crate::pathutil::{abs_cwd, canonicalize_best_effort, escape_project_path};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long hooks may wait for the session flock before giving up (never hang the harness).
pub const HOOK_LOCK_WAIT: Duration = Duration::from_millis(1500);
/// Background jobs may wait longer — they are not on the agent critical path.
pub const JOB_LOCK_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    UserPrompt,
    ScopeRequirements,
    TrajectoryMark,
    Judgement,
    Injection,
    Note,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JudgementVerdict {
    OnTrack,
    Warning,
    OffTrack,
    /// Judge saw no actionable tool events — never inject or notify.
    InsufficientEvidence,
    Unknown,
}

/// One meaningful (or noise) tool observation for structured judging.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolEvent {
    /// 1-based meaningful tool index after this event (0 if noise / uncounted).
    pub index: u64,
    pub name: String,
    #[serde(default)]
    pub args_preview: String,
    pub ts: DateTime<Utc>,
    /// Noise tools (write_stdin, wait_agent, …) are stored optionally but never open N-windows.
    #[serde(default)]
    pub noise: bool,
}

/// Deferred judge window when spawn was throttled.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingJudgeWindow {
    pub from_count: u64,
    pub to_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JudgementStatus {
    Pending,
    Ready,
    Injected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub type_: MessageType,
    pub ts: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<JudgementVerdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<JudgementStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl SessionMessage {
    pub fn user_prompt(content: impl Into<String>, prompt_hash: impl Into<String>) -> Self {
        Self {
            type_: MessageType::UserPrompt,
            ts: Utc::now(),
            content: Some(content.into()),
            tool_count: None,
            transcript_offset: None,
            from_count: None,
            to_count: None,
            verdict: None,
            status: None,
            summary: None,
            details: None,
            kind: None,
            prompt_hash: Some(prompt_hash.into()),
            id: Some(uuid::Uuid::new_v4().to_string()),
        }
    }

    pub fn scope_requirements(content: impl Into<String>, prompt_hash: Option<String>) -> Self {
        Self {
            type_: MessageType::ScopeRequirements,
            ts: Utc::now(),
            content: Some(content.into()),
            tool_count: None,
            transcript_offset: None,
            from_count: None,
            to_count: None,
            verdict: None,
            status: None,
            summary: None,
            details: None,
            kind: None,
            prompt_hash,
            id: Some(uuid::Uuid::new_v4().to_string()),
        }
    }

    pub fn trajectory_mark(tool_count: u64, transcript_offset: Option<u64>) -> Self {
        Self {
            type_: MessageType::TrajectoryMark,
            ts: Utc::now(),
            content: None,
            tool_count: Some(tool_count),
            transcript_offset,
            from_count: None,
            to_count: None,
            verdict: None,
            status: None,
            summary: None,
            details: None,
            kind: None,
            prompt_hash: None,
            id: Some(uuid::Uuid::new_v4().to_string()),
        }
    }

    pub fn judgement(
        from_count: u64,
        to_count: u64,
        verdict: JudgementVerdict,
        status: JudgementStatus,
        summary: impl Into<String>,
        details: impl Into<String>,
    ) -> Self {
        Self {
            type_: MessageType::Judgement,
            ts: Utc::now(),
            content: None,
            tool_count: Some(to_count),
            transcript_offset: None,
            from_count: Some(from_count),
            to_count: Some(to_count),
            verdict: Some(verdict),
            status: Some(status),
            summary: Some(summary.into()),
            details: Some(details.into()),
            kind: None,
            prompt_hash: None,
            id: Some(uuid::Uuid::new_v4().to_string()),
        }
    }

    pub fn injection(kind: impl Into<String>, content: impl Into<String>, tool_count: u64) -> Self {
        Self {
            type_: MessageType::Injection,
            ts: Utc::now(),
            content: Some(content.into()),
            tool_count: Some(tool_count),
            transcript_offset: None,
            from_count: None,
            to_count: None,
            verdict: None,
            status: None,
            summary: None,
            details: None,
            kind: Some(kind.into()),
            prompt_hash: None,
            id: Some(uuid::Uuid::new_v4().to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub session_id: String,
    pub cwd: String,
    #[serde(default)]
    pub harness: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub tool_call_count: u64,
    #[serde(default)]
    pub last_judged_to_count: u64,
    #[serde(default)]
    pub last_reminder_at_count: u64,
    #[serde(default)]
    pub last_injection_at_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub messages: Vec<SessionMessageWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_judgement_id: Option<String>,
    /// Structured tool journal (hook-side). Source of truth for judge windows.
    #[serde(default)]
    pub tool_events: Vec<ToolEvent>,
    /// User prompt arrived while summarize was busy — drain when free.
    #[serde(default)]
    pub summarize_pending: bool,
    /// Judge N-boundary hit while busy — drain when free (latest window wins).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_judge: Option<PendingJudgeWindow>,
}

/// Cap retained journal size so long sessions stay small.
pub const MAX_TOOL_EVENTS: usize = 500;

/// Wire format with `type` field name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessageWire {
    #[serde(rename = "type")]
    pub type_: MessageType,
    pub ts: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<JudgementVerdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<JudgementStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl From<SessionMessage> for SessionMessageWire {
    fn from(m: SessionMessage) -> Self {
        Self {
            type_: m.type_,
            ts: m.ts,
            content: m.content,
            tool_count: m.tool_count,
            transcript_offset: m.transcript_offset,
            from_count: m.from_count,
            to_count: m.to_count,
            verdict: m.verdict,
            status: m.status,
            summary: m.summary,
            details: m.details,
            kind: m.kind,
            prompt_hash: m.prompt_hash,
            id: m.id,
        }
    }
}

impl From<SessionMessageWire> for SessionMessage {
    fn from(m: SessionMessageWire) -> Self {
        Self {
            type_: m.type_,
            ts: m.ts,
            content: m.content,
            tool_count: m.tool_count,
            transcript_offset: m.transcript_offset,
            from_count: m.from_count,
            to_count: m.to_count,
            verdict: m.verdict,
            status: m.status,
            summary: m.summary,
            details: m.details,
            kind: m.kind,
            prompt_hash: m.prompt_hash,
            id: m.id,
        }
    }
}

pub struct SessionStore {
    pub path: PathBuf,
    pub data: SessionData,
    lock_file: File,
}

#[derive(Debug)]
pub struct SessionListEntry {
    pub session_id: String,
    pub path: PathBuf,
    pub updated_at: String,
    pub tool_call_count: u64,
}

impl SessionStore {
    /// Canonical session file path: keyed by **session_id only** (not cwd).
    ///
    /// Layout: `work_root/by-id/<session_id>.json`
    ///
    /// Older layouts used `work_root/<escaped-cwd>/<session_id>.json`, which
    /// fragmented one Claude/Codex session across every `cd` into subdirs.
    /// [`open_or_create_wait`] migrates legacy files when found.
    pub fn session_path(cfg: &Config, _cwd: &Path, session_id: &str) -> Result<PathBuf> {
        Ok(Self::session_path_by_id(cfg, session_id))
    }

    pub fn session_path_by_id(cfg: &Config, session_id: &str) -> PathBuf {
        cfg.work_root
            .join("by-id")
            .join(format!("{}.json", sanitize_session_id(session_id)))
    }

    /// Legacy Claude-style path (cwd-encoded). Used only for migration lookup.
    pub fn legacy_session_path(cfg: &Config, cwd: &Path, session_id: &str) -> Result<PathBuf> {
        let abs = canonicalize_best_effort(&abs_cwd(cwd)?);
        let esc = escape_project_path(&abs);
        Ok(cfg
            .work_root
            .join(esc)
            .join(format!("{}.json", sanitize_session_id(session_id))))
    }

    pub fn open(cfg: &Config, cwd: &Path, session_id: &str) -> Result<Self> {
        let path = Self::resolve_existing_path(cfg, cwd, session_id)
            .unwrap_or_else(|| Self::session_path_by_id(cfg, session_id));
        let mut s = Self::open_path(path, session_id, cwd, JOB_LOCK_WAIT)?;
        s.touch_cwd(cwd)?;
        Ok(s)
    }

    /// Open or create with default (job) lock wait.
    pub fn open_or_create(
        cfg: &Config,
        cwd: &Path,
        session_id: &str,
        harness: &str,
    ) -> Result<Self> {
        Self::open_or_create_wait(cfg, cwd, session_id, harness, JOB_LOCK_WAIT)
    }

    /// Open or create for hooks — short lock wait so PostToolUse never hangs the harness.
    pub fn open_or_create_hook(
        cfg: &Config,
        cwd: &Path,
        session_id: &str,
        harness: &str,
    ) -> Result<Self> {
        Self::open_or_create_wait(cfg, cwd, session_id, harness, HOOK_LOCK_WAIT)
    }

    pub fn open_or_create_wait(
        cfg: &Config,
        cwd: &Path,
        session_id: &str,
        harness: &str,
        lock_wait: Duration,
    ) -> Result<Self> {
        let canonical = Self::session_path_by_id(cfg, session_id);
        let existing = Self::resolve_existing_path(cfg, cwd, session_id);

        if let Some(old_path) = existing {
            // Migrate legacy cwd-keyed file → by-id when needed.
            if old_path != canonical && !canonical.exists() {
                if let Some(parent) = canonical.parent() {
                    fs::create_dir_all(parent)?;
                }
                // Prefer rename; fall back to copy if cross-device.
                if fs::rename(&old_path, &canonical).is_err() {
                    fs::copy(&old_path, &canonical).with_context(|| {
                        format!(
                            "migrate session {} → {}",
                            old_path.display(),
                            canonical.display()
                        )
                    })?;
                    let _ = fs::remove_file(&old_path);
                }
                // Best-effort: move companion lock if present.
                let old_lock = old_path.with_extension("json.lock");
                let new_lock = canonical.with_extension("json.lock");
                if old_lock.exists() {
                    let _ = fs::rename(&old_lock, &new_lock);
                }
                crate::eventlog::info(
                    session_id,
                    "session.migrate",
                    "migrated legacy cwd-keyed session to by-id",
                    serde_json::json!({
                        "from": old_path.display().to_string(),
                        "to": canonical.display().to_string(),
                    }),
                );
            }
            let path = if canonical.exists() {
                canonical
            } else {
                old_path
            };
            let mut s = Self::open_path(path, session_id, cwd, lock_wait)?;
            if s.data.harness.is_empty() && !harness.is_empty() {
                s.data.harness = harness.to_string();
            }
            s.touch_cwd(cwd)?;
            return Ok(s);
        }

        if let Some(parent) = canonical.parent() {
            fs::create_dir_all(parent)?;
        }
        let abs = canonicalize_best_effort(&abs_cwd(cwd)?);
        let now = Utc::now();
        let data = SessionData {
            session_id: session_id.to_string(),
            cwd: abs.to_string_lossy().to_string(),
            harness: harness.to_string(),
            created_at: now,
            updated_at: now,
            tool_call_count: 0,
            last_judged_to_count: 0,
            last_reminder_at_count: 0,
            last_injection_at_count: 0,
            transcript_path: None,
            messages: vec![],
            pending_judgement_id: None,
            tool_events: vec![],
            summarize_pending: false,
            pending_judge: None,
        };
        let lock_path = canonical.with_extension("json.lock");
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open lock {}", lock_path.display()))?;
        lock_wait_exclusive(&lock_file, lock_wait)
            .with_context(|| format!("lock {}", lock_path.display()))?;
        let mut store = Self {
            path: canonical,
            data,
            lock_file,
        };
        store.persist()?;
        Ok(store)
    }

    /// Find an existing session file: by-id first, then legacy cwd path, then
    /// any work_root/**/session_id.json (other legacy cwd fragments).
    fn resolve_existing_path(cfg: &Config, cwd: &Path, session_id: &str) -> Option<PathBuf> {
        let by_id = Self::session_path_by_id(cfg, session_id);
        if by_id.exists() {
            return Some(by_id);
        }
        if let Ok(legacy) = Self::legacy_session_path(cfg, cwd, session_id) {
            if legacy.exists() {
                return Some(legacy);
            }
        }
        // Scan work_root for other cwd fragments of this session_id.
        find_legacy_session_file(&cfg.work_root, session_id)
    }

    /// Keep SessionData.cwd current when the agent cds (does not re-key the file).
    fn touch_cwd(&mut self, cwd: &Path) -> Result<()> {
        let abs = canonicalize_best_effort(&abs_cwd(cwd)?);
        let s = abs.to_string_lossy().to_string();
        if self.data.cwd != s {
            self.data.cwd = s;
        }
        Ok(())
    }

    fn open_path(
        path: PathBuf,
        session_id: &str,
        cwd: &Path,
        lock_wait: Duration,
    ) -> Result<Self> {
        let lock_path = path.with_extension("json.lock");
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open lock {}", lock_path.display()))?;
        lock_wait_exclusive(&lock_file, lock_wait)
            .with_context(|| format!("lock {}", lock_path.display()))?;

        let mut f = File::open(&path).with_context(|| format!("open session {}", path.display()))?;
        let mut buf = String::new();
        f.read_to_string(&mut buf)?;
        let mut data: SessionData = serde_json::from_str(&buf)
            .with_context(|| format!("parse session {}", path.display()))?;
        if data.session_id.is_empty() {
            data.session_id = session_id.to_string();
        }
        if data.cwd.is_empty() {
            data.cwd = abs_cwd(cwd)?.to_string_lossy().to_string();
        }
        Ok(Self {
            path,
            data,
            lock_file,
        })
    }

    pub fn persist(&mut self) -> Result<()> {
        self.data.updated_at = Utc::now();
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(&self.data)?;
        {
            let mut f = File::create(&tmp)?;
            f.write_all(json.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn append(&mut self, msg: SessionMessage) {
        self.data.messages.push(msg.into());
    }

    pub fn set_transcript(&mut self, path: Option<&Path>) {
        if let Some(p) = path {
            self.data.transcript_path = Some(p.to_string_lossy().to_string());
        }
    }

    pub fn latest_scope_requirements(&self) -> Option<String> {
        self.data
            .messages
            .iter()
            .rev()
            .find(|m| m.type_ == MessageType::ScopeRequirements)
            .and_then(|m| m.content.clone())
    }

    pub fn all_user_prompts(&self) -> Vec<String> {
        self.data
            .messages
            .iter()
            .filter(|m| m.type_ == MessageType::UserPrompt)
            .filter_map(|m| m.content.clone())
            .collect()
    }

    /// Ready judgement that has not been injected yet (prefer newest).
    /// Never returns InsufficientEvidence / OnTrack / Unknown.
    pub fn ready_judgement_for_injection(&self) -> Option<&SessionMessageWire> {
        self.data.messages.iter().rev().find(|m| {
            m.type_ == MessageType::Judgement
                && m.status == Some(JudgementStatus::Ready)
                && matches!(
                    m.verdict,
                    Some(JudgementVerdict::OffTrack) | Some(JudgementVerdict::Warning)
                )
        })
    }

    /// Drop Ready off-track/warning judgements older than `max_lag` tools past their window end.
    pub fn expire_stale_judgements(&mut self, current_count: u64, max_lag: u64) -> usize {
        let mut n = 0;
        for m in self.data.messages.iter_mut() {
            if m.type_ != MessageType::Judgement || m.status != Some(JudgementStatus::Ready) {
                continue;
            }
            if !matches!(
                m.verdict,
                Some(JudgementVerdict::OffTrack) | Some(JudgementVerdict::Warning)
            ) {
                continue;
            }
            let to = m.to_count.unwrap_or(0);
            if current_count.saturating_sub(to) > max_lag {
                m.status = Some(JudgementStatus::Failed);
                m.summary = Some(format!(
                    "stale judgement discarded (window ended at {to}, now at {current_count})"
                ));
                n += 1;
            }
        }
        n
    }

    pub fn append_tool_event(&mut self, ev: ToolEvent) {
        self.data.tool_events.push(ev);
        if self.data.tool_events.len() > MAX_TOOL_EVENTS {
            let excess = self.data.tool_events.len() - MAX_TOOL_EVENTS;
            self.data.tool_events.drain(0..excess);
        }
    }

    pub fn tool_events_in_window(&self, from: u64, to: u64) -> Vec<ToolEvent> {
        crate::tool_journal::events_in_window(&self.data.tool_events, from, to)
    }

    pub fn mark_summarize_pending(&mut self) {
        self.data.summarize_pending = true;
    }

    pub fn clear_summarize_pending(&mut self) {
        self.data.summarize_pending = false;
    }

    pub fn set_pending_judge(&mut self, from: u64, to: u64) {
        self.data.pending_judge = Some(PendingJudgeWindow {
            from_count: from,
            to_count: to,
        });
    }



    pub fn mark_judgement_injected(&mut self, id: &str) {
        for m in self.data.messages.iter_mut() {
            if m.id.as_deref() == Some(id) {
                m.status = Some(JudgementStatus::Injected);
            }
        }
        if self.data.pending_judgement_id.as_deref() == Some(id) {
            self.data.pending_judgement_id = None;
        }
    }

    pub fn upsert_judgement(&mut self, msg: SessionMessage) {
        let id = msg.id.clone();
        // Replace pending judgement for same window if present.
        if let (Some(from), Some(to)) = (msg.from_count, msg.to_count) {
            self.data.messages.retain(|m| {
                !(m.type_ == MessageType::Judgement
                    && m.from_count == Some(from)
                    && m.to_count == Some(to)
                    && m.status == Some(JudgementStatus::Pending))
            });
        }
        if msg.status == Some(JudgementStatus::Ready)
            || msg.status == Some(JudgementStatus::Pending)
        {
            self.data.pending_judgement_id = id.clone();
        }
        self.append(msg);
    }

    pub fn summary(&self) -> String {
        let scope = self
            .latest_scope_requirements()
            .unwrap_or_else(|| "(no scope requirements yet)".into());
        let scope_preview: String = scope.chars().take(400).collect();
        let last_j = self
            .data
            .messages
            .iter()
            .rev()
            .find(|m| m.type_ == MessageType::Judgement);
        let j_line = match last_j {
            Some(j) => format!(
                "last_judgement: {:?} {:?} — {}",
                j.verdict,
                j.status,
                j.summary.clone().unwrap_or_default()
            ),
            None => "last_judgement: (none)".into(),
        };
        format!(
            "session_id: {}\n\
             path: {}\n\
             cwd: {}\n\
             harness: {}\n\
             tool_call_count: {}\n\
             last_judged_to_count: {}\n\
             last_reminder_at_count: {}\n\
             transcript_path: {}\n\
             {j_line}\n\
             \n\
             scope_requirements:\n{scope_preview}\n",
            self.data.session_id,
            self.path.display(),
            self.data.cwd,
            self.data.harness,
            self.data.tool_call_count,
            self.data.last_judged_to_count,
            self.data.last_reminder_at_count,
            self.data
                .transcript_path
                .clone()
                .unwrap_or_else(|| "(none)".into()),
        )
    }

    pub fn list(cfg: &Config, cwd: Option<&Path>, limit: usize) -> Result<Vec<SessionListEntry>> {
        let mut out = Vec::new();
        if !cfg.work_root.exists() {
            return Ok(out);
        }
        let filter_cwd = if let Some(c) = cwd {
            let abs = canonicalize_best_effort(&abs_cwd(c)?);
            Some(abs.to_string_lossy().to_string())
        } else {
            None
        };

        let mut seen_ids = std::collections::HashSet::new();
        for p in walk_session_json_files(&cfg.work_root) {
            let meta = match fs::read_to_string(&p) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let data: SessionData = match serde_json::from_str(&meta) {
                Ok(d) => d,
                Err(_) => continue,
            };
            if let Some(ref want) = filter_cwd {
                // Match exact cwd or prefix (session may have last touched a subdir).
                if data.cwd != *want && !data.cwd.starts_with(want) && !want.starts_with(&data.cwd) {
                    continue;
                }
            }
            // Prefer by-id path when duplicates exist after partial migration.
            if !seen_ids.insert(data.session_id.clone()) {
                continue;
            }
            out.push(SessionListEntry {
                session_id: data.session_id,
                path: p,
                updated_at: data.updated_at.to_rfc3339(),
                tool_call_count: data.tool_call_count,
            });
        }
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out.truncate(limit);
        Ok(out)
    }
}

/// Sanitize session id for a single path component.
pub fn sanitize_session_id(session_id: &str) -> String {
    let s: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "unknown".into()
    } else {
        s
    }
}

fn walk_session_json_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().and_then(|x| x.to_str()) == Some("json") {
                // Skip lock companions and non-session junk.
                if p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".json.lock") || n.ends_with(".tmp"))
                {
                    continue;
                }
                out.push(p);
            }
        }
    }
    walk(root, &mut out);
    // Prefer by-id paths first so list de-dup keeps the canonical file.
    out.sort_by(|a, b| {
        let a_id = a.to_string_lossy().contains("/by-id/");
        let b_id = b.to_string_lossy().contains("/by-id/");
        b_id.cmp(&a_id).then_with(|| a.cmp(b))
    });
    out
}

fn find_legacy_session_file(work_root: &Path, session_id: &str) -> Option<PathBuf> {
    let want = format!("{}.json", sanitize_session_id(session_id));
    for p in walk_session_json_files(work_root) {
        if p.file_name().and_then(|n| n.to_str()) == Some(want.as_str()) {
            // Skip already-canonical by-id (caller checked that first).
            if p.to_string_lossy().contains("/by-id/") {
                continue;
            }
            return Some(p);
        }
    }
    None
}

impl Drop for SessionStore {
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
    }
}

pub fn hash_prompt(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// Acquire exclusive flock without hanging forever. Spins with short sleeps.
fn lock_wait_exclusive(file: &File, max_wait: Duration) -> Result<()> {
    let start = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(_) if start.elapsed() < max_wait => {
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(e) => {
                bail!(
                    "session store lock busy for {}ms (another scopey job may be holding it): {e}",
                    start.elapsed().as_millis()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg(work: &Path) -> Config {
        let mut c = Config::default();
        c.work_root = work.to_path_buf();
        c
    }

    #[test]
    fn hash_prompt_stable() {
        assert_eq!(hash_prompt("a"), hash_prompt("a"));
        assert_ne!(hash_prompt("a"), hash_prompt("b"));
        assert_eq!(hash_prompt("a").len(), 64);
    }

    #[test]
    fn session_path_keyed_by_id_not_cwd() {
        let cfg = test_cfg(Path::new("/tmp/scopey-work"));
        let p1 = SessionStore::session_path(&cfg, Path::new("/Users/me/proj"), "sid1").unwrap();
        let p2 =
            SessionStore::session_path(&cfg, Path::new("/Users/me/proj/subdir"), "sid1").unwrap();
        assert_eq!(p1, p2);
        let s = p1.to_string_lossy();
        assert!(s.contains("by-id"));
        assert!(s.ends_with("sid1.json"));
        assert!(!s.contains("-Users-me-proj"));
    }

    #[test]
    fn open_same_session_across_cwd_shares_store() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let cwd1 = dir.path().join("proj");
        let cwd2 = dir.path().join("proj").join("subdir");
        fs::create_dir_all(&cwd2).unwrap();
        {
            let mut s = SessionStore::open_or_create(&cfg, &cwd1, "shared-sid", "claude").unwrap();
            s.data.tool_call_count = 7;
            s.persist().unwrap();
        }
        let s2 = SessionStore::open_or_create(&cfg, &cwd2, "shared-sid", "claude").unwrap();
        assert_eq!(s2.data.tool_call_count, 7);
        assert!(s2.path.to_string_lossy().contains("by-id"));
    }

    #[test]
    fn migrates_legacy_cwd_keyed_file() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let cwd = dir.path().join("proj");
        fs::create_dir_all(&cwd).unwrap();
        // Plant a legacy file.
        let legacy = SessionStore::legacy_session_path(&cfg, &cwd, "mig-1").unwrap();
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        let now = Utc::now();
        let data = SessionData {
            session_id: "mig-1".into(),
            cwd: cwd.to_string_lossy().to_string(),
            harness: "claude".into(),
            created_at: now,
            updated_at: now,
            tool_call_count: 42,
            last_judged_to_count: 0,
            last_reminder_at_count: 0,
            last_injection_at_count: 0,
            transcript_path: None,
            messages: vec![],
            pending_judgement_id: None,
            tool_events: vec![],
            summarize_pending: false,
            pending_judge: None,
        };
        fs::write(&legacy, serde_json::to_string_pretty(&data).unwrap()).unwrap();
        assert!(legacy.exists());

        let s = SessionStore::open_or_create(&cfg, &cwd, "mig-1", "claude").unwrap();
        assert_eq!(s.data.tool_call_count, 42);
        assert!(s.path.to_string_lossy().contains("by-id"));
        assert!(!legacy.exists(), "legacy file should be moved");
    }

    #[test]
    fn open_create_persist_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let cwd = dir.path().join("proj");
        fs::create_dir_all(&cwd).unwrap();
        let sid = "roundtrip-1";
        {
            let mut s = SessionStore::open_or_create(&cfg, &cwd, sid, "claude").unwrap();
            s.append(SessionMessage::user_prompt("do the thing", hash_prompt("do the thing")));
            s.append(SessionMessage::scope_requirements("- stay on task", Some("h".into())));
            s.data.tool_call_count = 5;
            s.set_transcript(Some(Path::new("/tmp/t.jsonl")));
            s.persist().unwrap();
        }
        let s2 = SessionStore::open(&cfg, &cwd, sid).unwrap();
        assert_eq!(s2.data.session_id, sid);
        assert_eq!(s2.data.harness, "claude");
        assert_eq!(s2.data.tool_call_count, 5);
        assert_eq!(s2.all_user_prompts(), vec!["do the thing".to_string()]);
        assert_eq!(
            s2.latest_scope_requirements().as_deref(),
            Some("- stay on task")
        );
        assert!(s2.summary().contains("roundtrip-1"));
    }

    #[test]
    fn judgement_ready_then_injected() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let cwd = dir.path().join("proj");
        fs::create_dir_all(&cwd).unwrap();
        let mut s = SessionStore::open_or_create(&cfg, &cwd, "j1", "claude").unwrap();
        s.append(SessionMessage::scope_requirements("- x", None));
        let j = SessionMessage::judgement(
            0,
            10,
            JudgementVerdict::OffTrack,
            JudgementStatus::Ready,
            "drifted",
            "details here",
        );
        let jid = j.id.clone().unwrap();
        s.upsert_judgement(j);
        assert!(s.ready_judgement_for_injection().is_some());
        assert_eq!(
            s.ready_judgement_for_injection().unwrap().summary.as_deref(),
            Some("drifted")
        );
        s.mark_judgement_injected(&jid);
        assert!(s.ready_judgement_for_injection().is_none());
    }

    #[test]
    fn on_track_judgement_not_injected() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let cwd = dir.path().join("proj");
        fs::create_dir_all(&cwd).unwrap();
        let mut s = SessionStore::open_or_create(&cfg, &cwd, "j2", "claude").unwrap();
        s.upsert_judgement(SessionMessage::judgement(
            0,
            10,
            JudgementVerdict::OnTrack,
            JudgementStatus::Ready,
            "ok",
            "",
        ));
        assert!(s.ready_judgement_for_injection().is_none());
    }

    #[test]
    fn insufficient_evidence_not_injected() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let cwd = dir.path().join("proj");
        fs::create_dir_all(&cwd).unwrap();
        let mut s = SessionStore::open_or_create(&cfg, &cwd, "j3", "codex").unwrap();
        s.upsert_judgement(SessionMessage::judgement(
            0,
            15,
            JudgementVerdict::InsufficientEvidence,
            JudgementStatus::Ready,
            "no tools",
            "",
        ));
        assert!(s.ready_judgement_for_injection().is_none());
    }

    #[test]
    fn stale_judgements_expire() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let cwd = dir.path().join("proj");
        fs::create_dir_all(&cwd).unwrap();
        let mut s = SessionStore::open_or_create(&cfg, &cwd, "j4", "codex").unwrap();
        let j = SessionMessage::judgement(
            0,
            15,
            JudgementVerdict::Warning,
            JudgementStatus::Ready,
            "old",
            "",
        );
        s.upsert_judgement(j);
        assert!(s.ready_judgement_for_injection().is_some());
        let n = s.expire_stale_judgements(100, 30);
        assert_eq!(n, 1);
        assert!(s.ready_judgement_for_injection().is_none());
    }

    #[test]
    fn tool_events_window_and_cap() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let cwd = dir.path().join("proj");
        fs::create_dir_all(&cwd).unwrap();
        let mut s = SessionStore::open_or_create(&cfg, &cwd, "j5", "codex").unwrap();
        for i in 1..=20 {
            s.append_tool_event(ToolEvent {
                index: i,
                name: format!("t{i}"),
                args_preview: "x".into(),
                ts: Utc::now(),
                noise: false,
            });
        }
        let w = s.tool_events_in_window(10, 15);
        assert_eq!(w.len(), 5);
        assert_eq!(w[0].index, 11);
    }

    #[test]
    fn hook_lock_wait_gives_up_quickly() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let cwd = dir.path().join("proj");
        fs::create_dir_all(&cwd).unwrap();
        let s1 = SessionStore::open_or_create(&cfg, &cwd, "lock-me", "codex").unwrap();
        // Hold s1's exclusive lock; a concurrent open with short wait must fail fast.
        let start = Instant::now();
        let err = SessionStore::open_or_create_wait(
            &cfg,
            &cwd,
            "lock-me",
            "codex",
            Duration::from_millis(80),
        );
        assert!(err.is_err(), "expected lock busy error");
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "hook wait must not hang: {:?}",
            start.elapsed()
        );
        drop(s1);
    }

    #[test]
    fn list_sessions_finds_file() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let cwd = dir.path().join("proj");
        fs::create_dir_all(&cwd).unwrap();
        let mut s = SessionStore::open_or_create(&cfg, &cwd, "list-me", "codex").unwrap();
        s.persist().unwrap();
        let list = SessionStore::list(&cfg, Some(&cwd), 10).unwrap();
        assert!(list.iter().any(|e| e.session_id == "list-me"));
    }

    #[test]
    fn wire_type_field_serializes_as_type() {
        let m = SessionMessage::user_prompt("hi", "abc");
        let w: SessionMessageWire = m.into();
        let v = serde_json::to_value(&w).unwrap();
        assert!(v.get("type").is_some());
        assert_eq!(v["type"], "user_prompt");
    }
}

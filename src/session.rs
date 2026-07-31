use crate::config::Config;
use crate::pathutil::{abs_cwd, canonicalize_best_effort, escape_project_path};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

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
    Unknown,
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
}

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
    pub fn session_path(cfg: &Config, cwd: &Path, session_id: &str) -> Result<PathBuf> {
        let abs = canonicalize_best_effort(&abs_cwd(cwd)?);
        let esc = escape_project_path(&abs);
        Ok(cfg.work_root.join(esc).join(format!("{session_id}.json")))
    }

    pub fn open(cfg: &Config, cwd: &Path, session_id: &str) -> Result<Self> {
        let path = Self::session_path(cfg, cwd, session_id)?;
        Self::open_path(path, session_id, cwd)
    }

    pub fn open_or_create(
        cfg: &Config,
        cwd: &Path,
        session_id: &str,
        harness: &str,
    ) -> Result<Self> {
        let path = Self::session_path(cfg, cwd, session_id)?;
        if path.exists() {
            let mut s = Self::open_path(path, session_id, cwd)?;
            if s.data.harness.is_empty() && !harness.is_empty() {
                s.data.harness = harness.to_string();
            }
            return Ok(s);
        }
        if let Some(parent) = path.parent() {
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
        };
        let lock_path = path.with_extension("json.lock");
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open lock {}", lock_path.display()))?;
        lock_file.lock_exclusive()?;
        let mut store = Self {
            path,
            data,
            lock_file,
        };
        store.persist()?;
        Ok(store)
    }

    fn open_path(path: PathBuf, session_id: &str, cwd: &Path) -> Result<Self> {
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
        lock_file.lock_exclusive()?;

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
        let filter_esc = if let Some(c) = cwd {
            let abs = canonicalize_best_effort(&abs_cwd(c)?);
            Some(escape_project_path(&abs))
        } else {
            None
        };

        for entry in fs::read_dir(&cfg.work_root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(ref esc) = filter_esc {
                if &name != esc {
                    continue;
                }
            }
            for f in fs::read_dir(entry.path())? {
                let f = f?;
                let p = f.path();
                if p.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.ends_with(".json"))
                {
                    continue;
                }
                let meta = match fs::read_to_string(&p) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let data: SessionData = match serde_json::from_str(&meta) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                out.push(SessionListEntry {
                    session_id: data.session_id,
                    path: p,
                    updated_at: data.updated_at.to_rfc3339(),
                    tool_call_count: data.tool_call_count,
                });
            }
        }
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out.truncate(limit);
        Ok(out)
    }
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
    fn session_path_uses_claude_escape() {
        let cfg = test_cfg(Path::new("/tmp/scopey-work"));
        let p = SessionStore::session_path(&cfg, Path::new("/Users/me/proj"), "sid1").unwrap();
        let s = p.to_string_lossy();
        assert!(s.contains("-Users-me-proj"));
        assert!(s.ends_with("sid1.json"));
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

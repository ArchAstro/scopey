use crate::config::Config;
use crate::session::{
    normalize_judgement_verdict, JudgementStatus, JudgementVerdict, MessageType, SessionData,
    SessionMessageWire, SessionStore,
};
use crate::term_viz::{self, Caps, Graphics};
use crate::transcript_tokens::{self, TranscriptTokens};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Days, Local, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

type DateRange = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);

#[derive(Debug)]
pub struct InsightArgs {
    pub session: Option<String>,
    pub date: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub cwd: Option<PathBuf>,
    pub harness: Option<String>,
    pub verdict: Option<String>,
    pub off_scope: bool,
    pub include_empty: bool,
    pub limit: usize,
    pub details: bool,
    pub patterns: bool,
    pub tokens: String,
    pub graphics: String,
    pub json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenScope {
    Shown,
    All,
    Off,
}

impl TokenScope {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "shown" => Some(TokenScope::Shown),
            "all" => Some(TokenScope::All),
            "off" | "none" => Some(TokenScope::Off),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct InsightQuery {
    session: Option<String>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    harness: Option<String>,
    verdict: Option<JudgementVerdict>,
    off_scope: bool,
    include_empty: bool,
    limit: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct InsightCounts {
    judgements: usize,
    evaluated: usize,
    on_track: usize,
    warning: usize,
    off_track: usize,
    insufficient_evidence: usize,
    unknown: usize,
    attention_rate_pct: f64,
}

#[derive(Debug, Default, Serialize)]
pub struct InsightTotals {
    sessions: usize,
    sessions_evaluated: usize,
    sessions_off_track: usize,
    sessions_warning: usize,
    sessions_unevaluated: usize,
    contaminated_scope_sessions: usize,
    evaluation_coverage_pct: f64,
    judge_failure_rate_pct: f64,
    #[serde(flatten)]
    counts: InsightCounts,
}

impl std::ops::Deref for InsightTotals {
    type Target = InsightCounts;

    fn deref(&self) -> &Self::Target {
        &self.counts
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InsightSignal {
    timestamp: String,
    verdict: JudgementVerdict,
    status: Option<JudgementStatus>,
    from_count: Option<u64>,
    to_count: Option<u64>,
    summary: String,
    details: String,
}

#[derive(Debug, Serialize)]
pub struct SessionInsight {
    session_id: String,
    harness: String,
    cwd: String,
    created_at: String,
    updated_at: String,
    tool_call_count: u64,
    severity: String,
    counts: InsightCounts,
    scope: String,
    scope_quality: ScopeQuality,
    boundary: &'static str,
    drift_onsets_pct: Vec<f64>,
    drift_archetypes: Vec<&'static str>,
    corrections: usize,
    corrections_recovered: usize,
    corrections_repeat: usize,
    tokens: Option<TranscriptTokens>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scopey_usage: Option<ScopeyMeasured>,
    latest_signal: Option<InsightSignal>,
    signals: Vec<InsightSignal>,
    #[serde(skip)]
    drift_window_archetypes: Vec<Vec<&'static str>>,
    #[serde(skip)]
    drift_onset_tools: Vec<u64>,
    #[serde(skip)]
    transcript_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LengthBucketRow {
    label: &'static str,
    sessions: usize,
    drifted: usize,
    judged: usize,
    drift_windows: usize,
}

#[derive(Debug, Serialize)]
pub struct DriftPatterns {
    drift_windows: usize,
    drift_sessions: usize,
    /// (archetype, windows tagged) sorted descending; multi-label.
    archetype_counts: Vec<(&'static str, usize)>,
    /// Relative onset position within the session, ten buckets 0–100%.
    onset_histogram: [usize; 10],
    /// Absolute tool count at detection: p25 / p50 / p75.
    onset_tool_quartiles: [u64; 3],
    boundary_explicit: usize,
    boundary_implicit: usize,
    by_length: Vec<LengthBucketRow>,
    corrections: usize,
    corrections_followed: usize,
    corrections_recovered: usize,
}

/// Measured analyzer usage for one session, summed from the per-call
/// records Scopey persists as it runs. A floor, never an estimate: calls
/// whose runner exposed no usage are absent.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ScopeyMeasured {
    calls: usize,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct TokenTotals {
    scope: &'static str,
    sessions_counted: usize,
    input: u64,
    cached: u64,
    output: u64,
    total: u64,
    scopey_sessions: usize,
    scopey_calls: usize,
    scopey_total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeQuality {
    Ok,
    Missing,
    Contaminated,
}

#[derive(Debug, Serialize)]
pub struct InsightReport {
    generated_at: String,
    filters: Vec<String>,
    totals: InsightTotals,
    #[serde(skip_serializing_if = "Option::is_none")]
    patterns: Option<DriftPatterns>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_totals: Option<TokenTotals>,
    sessions: Vec<SessionInsight>,
    excluded_empty_sessions: usize,
    skipped_files: usize,
}

pub fn run(cfg: &Config, args: InsightArgs) -> Result<()> {
    if args.limit == 0 {
        bail!("--limit must be greater than zero");
    }
    let (since, until) = parse_date_filters(
        args.date.as_deref(),
        args.since.as_deref(),
        args.until.as_deref(),
    )?;
    if let (Some(start), Some(end)) = (since, until) {
        if start > end {
            bail!("--since must be before --until");
        }
    }
    let verdict = args.verdict.as_deref().map(parse_verdict).transpose()?;
    let query = InsightQuery {
        session: args.session.clone(),
        since,
        until,
        harness: args.harness.as_ref().map(|s| s.to_ascii_lowercase()),
        verdict,
        off_scope: args.off_scope,
        include_empty: args.include_empty || args.session.is_some(),
        limit: args.limit,
    };

    let list = SessionStore::list(cfg, args.cwd.as_deref(), usize::MAX)?;
    let mut sessions = Vec::with_capacity(list.len());
    let mut skipped_files = 0;
    for entry in list {
        match fs::read_to_string(&entry.path)
            .with_context(|| format!("read {}", entry.path.display()))
            .and_then(|raw| {
                serde_json::from_str::<SessionData>(&raw)
                    .with_context(|| format!("parse {}", entry.path.display()))
            }) {
            Ok(data) => sessions.push(data),
            Err(error) => {
                skipped_files += 1;
                if std::env::var_os("SCOPEY_DEBUG").is_some() {
                    eprintln!("scopey insights: {error:#}");
                }
            }
        }
    }

    resolve_session_selector(&mut sessions, query.session.as_deref())?;
    let mut filters = describe_filters(&args, since, until);
    if let Some(cwd) = args.cwd {
        filters.push(format!("cwd={}", cwd.display()));
    }
    let token_scope = TokenScope::parse(&args.tokens)
        .with_context(|| format!("invalid --tokens {:?}; use shown|all|off", args.tokens))?;
    let graphics = Graphics::parse(&args.graphics)
        .with_context(|| format!("invalid --graphics {:?}; use auto|kitty|off", args.graphics))?;
    let report = analyze_sessions(
        sessions,
        &query,
        filters,
        skipped_files,
        args.patterns,
        token_scope,
    );
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let caps = term_viz::detect(graphics);
        print_human(&report, args.details, &cfg.work_root, caps);
    }
    Ok(())
}

fn parse_date_filters(
    date: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<DateRange> {
    if let Some(day) = date {
        return Ok((Some(parse_day_start(day)?), Some(parse_day_end(day)?)));
    }
    Ok((
        since.map(parse_start).transpose()?,
        until.map(parse_end).transpose()?,
    ))
}

fn parse_start(value: &str) -> Result<DateTime<Utc>> {
    if NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok() {
        parse_day_start(value)
    } else {
        parse_rfc3339(value)
    }
}

fn parse_end(value: &str) -> Result<DateTime<Utc>> {
    if NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok() {
        parse_day_end(value)
    } else {
        parse_rfc3339(value)
    }
}

fn parse_day_start(value: &str) -> Result<DateTime<Utc>> {
    let day = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("invalid date {value:?}; use YYYY-MM-DD"))?;
    let midnight = day.and_hms_opt(0, 0, 0).context("invalid midnight")?;
    let local = Local
        .from_local_datetime(&midnight)
        .earliest()
        .with_context(|| format!("local date {value:?} does not have a midnight"))?;
    Ok(local.with_timezone(&Utc))
}

fn parse_day_end(value: &str) -> Result<DateTime<Utc>> {
    let day = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("invalid date {value:?}; use YYYY-MM-DD"))?;
    let next = day
        .checked_add_days(Days::new(1))
        .context("date is out of range")?;
    let midnight = next.and_hms_opt(0, 0, 0).context("invalid midnight")?;
    let local = Local
        .from_local_datetime(&midnight)
        .latest()
        .with_context(|| format!("day after {value:?} does not have a midnight"))?;
    Ok(local.with_timezone(&Utc) - chrono::Duration::nanoseconds(1))
}

fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .with_context(|| format!("invalid time {value:?}; use YYYY-MM-DD or RFC3339"))
}

fn parse_verdict(value: &str) -> Result<JudgementVerdict> {
    match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "on-track" | "ontrack" | "ok" => Ok(JudgementVerdict::OnTrack),
        "warning" | "warn" => Ok(JudgementVerdict::Warning),
        "off-track" | "offtrack" => Ok(JudgementVerdict::OffTrack),
        "insufficient-evidence" | "insufficient" | "no-evidence" => {
            Ok(JudgementVerdict::InsufficientEvidence)
        }
        "unknown" => Ok(JudgementVerdict::Unknown),
        _ => bail!(
            "unknown verdict {value:?}; use on-track|warning|off-track|insufficient-evidence|unknown"
        ),
    }
}

fn resolve_session_selector(sessions: &mut Vec<SessionData>, selector: Option<&str>) -> Result<()> {
    let Some(selector) = selector else {
        return Ok(());
    };
    if sessions.iter().any(|s| s.session_id == selector) {
        sessions.retain(|s| s.session_id == selector);
        return Ok(());
    }
    let matches: Vec<String> = sessions
        .iter()
        .filter(|s| s.session_id.starts_with(selector))
        .map(|s| s.session_id.clone())
        .collect();
    if matches.len() > 1 {
        bail!(
            "session prefix {selector:?} is ambiguous; matches {}",
            matches.into_iter().take(5).collect::<Vec<_>>().join(", ")
        );
    }
    sessions.retain(|s| s.session_id.starts_with(selector));
    Ok(())
}

/// Drift archetype taxonomy, ported verbatim from the eval's
/// `drift_pattern_analysis.py` so the product view and the eval tooling
/// cannot diverge. Multi-label; keyword membership over summary+details.
const ARCHETYPES: [(&str, &[&str]); 8] = [
    (
        "analysis→implementation",
        &[
            "analysis-only",
            "analysis only",
            "research",
            "plan",
            "prototype",
            "instead of analyz",
            "implementation plan",
            "began implement",
            "started implement",
            "diagnos",
        ],
    ),
    (
        "out-of-scope files",
        &[
            "out of scope",
            "outside the scope",
            "unrelated",
            "not part of",
            "beyond the request",
            "different module",
            "excluded",
            "out-of-scope",
        ],
    ),
    (
        "unauthorized tests",
        &["test", "unittest", "pytest", "spec"],
    ),
    (
        "docs/readme",
        &["readme", "documentation", "docs", "changelog", "comment"],
    ),
    (
        "refactor/cleanup",
        &[
            "refactor",
            "cleanup",
            "clean up",
            "rename",
            "reorganiz",
            "restructur",
        ],
    ),
    (
        "dependency/tooling",
        &[
            "dependency",
            "dependencies",
            "install",
            "npm",
            "pip ",
            "cargo add",
            "upgrade",
            "version bump",
            "lint",
        ],
    ),
    (
        "vcs/release",
        &[
            "commit",
            "push",
            "branch",
            "merge",
            "release",
            "tag ",
            "pull request",
            "pr #",
        ],
    ),
    (
        "config/infra",
        &[
            "config",
            "settings",
            "deploy",
            "ci ",
            "workflow",
            "environment",
            "infra",
        ],
    ),
];

fn classify_archetypes(text: &str) -> Vec<&'static str> {
    let lowered = text.to_lowercase();
    let labels: Vec<&'static str> = ARCHETYPES
        .iter()
        .filter(|(_, needles)| needles.iter().any(|n| lowered.contains(n)))
        .map(|(name, _)| *name)
        .collect();
    if labels.is_empty() {
        vec!["other"]
    } else {
        labels
    }
}

const EXPLICIT_BOUNDARY_MARKERS: [&str; 15] = [
    "do not",
    "don't",
    "only ",
    "read-only",
    "analysis only",
    "analysis-only",
    "no edits",
    "no changes",
    "avoid ",
    "out of scope",
    "must not",
    "without ",
    "stay within",
    "restrict",
    "limit ",
];

fn boundary_kind(scope: &str) -> &'static str {
    let lowered = scope.to_lowercase();
    if EXPLICIT_BOUNDARY_MARKERS
        .iter()
        .any(|m| lowered.contains(m))
    {
        "explicit"
    } else {
        "implicit"
    }
}

/// Per-correction recovery, same walk as the eval's `analyze_session`: a
/// correction "recovers" when the next completed judgement after it lands
/// on_track; a drift-class verdict there means repeat drift.
fn recovery_walk(messages: &[&SessionMessageWire]) -> (usize, usize, usize) {
    let mut pending: Vec<u64> = Vec::new();
    let (mut corrections, mut recovered, mut repeat) = (0usize, 0usize, 0usize);
    for message in messages {
        match message.type_ {
            MessageType::Injection
                if message
                    .kind
                    .as_deref()
                    .is_some_and(|k| k.starts_with("correction")) =>
            {
                corrections += 1;
                pending.push(message.tool_count.unwrap_or(0));
            }
            MessageType::Judgement => {
                if message.status == Some(JudgementStatus::Pending) {
                    continue;
                }
                let Some(verdict) = message.verdict.as_ref() else {
                    continue;
                };
                if !matches!(
                    verdict,
                    JudgementVerdict::OnTrack
                        | JudgementVerdict::Warning
                        | JudgementVerdict::OffTrack
                ) {
                    continue;
                }
                let to = message.to_count.unwrap_or(0);
                pending.retain(|at| {
                    if to > *at {
                        if matches!(verdict, JudgementVerdict::OnTrack) {
                            recovered += 1;
                        } else {
                            repeat += 1;
                        }
                        false
                    } else {
                        true
                    }
                });
            }
            _ => {}
        }
    }
    (corrections, recovered, repeat)
}

fn length_bucket(tools: u64) -> &'static str {
    match tools {
        0..=24 => "<25 tools",
        25..=99 => "25–99",
        100..=249 => "100–249",
        250..=499 => "250–499",
        _ => "500+",
    }
}

const LENGTH_BUCKET_ORDER: [&str; 5] = ["<25 tools", "25–99", "100–249", "250–499", "500+"];

fn build_patterns(sessions: &[SessionInsight]) -> DriftPatterns {
    let mut archetype_counts: Vec<(&'static str, usize)> = Vec::new();
    let mut onset_histogram = [0usize; 10];
    let mut onset_tools: Vec<u64> = Vec::new();
    let (mut explicit, mut implicit) = (0usize, 0usize);
    let (mut drift_windows, mut drift_sessions) = (0usize, 0usize);
    let (mut corrections, mut followed, mut recovered) = (0usize, 0usize, 0usize);
    let mut buckets: std::collections::HashMap<&'static str, LengthBucketRow> =
        std::collections::HashMap::new();
    for session in sessions {
        let bucket = buckets
            .entry(length_bucket(session.tool_call_count))
            .or_insert_with(|| LengthBucketRow {
                label: length_bucket(session.tool_call_count),
                sessions: 0,
                drifted: 0,
                judged: 0,
                drift_windows: 0,
            });
        bucket.sessions += 1;
        bucket.judged += session.counts.evaluated;
        bucket.drift_windows += session.drift_onsets_pct.len();
        if !session.drift_onsets_pct.is_empty() {
            bucket.drifted += 1;
            drift_sessions += 1;
        }
        drift_windows += session.drift_onsets_pct.len();
        for labels in &session.drift_window_archetypes {
            for label in labels {
                match archetype_counts.iter_mut().find(|(name, _)| name == label) {
                    Some((_, count)) => *count += 1,
                    None => archetype_counts.push((label, 1)),
                }
            }
            if session.boundary == "explicit" {
                explicit += 1;
            } else {
                implicit += 1;
            }
        }
        for pct in &session.drift_onsets_pct {
            let index = ((pct / 10.0).floor() as usize).min(9);
            onset_histogram[index] += 1;
        }
        onset_tools.extend(&session.drift_onset_tools);
        corrections += session.corrections;
        followed += session.corrections_recovered + session.corrections_repeat;
        recovered += session.corrections_recovered;
    }
    archetype_counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    onset_tools.sort_unstable();
    let quartile = |q: f64| -> u64 {
        if onset_tools.is_empty() {
            0
        } else {
            let index = ((onset_tools.len() as f64 * q) as usize).min(onset_tools.len() - 1);
            onset_tools[index]
        }
    };
    let by_length = LENGTH_BUCKET_ORDER
        .iter()
        .filter_map(|label| buckets.remove(label))
        .collect();
    DriftPatterns {
        drift_windows,
        drift_sessions,
        archetype_counts,
        onset_histogram,
        onset_tool_quartiles: [quartile(0.25), quartile(0.5), quartile(0.75)],
        boundary_explicit: explicit,
        boundary_implicit: implicit,
        by_length,
        corrections,
        corrections_followed: followed,
        corrections_recovered: recovered,
    }
}

fn fill_tokens(sessions: &mut [SessionInsight]) {
    for session in sessions {
        if session.tokens.is_some() {
            continue;
        }
        session.tokens = session
            .transcript_path
            .as_deref()
            .map(Path::new)
            .filter(|p| p.is_file())
            .and_then(|p| transcript_tokens::read(p, &session.harness));
    }
}

fn token_totals(sessions: &[SessionInsight], scope: &'static str) -> TokenTotals {
    let mut totals = TokenTotals {
        scope,
        ..TokenTotals::default()
    };
    for tokens in sessions.iter().filter_map(|s| s.tokens.as_ref()) {
        totals.sessions_counted += 1;
        totals.input += tokens.input;
        totals.cached += tokens.cached;
        totals.output += tokens.output;
        totals.total += tokens.total;
    }
    for measured in sessions.iter().filter_map(|s| s.scopey_usage.as_ref()) {
        totals.scopey_sessions += 1;
        totals.scopey_calls += measured.calls;
        totals.scopey_total += measured.total_tokens;
    }
    totals
}

fn analyze_sessions(
    sessions: Vec<SessionData>,
    query: &InsightQuery,
    filters: Vec<String>,
    skipped_files: usize,
    patterns_enabled: bool,
    token_scope: TokenScope,
) -> InsightReport {
    let mut analyzed = Vec::new();
    let mut excluded_empty_sessions = 0;
    for data in sessions {
        if query
            .harness
            .as_ref()
            .is_some_and(|want| !data.harness.eq_ignore_ascii_case(want))
        {
            continue;
        }
        if !session_overlaps(&data, query.since, query.until) {
            continue;
        }
        if data.tool_call_count == 0 && !query.include_empty {
            excluded_empty_sessions += 1;
            continue;
        }

        let mut judgement_messages: Vec<&SessionMessageWire> = data
            .messages
            .iter()
            .filter(|m| m.type_ == MessageType::Judgement)
            .filter(|m| m.status != Some(JudgementStatus::Pending))
            .filter(|m| in_range(m.ts, query.since, query.until))
            .collect();
        judgement_messages.sort_by_key(|m| m.ts);

        let signals: Vec<InsightSignal> = judgement_messages
            .iter()
            .map(|m| signal_from_message(m))
            .collect();
        if let Some(ref verdict) = query.verdict {
            if !signals.iter().any(|signal| &signal.verdict == verdict) {
                continue;
            }
        } else if query.off_scope
            && !signals.iter().any(|signal| {
                matches!(
                    signal.verdict,
                    JudgementVerdict::Warning | JudgementVerdict::OffTrack
                )
            })
        {
            continue;
        }

        let counts = count_signals(&signals);
        let severity = session_severity(&counts).to_string();
        let raw_scope = data
            .messages
            .iter()
            .filter(|m| m.type_ == MessageType::ScopeRequirements)
            .filter(|m| query.until.is_none_or(|end| m.ts <= end))
            .max_by_key(|m| m.ts)
            .and_then(|m| m.content.as_deref());
        let (scope, scope_quality) = clean_scope(raw_scope);
        let boundary = boundary_kind(raw_scope.unwrap_or(""));
        let mut drift_onsets_pct = Vec::new();
        let mut drift_onset_tools = Vec::new();
        let mut drift_window_archetypes = Vec::new();
        let mut drift_archetypes: Vec<&'static str> = Vec::new();
        for signal in signals.iter().filter(|s| {
            matches!(
                s.verdict,
                JudgementVerdict::Warning | JudgementVerdict::OffTrack
            )
        }) {
            let at = signal.to_count.unwrap_or(0);
            drift_onset_tools.push(at);
            if data.tool_call_count > 0 {
                drift_onsets_pct.push((at as f64 * 100.0 / data.tool_call_count as f64).min(100.0));
            }
            let labels = classify_archetypes(&format!("{} {}", signal.summary, signal.details));
            for label in &labels {
                if !drift_archetypes.contains(label) {
                    drift_archetypes.push(label);
                }
            }
            drift_window_archetypes.push(labels);
        }
        let mut ordered_messages: Vec<&SessionMessageWire> = data.messages.iter().collect();
        ordered_messages.sort_by_key(|m| m.ts);
        let (corrections, corrections_recovered, corrections_repeat) =
            recovery_walk(&ordered_messages);
        let latest_signal = signals.last().cloned();
        let scopey_usage = if data.analyzer_usage.is_empty() {
            None
        } else {
            let mut measured = ScopeyMeasured {
                calls: data.analyzer_usage.len(),
                input_tokens: 0,
                cached_input_tokens: 0,
                output_tokens: 0,
                total_tokens: 0,
            };
            for call in &data.analyzer_usage {
                measured.input_tokens += call.input_tokens;
                measured.cached_input_tokens += call.cached_input_tokens;
                measured.output_tokens += call.output_tokens;
                measured.total_tokens += call.total_tokens;
            }
            Some(measured)
        };
        let mut session = SessionInsight {
            session_id: data.session_id,
            harness: data.harness,
            cwd: data.cwd,
            created_at: data.created_at.to_rfc3339(),
            updated_at: data.updated_at.to_rfc3339(),
            tool_call_count: data.tool_call_count,
            severity,
            counts,
            scope,
            scope_quality,
            boundary,
            drift_onsets_pct,
            drift_archetypes,
            corrections,
            corrections_recovered,
            corrections_repeat,
            tokens: None,
            scopey_usage,
            latest_signal,
            signals,
            drift_window_archetypes,
            drift_onset_tools,
            transcript_path: data.transcript_path.clone(),
        };
        if token_scope == TokenScope::All {
            fill_tokens(std::slice::from_mut(&mut session));
        }
        analyzed.push(session);
    }

    analyzed.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    let totals = total_counts(&analyzed);
    let patterns = patterns_enabled.then(|| build_patterns(&analyzed));
    analyzed.truncate(query.limit);
    if token_scope == TokenScope::Shown {
        fill_tokens(&mut analyzed);
    }
    let token_totals = match token_scope {
        TokenScope::Off => None,
        TokenScope::Shown => Some(token_totals(&analyzed, "shown sessions")),
        TokenScope::All => Some(token_totals(&analyzed, "all analyzed sessions")),
    };
    InsightReport {
        generated_at: Utc::now().to_rfc3339(),
        filters,
        totals,
        patterns,
        token_totals,
        sessions: analyzed,
        excluded_empty_sessions,
        skipped_files,
    }
}

fn signal_from_message(message: &SessionMessageWire) -> InsightSignal {
    let summary = message.summary.clone().unwrap_or_default();
    let details = message.details.clone().unwrap_or_default();
    let verdict = normalize_judgement_verdict(
        message.verdict.clone().unwrap_or(JudgementVerdict::Unknown),
        &summary,
        &details,
    );
    InsightSignal {
        timestamp: message.ts.to_rfc3339(),
        verdict,
        status: message.status.clone(),
        from_count: message.from_count,
        to_count: message.to_count,
        summary,
        details,
    }
}

fn clean_scope(scope: Option<&str>) -> (String, ScopeQuality) {
    let Some(scope) = scope.map(str::trim).filter(|scope| !scope.is_empty()) else {
        return (
            "(no scope requirements recorded)".into(),
            ScopeQuality::Missing,
        );
    };
    let lower = scope.to_ascii_lowercase();
    let looks_like_judge_json = (lower.starts_with('{') || lower.starts_with("```json"))
        && lower.contains("\"verdict\"")
        && lower.contains("\"summary\"");
    if looks_like_judge_json {
        return (
            "(invalid scope: judge output was captured instead of scope requirements)".into(),
            ScopeQuality::Contaminated,
        );
    }
    (clip_one_line(scope, 240), ScopeQuality::Ok)
}

fn count_signals(signals: &[InsightSignal]) -> InsightCounts {
    let mut counts = InsightCounts {
        judgements: signals.len(),
        ..InsightCounts::default()
    };
    for signal in signals {
        match signal.verdict {
            JudgementVerdict::OnTrack => counts.on_track += 1,
            JudgementVerdict::Warning => counts.warning += 1,
            JudgementVerdict::OffTrack => counts.off_track += 1,
            JudgementVerdict::InsufficientEvidence => counts.insufficient_evidence += 1,
            JudgementVerdict::Unknown => counts.unknown += 1,
        }
    }
    counts.evaluated = counts.on_track + counts.warning + counts.off_track;
    counts.attention_rate_pct = percentage(counts.warning + counts.off_track, counts.evaluated);
    counts
}

fn total_counts(sessions: &[SessionInsight]) -> InsightTotals {
    let mut totals = InsightTotals {
        sessions: sessions.len(),
        ..InsightTotals::default()
    };
    for session in sessions {
        if session.counts.evaluated > 0 {
            totals.sessions_evaluated += 1;
        }
        if session.counts.off_track > 0 {
            totals.sessions_off_track += 1;
        } else if session.counts.warning > 0 {
            totals.sessions_warning += 1;
        }
        if session.counts.judgements == 0 {
            totals.sessions_unevaluated += 1;
        }
        if session.scope_quality == ScopeQuality::Contaminated {
            totals.contaminated_scope_sessions += 1;
        }
        totals.counts.judgements += session.counts.judgements;
        totals.counts.evaluated += session.counts.evaluated;
        totals.counts.on_track += session.counts.on_track;
        totals.counts.warning += session.counts.warning;
        totals.counts.off_track += session.counts.off_track;
        totals.counts.insufficient_evidence += session.counts.insufficient_evidence;
        totals.counts.unknown += session.counts.unknown;
    }
    totals.counts.attention_rate_pct = percentage(
        totals.counts.warning + totals.counts.off_track,
        totals.counts.evaluated,
    );
    totals.evaluation_coverage_pct = percentage(totals.sessions_evaluated, totals.sessions);
    totals.judge_failure_rate_pct = percentage(totals.counts.unknown, totals.counts.judgements);
    totals
}

fn session_overlaps(
    session: &SessionData,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> bool {
    since.is_none_or(|start| session.updated_at >= start)
        && until.is_none_or(|end| session.created_at <= end)
}

fn in_range(
    timestamp: DateTime<Utc>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> bool {
    since.is_none_or(|start| timestamp >= start) && until.is_none_or(|end| timestamp <= end)
}

fn session_severity(counts: &InsightCounts) -> &'static str {
    if counts.off_track > 0 {
        "off_track"
    } else if counts.warning > 0 {
        "warning"
    } else if counts.on_track > 0 {
        "on_track"
    } else if counts.insufficient_evidence > 0 {
        "insufficient_evidence"
    } else if counts.unknown > 0 {
        "unknown"
    } else {
        "unevaluated"
    }
}

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "off_track" => 0,
        "warning" => 1,
        "on_track" => 2,
        "insufficient_evidence" => 3,
        "unknown" => 4,
        _ => 5,
    }
}

fn percentage(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        (numerator as f64 * 1000.0 / denominator as f64).round() / 10.0
    }
}

fn describe_filters(
    args: &InsightArgs,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(session) = &args.session {
        out.push(format!("session={session}"));
    }
    if let Some(date) = &args.date {
        out.push(format!("date={date}"));
    } else {
        if let Some(start) = since {
            out.push(format!("since={}", start.to_rfc3339()));
        }
        if let Some(end) = until {
            out.push(format!("until={}", end.to_rfc3339()));
        }
    }
    if let Some(harness) = &args.harness {
        out.push(format!("harness={harness}"));
    }
    if let Some(verdict) = &args.verdict {
        out.push(format!("verdict={verdict}"));
    }
    if args.off_scope {
        out.push("verdict=warning|off-track".into());
    }
    if args.include_empty {
        out.push("include-empty=true".into());
    }
    out
}

fn humanize(value: u64) -> String {
    match value {
        v if v >= 1_000_000_000 => format!("{:.2}B", v as f64 / 1e9),
        v if v >= 1_000_000 => format!("{:.1}M", v as f64 / 1e6),
        v if v >= 10_000 => format!("{:.0}k", v as f64 / 1e3),
        v if v >= 1_000 => format!("{:.1}k", v as f64 / 1e3),
        v => v.to_string(),
    }
}

/// One proportional line of colored blocks: on-track / warning / off-track.
fn verdict_mix_line(caps: Caps, counts: &InsightCounts, width: usize) -> String {
    let evaluated = counts.evaluated.max(1);
    let parts = [
        (counts.on_track, term_viz::STATUS_GOOD, "on-track"),
        (counts.warning, term_viz::STATUS_WARNING, "warning"),
        (counts.off_track, term_viz::STATUS_SERIOUS, "off-track"),
    ];
    let mut bar = String::new();
    let mut legend = Vec::new();
    for (count, color, label) in parts {
        let cells = ((count as f64 / evaluated as f64) * width as f64).round() as usize;
        if count > 0 {
            bar.push_str(&term_viz::fg(caps, color, &"█".repeat(cells.max(1))));
        }
        legend.push(format!(
            "{} {label} {count}",
            term_viz::fg(caps, color, "■")
        ));
    }
    format!("  {bar}  {}", legend.join(&term_viz::dim(caps, " · ")))
}

fn print_patterns(caps: Caps, patterns: &DriftPatterns, width: usize) {
    if patterns.drift_windows == 0 {
        println!("\ndrift patterns: no drift-class windows in the analyzed sessions");
        return;
    }
    println!(
        "\n{}",
        term_viz::section(
            caps,
            "drift patterns",
            &format!(
                "{} flagged checks · {} sessions · {} broke a stated limit / {} an implied one",
                patterns.drift_windows,
                patterns.drift_sessions,
                patterns.boundary_explicit,
                patterns.boundary_implicit
            ),
            width,
        )
    );
    let max = patterns
        .archetype_counts
        .first()
        .map(|(_, c)| *c)
        .unwrap_or(1) as f64;
    // Categorical hues are a fixed-order budget: rows beyond the named slots
    // (and any "other" already present) fold into one neutral "other" row
    // rather than reusing or inventing hues.
    const SLOTS: usize = 7;
    let mut named: Vec<(&str, usize)> = Vec::new();
    let mut folded = 0usize;
    for (name, count) in &patterns.archetype_counts {
        if *name != "other" && named.len() < SLOTS {
            named.push((name, *count));
        } else {
            folded += count;
        }
    }
    let mut rows: Vec<term_viz::BarRow> = named
        .iter()
        .enumerate()
        .map(|(index, (name, count))| term_viz::BarRow {
            label: name.to_string(),
            value: count.to_string(),
            frac: *count as f64 / max,
            color: term_viz::CATEGORICAL[index],
        })
        .collect();
    if folded > 0 {
        rows.push(term_viz::BarRow {
            label: "other".into(),
            value: folded.to_string(),
            frac: folded as f64 / max,
            color: term_viz::NEUTRAL,
        });
    }
    print!("{}", term_viz::render_bar_rows(caps, &rows, 32));
    // Onset over relative position: one hue; kitty raster when supported,
    // sparkline otherwise. Labels stay in terminal text.
    let [p25, p50, p75] = patterns.onset_tool_quartiles;
    if caps.kitty {
        print!(
            "{}",
            term_viz::histogram_chart(&patterns.onset_histogram, 560, 64).kitty_escape()
        );
    } else {
        let values: Vec<f64> = patterns.onset_histogram.iter().map(|c| *c as f64).collect();
        println!(
            "  onset    {}",
            term_viz::fg(caps, term_viz::SEQUENTIAL, &term_viz::sparkline(&values))
        );
    }
    println!(
        "  {}",
        term_viz::dim(
            caps,
            &format!(
                "when flags happen: session start ── end · half of all flags by tool {p50} (25% by {p25}, 75% by {p75})"
            )
        )
    );
    println!();
    println!(
        "  {}",
        term_viz::dim(
            caps,
            &format!(
                "{:<12} {:>8} {:>10} {:>14}  {:>9}",
                "length", "sessions", "with flags", "flagged checks", "flag rate"
            )
        )
    );
    for row in &patterns.by_length {
        let rate = if row.judged > 0 {
            row.drift_windows as f64 / row.judged as f64
        } else {
            0.0
        };
        println!(
            "  {:<12} {:>8} {:>10} {:>14}  {:>8.1}%  {}",
            row.label,
            row.sessions,
            row.drifted,
            row.drift_windows,
            rate * 100.0,
            term_viz::fg(
                caps,
                term_viz::SEQUENTIAL,
                &term_viz::unicode_bar(rate * 5.0, 10)
            ),
        );
    }
    if patterns.corrections > 0 {
        let followed = patterns.corrections_followed.max(1);
        let rate = patterns.corrections_recovered as f64 / followed as f64;
        println!(
            "  course corrections: {} sent · back on-track by the next check {}/{} ({:.0}%) {}",
            patterns.corrections,
            patterns.corrections_recovered,
            patterns.corrections_followed,
            rate * 100.0,
            term_viz::fg(
                caps,
                term_viz::STATUS_GOOD,
                &term_viz::unicode_bar(rate, 10)
            ),
        );
    }
}

fn print_token_totals(caps: Caps, totals: &TokenTotals, width: usize) {
    if totals.sessions_counted == 0 {
        return;
    }
    let fresh = totals.input.saturating_sub(totals.cached);
    println!(
        "{}",
        term_viz::section(
            caps,
            "tokens",
            &format!(
                "{} across {} transcripts ({})",
                humanize(totals.total),
                totals.sessions_counted,
                totals.scope
            ),
            width,
        )
    );
    let parts = [
        (
            totals.cached as f64,
            term_viz::NEUTRAL,
            "cache reads",
            totals.cached,
        ),
        (fresh as f64, term_viz::CATEGORICAL[0], "fresh input", fresh),
        (
            totals.output as f64,
            term_viz::CATEGORICAL[1],
            "output",
            totals.output,
        ),
    ];
    let mut bar = String::new();
    let mut legend = Vec::new();
    let total = totals.total.max(1) as f64;
    for (value, color, label, raw) in parts {
        let cells = ((value / total) * 36.0).round() as usize;
        if value > 0.0 {
            bar.push_str(&term_viz::fg(caps, color, &"█".repeat(cells.max(1))));
        }
        legend.push(format!(
            "{} {label} {}",
            term_viz::fg(caps, color, "■"),
            humanize(raw)
        ));
    }
    println!("  {bar}  {}", legend.join(&term_viz::dim(caps, " · ")));
}

fn print_session_extras(caps: Caps, session: &SessionInsight, width: usize) {
    if let Some(tokens) = &session.tokens {
        println!(
            "  tokens: {} ({} cache reads · {} fresh input · {} output)",
            humanize(tokens.total),
            humanize(tokens.cached),
            humanize(tokens.input.saturating_sub(tokens.cached)),
            humanize(tokens.output),
        );
    }
    if let Some(measured) = &session.scopey_usage {
        let mut ratios = String::new();
        if let Some(tokens) = &session.tokens {
            if tokens.total > 0 {
                let volume = measured.total_tokens as f64 * 100.0 / tokens.total as f64;
                ratios.push_str(&format!(" · {volume:.1}% of main volume"));
                let full_price = tokens.input.saturating_sub(tokens.cached) + tokens.output;
                if full_price > 0 {
                    ratios.push_str(&format!(
                        " · {:.0}% of full-price tokens",
                        measured.total_tokens as f64 * 100.0 / full_price as f64
                    ));
                }
            }
        }
        println!(
            "  scopey overhead: {} measured across {} calls{}",
            humanize(measured.total_tokens),
            measured.calls,
            ratios,
        );
    }
    if !session.drift_onsets_pct.is_empty() {
        let onsets = session
            .drift_onsets_pct
            .iter()
            .map(|p| format!("{p:.0}%"))
            .collect::<Vec<_>>()
            .join(" ");
        let line = format!(
            "flags: at {onsets} through the session · work involved: {} · broke {} limit",
            session.drift_archetypes.join(", "),
            if session.boundary == "explicit" {
                "a stated"
            } else {
                "an implied"
            },
        );
        println!("{}", term_viz::wrap_indent(&line, width, "  ", "      "));
    }
    if session.corrections > 0 {
        let recovered = term_viz::fg(
            caps,
            term_viz::STATUS_GOOD,
            &format!("{} back on-track next check", session.corrections_recovered),
        );
        let repeat = term_viz::fg(
            caps,
            term_viz::STATUS_SERIOUS,
            &format!("{} drifted again", session.corrections_repeat),
        );
        println!(
            "  course corrections: {} sent · {recovered} · {repeat}",
            session.corrections
        );
    }
}

fn print_human(report: &InsightReport, details: bool, work_root: &std::path::Path, caps: Caps) {
    if report.totals.sessions == 0 {
        println!("No sessions matched under {}.", work_root.display());
        if !report.filters.is_empty() {
            println!("filters: {}", report.filters.join(", "));
        }
        if report.excluded_empty_sessions > 0 {
            println!(
                "{} zero-tool session store(s) excluded; use --include-empty to inspect them.",
                report.excluded_empty_sessions
            );
        }
        return;
    }
    println!("Scopey insights");
    if !report.filters.is_empty() {
        println!("filters: {}", report.filters.join(", "));
    }
    let session_label = if report.excluded_empty_sessions > 0 {
        "active sessions"
    } else {
        "sessions"
    };
    println!(
        "{} {} · {} checks · {} off-track · {} warning · {} on-track",
        report.totals.sessions,
        session_label,
        report.totals.judgements,
        report.totals.off_track,
        report.totals.warning,
        report.totals.on_track
    );
    if report.totals.evaluated > 0 {
        println!("{}", verdict_mix_line(caps, &report.totals.counts, 40));
    }
    println!(
        "flagged: {:.1}% of completed checks · {} sessions off-track · {} warning-only · {} never checked",
        report.totals.attention_rate_pct,
        report.totals.sessions_off_track,
        report.totals.sessions_warning,
        report.totals.sessions_unevaluated
    );
    println!(
        "coverage: {}/{} sessions had at least one completed check ({:.1}%) · {}/{} checks failed to run ({:.1}%)",
        report.totals.sessions_evaluated,
        report.totals.sessions,
        report.totals.evaluation_coverage_pct,
        report.totals.unknown,
        report.totals.judgements,
        report.totals.judge_failure_rate_pct
    );
    if report.excluded_empty_sessions > 0 {
        println!(
            "data hygiene: {} zero-tool session store(s) excluded (use --include-empty to inspect)",
            report.excluded_empty_sessions
        );
    }
    if report.totals.contaminated_scope_sessions > 0 {
        println!(
            "data hygiene: {} active session(s) have contaminated scope records",
            report.totals.contaminated_scope_sessions
        );
    }
    if report.totals.insufficient_evidence > 0 || report.totals.unknown > 0 {
        println!(
            "gaps: {} check(s) had no tool evidence · {} failed to run",
            report.totals.insufficient_evidence, report.totals.unknown
        );
    }
    let width = term_viz::terminal_width().clamp(60, 160);
    if let Some(patterns) = &report.patterns {
        print_patterns(caps, patterns, width);
    }
    if let Some(totals) = &report.token_totals {
        println!();
        print_token_totals(caps, totals, width);
    }

    for session in &report.sessions {
        let severity_color = match session.severity.as_str() {
            "off_track" => term_viz::STATUS_SERIOUS,
            "warning" => term_viz::STATUS_WARNING,
            "on_track" => term_viz::STATUS_GOOD,
            _ => term_viz::NEUTRAL,
        };
        println!();
        println!(
            "{} {}  {}  {}",
            term_viz::fg(
                caps,
                severity_color,
                &term_viz::bold(caps, severity_label(&session.severity))
            ),
            term_viz::bold(caps, &session.session_id),
            empty_as(&session.harness, "unknown-harness"),
            term_viz::dim(
                caps,
                &format!("updated {}", display_time(&session.updated_at))
            )
        );
        println!(
            "{}",
            term_viz::dim(caps, &format!("  cwd: {}", session.cwd))
        );
        println!(
            "  checks: {} · {} off-track · {} warning · {} on-track · {:.1}% flagged",
            session.counts.judgements,
            session.counts.off_track,
            session.counts.warning,
            session.counts.on_track,
            session.counts.attention_rate_pct
        );
        print_session_extras(caps, session, width);
        println!(
            "{}",
            term_viz::dim(
                caps,
                &term_viz::wrap_indent(&format!("scope: {}", session.scope), width, "  ", "      ")
            )
        );
        if session.scope_quality == ScopeQuality::Contaminated {
            println!("  data quality: scope record contains judge output");
        }
        if details {
            if session.signals.is_empty() {
                println!("  signals: none");
            }
            for signal in session.signals.iter().rev() {
                print_signal(caps, signal, "  ", width);
            }
        } else if let Some(signal) =
            latest_concerning(&session.signals).or(session.latest_signal.as_ref())
        {
            print_signal(caps, signal, "  latest: ", width);
        }
    }
    if report.sessions.len() < report.totals.sessions {
        println!(
            "\nshowing {} of {} matching sessions (raise --limit to see more)",
            report.sessions.len(),
            report.totals.sessions
        );
    }
    if report.skipped_files > 0 {
        println!(
            "\nwarning: skipped {} unreadable session file(s); set SCOPEY_DEBUG=1 for paths",
            report.skipped_files
        );
    }
}

fn latest_concerning(signals: &[InsightSignal]) -> Option<&InsightSignal> {
    signals.iter().rev().find(|signal| {
        matches!(
            signal.verdict,
            JudgementVerdict::Warning | JudgementVerdict::OffTrack
        )
    })
}

fn print_signal(caps: Caps, signal: &InsightSignal, prefix: &str, width: usize) {
    let window = match (signal.from_count, signal.to_count) {
        (Some(from), Some(to)) => format!(" tools {from}–{to}"),
        _ => String::new(),
    };
    let summary = clip_one_line(&signal.summary, 220);
    let color = match signal.verdict {
        JudgementVerdict::OffTrack => term_viz::STATUS_SERIOUS,
        JudgementVerdict::Warning => term_viz::STATUS_WARNING,
        JudgementVerdict::OnTrack => term_viz::STATUS_GOOD,
        _ => term_viz::NEUTRAL,
    };
    let line = format!(
        "{}{} at {} — {}",
        term_viz::fg(caps, color, verdict_label(&signal.verdict)),
        window,
        display_time(&signal.timestamp),
        empty_as(&summary, "(no summary)")
    );
    println!("{}", term_viz::wrap_indent(&line, width, prefix, "      "));
    if !signal.details.trim().is_empty() {
        let why = format!("why: {}", clip_one_line(&signal.details, 320));
        println!(
            "{}",
            term_viz::dim(caps, &term_viz::wrap_indent(&why, width, "    ", "      "))
        );
    }
}

fn severity_label(severity: &str) -> &'static str {
    match severity {
        "off_track" => "!! OFF TRACK",
        "warning" => "!  WARNING",
        "on_track" => "OK ON TRACK",
        "insufficient_evidence" => "?  NO EVIDENCE",
        "unknown" => "?  UNKNOWN",
        _ => "·  UNEVALUATED",
    }
}

fn verdict_label(verdict: &JudgementVerdict) -> &'static str {
    match verdict {
        JudgementVerdict::OnTrack => "on-track",
        JudgementVerdict::Warning => "warning",
        JudgementVerdict::OffTrack => "off-track",
        JudgementVerdict::InsufficientEvidence => "insufficient-evidence",
        JudgementVerdict::Unknown => "unknown",
    }
}

fn display_time(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| {
            dt.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M %Z")
                .to_string()
        })
        .unwrap_or_else(|_| value.to_string())
}

fn clip_one_line(value: &str, max: usize) -> String {
    let one_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let mut out: String = one_line.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn empty_as<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionMessage, SessionMessageWire};

    #[test]
    fn archetype_classification_matches_eval_taxonomy() {
        let labels = classify_archetypes(
            "The agent pivoted from research to implementing and committing an evaluation suite",
        );
        assert!(labels.contains(&"analysis→implementation"));
        assert!(labels.contains(&"vcs/release"));
        assert_eq!(
            classify_archetypes("something unclassifiable"),
            vec!["other"]
        );
    }

    #[test]
    fn boundary_markers_split_explicit_from_implicit() {
        assert_eq!(
            boundary_kind("- Analysis only: do not edit files"),
            "explicit"
        );
        assert_eq!(boundary_kind("- Ship the new slug helper"), "implicit");
    }

    #[test]
    fn recovery_walk_matches_eval_semantics() {
        let messages: Vec<SessionMessageWire> = vec![
            SessionMessage::injection("correction", "back on scope", 30).into(),
            SessionMessage::judgement(
                30,
                45,
                JudgementVerdict::OnTrack,
                JudgementStatus::Ready,
                "ok",
                "",
            )
            .into(),
            SessionMessage::injection("correction-repeat", "again", 60).into(),
            SessionMessage::judgement(
                60,
                75,
                JudgementVerdict::OffTrack,
                JudgementStatus::Ready,
                "bad",
                "",
            )
            .into(),
        ];
        let ordered: Vec<&SessionMessageWire> = messages.iter().collect();
        let (corrections, recovered, repeat) = recovery_walk(&ordered);
        assert_eq!((corrections, recovered, repeat), (2, 1, 1));
    }

    #[test]
    fn length_buckets_and_humanize_render_stable_labels() {
        assert_eq!(length_bucket(10), "<25 tools");
        assert_eq!(length_bucket(146), "100–249");
        assert_eq!(length_bucket(906), "500+");
        assert_eq!(humanize(906), "906");
        assert_eq!(humanize(51_974), "52k");
        assert_eq!(humanize(142_203_597), "142.2M");
        assert_eq!(humanize(1_414_000_000), "1.41B");
    }

    fn session(id: &str, verdicts: &[JudgementVerdict]) -> SessionData {
        let now = Utc::now();
        let mut messages: Vec<SessionMessageWire> =
            vec![SessionMessage::scope_requirements("- ship the requested CLI", None).into()];
        for (index, verdict) in verdicts.iter().enumerate() {
            messages.push(
                SessionMessage::judgement(
                    index as u64 * 10,
                    index as u64 * 10 + 10,
                    verdict.clone(),
                    JudgementStatus::Injected,
                    format!("signal {index}"),
                    "useful context",
                )
                .into(),
            );
        }
        SessionData {
            session_id: id.into(),
            cwd: "/tmp/project".into(),
            harness: "codex".into(),
            created_at: now,
            updated_at: now,
            tool_call_count: 30,
            last_judged_to_count: 30,
            last_reminder_at_count: 0,
            last_injection_at_count: 0,
            scope_epoch_start_tool_count: 0,
            transcript_path: None,
            messages,
            pending_judgement_id: None,
            tool_events: vec![],
            summarize_pending: false,
            pending_judge: None,
            analyzer_usage: vec![],
        }
    }

    fn query() -> InsightQuery {
        InsightQuery {
            session: None,
            since: None,
            until: None,
            harness: None,
            verdict: None,
            off_scope: false,
            include_empty: false,
            limit: 20,
        }
    }

    #[test]
    fn ranks_off_track_first_and_counts_attention() {
        let report = analyze_sessions(
            vec![
                session("ok", &[JudgementVerdict::OnTrack]),
                session(
                    "drift",
                    &[
                        JudgementVerdict::OnTrack,
                        JudgementVerdict::Warning,
                        JudgementVerdict::OffTrack,
                    ],
                ),
            ],
            &query(),
            vec![],
            0,
            true,
            TokenScope::Off,
        );
        assert_eq!(report.sessions[0].session_id, "drift");
        assert_eq!(report.totals.sessions_off_track, 1);
        assert_eq!(report.totals.judgements, 4);
        assert_eq!(report.totals.attention_rate_pct, 50.0);
    }

    #[test]
    fn off_scope_filter_excludes_clean_sessions_and_windows() {
        let mut q = query();
        q.off_scope = true;
        let report = analyze_sessions(
            vec![
                session("ok", &[JudgementVerdict::OnTrack]),
                session(
                    "warn",
                    &[JudgementVerdict::OnTrack, JudgementVerdict::Warning],
                ),
            ],
            &q,
            vec![],
            0,
            true,
            TokenScope::Off,
        );
        assert_eq!(report.sessions.len(), 1);
        assert_eq!(report.sessions[0].session_id, "warn");
        assert_eq!(report.sessions[0].counts.judgements, 2);
        assert_eq!(report.sessions[0].counts.warning, 1);
        assert_eq!(report.sessions[0].counts.on_track, 1);
        assert_eq!(report.sessions[0].counts.attention_rate_pct, 50.0);
    }

    #[test]
    fn parses_date_and_verdict_aliases() {
        let (start, end) = parse_date_filters(Some("2026-07-30"), None, None).unwrap();
        assert!(start < end);
        assert_eq!(
            parse_verdict("off_track").unwrap(),
            JudgementVerdict::OffTrack
        );
        assert_eq!(
            parse_verdict("insufficient").unwrap(),
            JudgementVerdict::InsufficientEvidence
        );
    }

    #[test]
    fn session_prefix_must_be_unique() {
        let mut sessions = vec![session("abc-1", &[]), session("abc-2", &[])];
        assert!(resolve_session_selector(&mut sessions, Some("abc")).is_err());
        resolve_session_selector(&mut sessions, Some("abc-1")).unwrap();
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn excludes_empty_sessions_by_default_but_can_include_them() {
        let mut ghost = session("ghost", &[]);
        ghost.tool_call_count = 0;
        let active = session("active", &[JudgementVerdict::OnTrack]);

        let report = analyze_sessions(
            vec![ghost.clone(), active.clone()],
            &query(),
            vec![],
            0,
            true,
            TokenScope::Off,
        );
        assert_eq!(report.totals.sessions, 1);
        assert_eq!(report.excluded_empty_sessions, 1);
        assert_eq!(report.sessions[0].session_id, "active");

        let mut include = query();
        include.include_empty = true;
        let report = analyze_sessions(
            vec![ghost, active],
            &include,
            vec![],
            0,
            true,
            TokenScope::Off,
        );
        assert_eq!(report.totals.sessions, 2);
        assert_eq!(report.excluded_empty_sessions, 0);
    }

    #[test]
    fn missing_evidence_is_not_counted_as_scope_drift() {
        let mut data = session("missing", &[]);
        data.messages.push(
            SessionMessage::judgement(
                0,
                10,
                JudgementVerdict::OffTrack,
                JudgementStatus::Injected,
                "Cannot audit agent scope without transcript data",
                "The transcript file is missing or empty; no actions to evaluate.",
            )
            .into(),
        );
        let report = analyze_sessions(vec![data], &query(), vec![], 0, true, TokenScope::Off);
        assert_eq!(report.totals.off_track, 0);
        assert_eq!(report.totals.insufficient_evidence, 1);
        assert_eq!(report.totals.attention_rate_pct, 0.0);
    }

    #[test]
    fn flags_judge_json_captured_as_scope() {
        let mut data = session("contaminated", &[JudgementVerdict::OnTrack]);
        data.messages.push(
            SessionMessage::scope_requirements(
                r#"```json {"verdict":"warning","summary":"not scope"} ```"#,
                None,
            )
            .into(),
        );
        let report = analyze_sessions(vec![data], &query(), vec![], 0, true, TokenScope::Off);
        assert_eq!(report.totals.contaminated_scope_sessions, 1);
        assert_eq!(report.sessions[0].scope_quality, ScopeQuality::Contaminated);
        assert!(report.sessions[0].scope.starts_with("(invalid scope:"));
    }
}

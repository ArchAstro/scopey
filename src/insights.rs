use crate::config::Config;
use crate::session::{
    JudgementStatus, JudgementVerdict, MessageType, SessionData, SessionMessageWire, SessionStore,
};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Days, Local, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use std::fs;
use std::path::PathBuf;

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
    pub limit: usize,
    pub details: bool,
    pub json: bool,
}

#[derive(Debug, Clone)]
struct InsightQuery {
    session: Option<String>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    harness: Option<String>,
    verdict: Option<JudgementVerdict>,
    off_scope: bool,
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
    sessions_off_track: usize,
    sessions_warning: usize,
    sessions_unevaluated: usize,
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
    latest_signal: Option<InsightSignal>,
    signals: Vec<InsightSignal>,
}

#[derive(Debug, Serialize)]
pub struct InsightReport {
    generated_at: String,
    filters: Vec<String>,
    totals: InsightTotals,
    sessions: Vec<SessionInsight>,
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
    let report = analyze_sessions(sessions, &query, filters, skipped_files);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report, args.details, &cfg.work_root);
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

fn analyze_sessions(
    sessions: Vec<SessionData>,
    query: &InsightQuery,
    filters: Vec<String>,
    skipped_files: usize,
) -> InsightReport {
    let mut analyzed = Vec::new();
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

        let mut judgement_messages: Vec<&SessionMessageWire> = data
            .messages
            .iter()
            .filter(|m| m.type_ == MessageType::Judgement)
            .filter(|m| m.status != Some(JudgementStatus::Pending))
            .filter(|m| in_range(m.ts, query.since, query.until))
            .collect();
        judgement_messages.sort_by_key(|m| m.ts);

        if let Some(ref verdict) = query.verdict {
            if !judgement_messages
                .iter()
                .any(|m| m.verdict.as_ref() == Some(verdict))
            {
                continue;
            }
        } else if query.off_scope
            && !judgement_messages.iter().any(|m| {
                matches!(
                    m.verdict,
                    Some(JudgementVerdict::Warning) | Some(JudgementVerdict::OffTrack)
                )
            })
        {
            continue;
        }

        let signals: Vec<InsightSignal> = judgement_messages
            .iter()
            .map(|m| signal_from_message(m))
            .collect();
        let counts = count_signals(&signals);
        let severity = session_severity(&counts).to_string();
        let scope = data
            .messages
            .iter()
            .filter(|m| m.type_ == MessageType::ScopeRequirements)
            .filter(|m| query.until.is_none_or(|end| m.ts <= end))
            .max_by_key(|m| m.ts)
            .and_then(|m| m.content.as_deref())
            .map(|s| clip_one_line(s, 240))
            .unwrap_or_else(|| "(no scope requirements recorded)".into());
        let latest_signal = signals.last().cloned();
        analyzed.push(SessionInsight {
            session_id: data.session_id,
            harness: data.harness,
            cwd: data.cwd,
            created_at: data.created_at.to_rfc3339(),
            updated_at: data.updated_at.to_rfc3339(),
            tool_call_count: data.tool_call_count,
            severity,
            counts,
            scope,
            latest_signal,
            signals,
        });
    }

    analyzed.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    let totals = total_counts(&analyzed);
    analyzed.truncate(query.limit);
    InsightReport {
        generated_at: Utc::now().to_rfc3339(),
        filters,
        totals,
        sessions: analyzed,
        skipped_files,
    }
}

fn signal_from_message(message: &SessionMessageWire) -> InsightSignal {
    InsightSignal {
        timestamp: message.ts.to_rfc3339(),
        verdict: message.verdict.clone().unwrap_or(JudgementVerdict::Unknown),
        status: message.status.clone(),
        from_count: message.from_count,
        to_count: message.to_count,
        summary: message.summary.clone().unwrap_or_default(),
        details: message.details.clone().unwrap_or_default(),
    }
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
        if session.counts.off_track > 0 {
            totals.sessions_off_track += 1;
        } else if session.counts.warning > 0 {
            totals.sessions_warning += 1;
        }
        if session.counts.judgements == 0 {
            totals.sessions_unevaluated += 1;
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
    out
}

fn print_human(report: &InsightReport, details: bool, work_root: &std::path::Path) {
    if report.totals.sessions == 0 {
        println!("No sessions matched under {}.", work_root.display());
        if !report.filters.is_empty() {
            println!("filters: {}", report.filters.join(", "));
        }
        return;
    }
    println!("Scopey insights");
    if !report.filters.is_empty() {
        println!("filters: {}", report.filters.join(", "));
    }
    println!(
        "{} sessions · {} judged windows · {} off-track · {} warning · {} on-track",
        report.totals.sessions,
        report.totals.judgements,
        report.totals.off_track,
        report.totals.warning,
        report.totals.on_track
    );
    println!(
        "attention rate: {:.1}% of evaluated windows · {} sessions off-track · {} warning-only · {} unevaluated",
        report.totals.attention_rate_pct,
        report.totals.sessions_off_track,
        report.totals.sessions_warning,
        report.totals.sessions_unevaluated
    );
    if report.totals.insufficient_evidence > 0 || report.totals.unknown > 0 {
        println!(
            "coverage gaps: {} insufficient-evidence · {} unknown",
            report.totals.insufficient_evidence, report.totals.unknown
        );
    }

    for session in &report.sessions {
        println!();
        println!(
            "{} {}  {}  updated {}",
            severity_label(&session.severity),
            session.session_id,
            empty_as(&session.harness, "unknown-harness"),
            display_time(&session.updated_at)
        );
        println!("  cwd: {}", session.cwd);
        println!(
            "  windows: {} judged · {} off-track · {} warning · {} on-track · {:.1}% attention",
            session.counts.judgements,
            session.counts.off_track,
            session.counts.warning,
            session.counts.on_track,
            session.counts.attention_rate_pct
        );
        println!("  scope: {}", session.scope);
        if details {
            if session.signals.is_empty() {
                println!("  signals: none");
            }
            for signal in session.signals.iter().rev() {
                print_signal(signal, "  ");
            }
        } else if let Some(signal) =
            latest_concerning(&session.signals).or(session.latest_signal.as_ref())
        {
            print_signal(signal, "  latest: ");
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

fn print_signal(signal: &InsightSignal, prefix: &str) {
    let window = match (signal.from_count, signal.to_count) {
        (Some(from), Some(to)) => format!(" tools {from}–{to}"),
        _ => String::new(),
    };
    let summary = clip_one_line(&signal.summary, 220);
    println!(
        "{prefix}{}{} at {} — {}",
        verdict_label(&signal.verdict),
        window,
        display_time(&signal.timestamp),
        empty_as(&summary, "(no summary)")
    );
    if !signal.details.trim().is_empty() {
        println!("    why: {}", clip_one_line(&signal.details, 320));
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
            transcript_path: None,
            messages,
            pending_judgement_id: None,
            tool_events: vec![],
            summarize_pending: false,
            pending_judge: None,
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
}

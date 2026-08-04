#!/usr/bin/env python3
"""Analyze real Scopey sessions in a fixed time window.

The script joins Scopey's state/event records to provider-reported main-session
usage. Claude analyzer usage is provider-reported when its print-mode transcript
can be matched to a Scopey job. Codex analyzer calls are historical estimates:
Scopey used ``codex exec --ephemeral`` and therefore left no provider transcript.
Their input cost is calibrated from the checked 2026-08-04 benchmark; generated
tokens are estimated from the persisted response size. No message content, cwd,
or transcript path is emitted.
"""

from __future__ import annotations

import argparse
from collections import Counter
from dataclasses import asdict
from datetime import datetime, timedelta, timezone
import hashlib
import json
import math
from pathlib import Path
import re
import statistics
import sys
from typing import Any, Iterable

EVAL_ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(EVAL_ROOT))

from transcript_usage import Usage, claude_snapshot, codex_snapshot  # noqa: E402

SCHEMA_VERSION = 1
DEFAULT_WINDOW_HOURS = 48.0
DEFAULT_EXCLUDES = ("/eval/results/", "/private/var/folders/", "/var/folders/")
CALIBRATION = {
    "id": "benchmark-20260804T170422Z",
    "model": "gpt-5.6-terra",
    "source": "eval/calibration/2026-08-04-terra-low-analyzer.json",
    "sample_calls_per_kind": 55,
    "input_tokens": {"summarize": 14198.673, "judge": 14098.600},
    "input_min": {"summarize": 14039, "judge": 13894},
    "input_max": {"summarize": 14277, "judge": 14141},
}
MODEL_RE = re.compile(r"scopey model: runner=([^ ]+).*; model=([^ ]+)")


def parse_time(value: str) -> datetime:
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed.astimezone(timezone.utc)


def timestamp(record: dict[str, Any]) -> datetime | None:
    value = record.get("ts") or record.get("timestamp")
    if not isinstance(value, str):
        return None
    try:
        return parse_time(value)
    except ValueError:
        return None


def json_lines(path: Path) -> list[dict[str, Any]]:
    values: list[dict[str, Any]] = []
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return values
    for line in lines:
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            values.append(value)
    return values


def in_window(value: datetime | None, since: datetime, until: datetime) -> bool:
    return value is not None and since <= value < until


def hash_id(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()[:12]


def display_path(path: Path) -> str:
    try:
        return "~/" + str(path.relative_to(Path.home()))
    except ValueError:
        return str(path)


def normalize_verdict(value: Any) -> str:
    compact = str(value or "unknown").replace("_", "").casefold()
    return {
        "offtrack": "off_track",
        "ontrack": "on_track",
        "insufficientevidence": "insufficient_evidence",
    }.get(compact, compact)


def message_text(record: dict[str, Any]) -> str:
    message = record.get("message")
    if not isinstance(message, dict):
        return ""
    content = message.get("content", "")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return " ".join(
            str(item.get("text", "")) for item in content if isinstance(item, dict)
        )
    return ""


def records_through(values: Iterable[dict[str, Any]], bound: datetime) -> list[dict[str, Any]]:
    return [record for record in values if (timestamp(record) or datetime.min.replace(tzinfo=timezone.utc)) < bound]


def main_usage_in_window(
    transcript: Path, harness: str, since: datetime, until: datetime
) -> Usage:
    values = json_lines(transcript)
    if harness == "codex":
        end = codex_snapshot(records_through(values, until))
        start = codex_snapshot(records_through(values, since))
        return end.minus(start)
    if harness == "claude":
        return claude_snapshot(
            record for record in values if in_window(timestamp(record), since, until)
        )
    raise ValueError(f"unsupported harness {harness!r}")


def model_choice(log_root: Path, session_id: str, kind: str) -> tuple[str, str] | None:
    path = log_root / f"{kind}-{session_id}.log"
    if not path.is_file():
        return None
    choices = MODEL_RE.findall(path.read_text(encoding="utf-8", errors="replace"))
    if not choices:
        return None
    return Counter(choices).most_common(1)[0][0]


def output_chars(messages: list[dict[str, Any]], kind: str, since: datetime, until: datetime) -> list[int]:
    wanted = "scope_requirements" if kind == "summarize" else "judgement"
    selected = [
        item for item in messages
        if item.get("type") == wanted and in_window(timestamp(item), since, until)
    ]
    selected.sort(key=lambda item: timestamp(item) or since)
    if kind == "summarize":
        return [len(str(item.get("content", ""))) + 40 for item in selected]
    return [
        len(json.dumps({
            "verdict": item.get("verdict", ""),
            "summary": item.get("summary", ""),
            "details": item.get("details", ""),
        }, separators=(",", ":")))
        for item in selected if item.get("status", "ready") == "ready"
    ]


def estimate_codex_job(kind: str, chars: int, model: str) -> dict[str, Any]:
    calibrated = model == CALIBRATION["model"] and kind in CALIBRATION["input_tokens"]
    generated = math.ceil(chars / 4)
    if calibrated:
        input_tokens = round(CALIBRATION["input_tokens"][kind])
        low = CALIBRATION["input_min"][kind] + math.ceil(chars / 5)
        high = CALIBRATION["input_max"][kind] + math.ceil(chars / 3)
        method = "benchmark-calibrated-input+chars/4-output"
    else:
        # Explicit fallback for an uncalibrated model. The wide range is retained
        # in the result so this cannot masquerade as provider-reported usage.
        input_tokens = math.ceil(chars / 4)
        low = math.ceil(chars / 5)
        high = math.ceil(chars / 3)
        method = "chars/4-uncalibrated"
    return {
        "kind": kind,
        "model": model,
        "input_tokens": input_tokens,
        "generated_tokens": generated,
        "total_tokens": input_tokens + generated,
        "low_tokens": low,
        "high_tokens": high,
        "source": "estimated",
        "method": method,
    }


def claude_analyzer_calls(root: Path, since: datetime, until: datetime) -> list[dict[str, Any]]:
    calls: list[dict[str, Any]] = []
    if not root.is_dir():
        return calls
    for path in sorted(root.rglob("*.jsonl")):
        values = json_lines(path)
        kind = ""
        for record in values:
            if record.get("type") != "user":
                continue
            text = message_text(record)
            if "You are a scope analyst for a coding agent session" in text:
                kind = "summarize"
                break
            if "You are a strict scope auditor for a coding agent" in text:
                kind = "judge"
                break
        if not kind:
            continue
        times = [value for value in map(timestamp, values) if value is not None]
        when = min(times) if times else None
        if not in_window(when, since - timedelta(minutes=5), until + timedelta(minutes=5)):
            continue
        usage = claude_snapshot(values)
        if usage.total_tokens == 0:
            continue
        cwd = next((str(record.get("cwd")) for record in values if record.get("cwd")), "")
        observed_models = [
            str(record["message"].get("model"))
            for record in values
            if record.get("type") == "assistant"
            and isinstance(record.get("message"), dict)
            and record["message"].get("model")
        ]
        model = next(
            (value for value in reversed(observed_models) if value != "<synthetic>"),
            observed_models[-1] if observed_models else "unknown",
        )
        calls.append({"path": path, "kind": kind, "when": when, "cwd": cwd, "model": model, "usage": usage})
    return calls


def match_claude_call(
    calls: list[dict[str, Any]], used: set[Path], kind: str, cwd: str,
    start: datetime, end: datetime,
) -> dict[str, Any] | None:
    candidates = [
        call for call in calls
        if call["path"] not in used and call["kind"] == kind
        and start - timedelta(seconds=10) <= call["when"] <= end + timedelta(seconds=30)
    ]
    if not candidates:
        return None
    chosen = min(
        candidates,
        key=lambda call: (
            call["cwd"] != cwd,
            abs((call["when"] - start).total_seconds()),
            str(call["path"]),
        ),
    )
    used.add(chosen["path"])
    usage: Usage = chosen["usage"]
    return {
        "kind": kind,
        "model": chosen["model"],
        "input_tokens": usage.input_tokens,
        "generated_tokens": usage.output_tokens,
        "total_tokens": usage.total_tokens,
        "low_tokens": usage.total_tokens,
        "high_tokens": usage.total_tokens,
        "source": "provider-reported",
        "method": "matched-claude-transcript",
    }


def estimate_unmatched_claude_job(
    calls: list[dict[str, Any]], kind: str, model: str
) -> dict[str, Any]:
    observed = [call["usage"] for call in calls if call["kind"] == kind]
    totals = [usage.total_tokens for usage in observed]
    inputs = [usage.input_tokens for usage in observed]
    outputs = [usage.output_tokens for usage in observed]
    if not totals:
        return {
            "kind": kind, "model": model, "input_tokens": 0,
            "generated_tokens": 0, "total_tokens": 0, "low_tokens": 0,
            "high_tokens": 0, "source": "unmeasured",
            "method": "no-matched-claude-calibration",
        }
    return {
        "kind": kind,
        "model": model,
        "input_tokens": round(statistics.median(inputs)),
        "generated_tokens": round(statistics.median(outputs)),
        "total_tokens": round(statistics.median(totals)),
        "low_tokens": 0,
        "high_tokens": max(totals),
        "source": "estimated",
        "method": "same-window-matched-claude-median",
    }


def jobs(events: list[dict[str, Any]], kind: str, since: datetime, until: datetime) -> list[tuple[datetime, datetime]]:
    starts = [
        timestamp(event) for event in events
        if event.get("event") == f"job.{kind}.start" and in_window(timestamp(event), since, until)
    ]
    pairs: list[tuple[datetime, datetime]] = []
    ordered_starts = [value for value in starts if value is not None]
    for index, start in enumerate(ordered_starts):
        next_start = ordered_starts[index + 1] if index + 1 < len(ordered_starts) else until
        # Claude print-mode transcript timestamps can lag Scopey's done event,
        # so use a bounded start-to-next-start matching window rather than done.
        end = min(start + timedelta(minutes=5), next_start, until)
        pairs.append((start, end))
    return pairs


def skipped_without_model(
    events: list[dict[str, Any]], kind: str, start: datetime, end: datetime
) -> bool:
    if kind != "judge":
        return False
    for event in events:
        when = timestamp(event)
        if when is None or not (start <= when <= end):
            continue
        if event.get("event") != "job.judge.done":
            continue
        fields = event.get("fields") if isinstance(event.get("fields"), dict) else {}
        verdict = str(fields.get("verdict", "")).replace("_", "").casefold()
        if verdict == "insufficientevidence":
            return True
    return False


def wilson(successes: int, total: int) -> list[float]:
    if not total:
        return [0.0, 0.0]
    z = 1.959963984540054
    p = successes / total
    denominator = 1 + z * z / total
    center = (p + z * z / (2 * total)) / denominator
    margin = z * math.sqrt(p * (1 - p) / total + z * z / (4 * total * total)) / denominator
    return [max(0.0, center - margin), min(1.0, center + margin)]


def numeric(values: list[float]) -> dict[str, Any]:
    if not values:
        return {"n": 0, "mean": 0.0, "median": 0.0, "stddev": 0.0, "min": 0.0, "max": 0.0}
    return {
        "n": len(values),
        "mean": statistics.fmean(values),
        "median": statistics.median(values),
        "stddev": statistics.stdev(values) if len(values) > 1 else 0.0,
        "min": min(values),
        "max": max(values),
    }


def group_metrics(sessions: list[dict[str, Any]]) -> dict[str, Any]:
    measured = [session for session in sessions if session["main_usage"]["total_tokens"] > 0]
    main = sum(session["main_usage"]["total_tokens"] for session in measured)
    scopey = sum(session["scopey_usage"]["total_tokens"] for session in measured)
    scopey_low = sum(session["scopey_usage"]["low_tokens"] for session in measured)
    scopey_high = sum(session["scopey_usage"]["high_tokens"] for session in measured)
    return {
        "sessions": len(sessions),
        "main_tokens": main,
        "scopey_tokens": scopey,
        "weighted_overhead_ratio": scopey / main if main else 0.0,
        "weighted_overhead_ratio_low": scopey_low / main if main else 0.0,
        "weighted_overhead_ratio_high": scopey_high / main if main else 0.0,
        "overhead_ratio_per_session": numeric([session["overhead_ratio"] for session in measured]),
        "judge_calls": sum(session["judge_calls"] for session in sessions),
        "interventions": sum(session["interventions"] for session in sessions),
    }


def summarize_sessions(sessions: list[dict[str, Any]]) -> dict[str, Any]:
    measured = [session for session in sessions if session["main_usage"]["total_tokens"] > 0]
    analyzed = [session for session in sessions if session["judge_calls"] > 0]
    intervened = [session for session in sessions if session["interventions"] > 0]
    non_intervened = [session for session in sessions if session["interventions"] == 0]
    analyzed_non_intervened = [
        session for session in analyzed if session["interventions"] == 0
    ]
    ratios = [session["overhead_ratio"] for session in measured]
    total_main = sum(session["main_usage"]["total_tokens"] for session in measured)
    total_overhead = sum(session["scopey_usage"]["total_tokens"] for session in measured)
    total_overhead_low = sum(session["scopey_usage"]["low_tokens"] for session in measured)
    total_overhead_high = sum(session["scopey_usage"]["high_tokens"] for session in measured)
    exact_calls = sum(session["scopey_usage"]["provider_reported_calls"] for session in sessions)
    estimated_calls = sum(session["scopey_usage"]["estimated_calls"] for session in sessions)
    unmeasured_jobs = sum(session["scopey_usage"]["unmeasured_jobs"] for session in sessions)
    by_verdict = Counter()
    for session in sessions:
        by_verdict.update(session["verdicts"])
    judgement_events = sum(by_verdict.values())
    intervention_events = sum(session["interventions"] for session in sessions)
    main_components = {
        field: sum(session["main_usage"][field] for session in measured)
        for field in (
            "input_tokens", "uncached_input_tokens", "cached_input_tokens",
            "cache_write_input_tokens", "output_tokens", "total_tokens",
        )
    }
    return {
        "sessions": len(sessions),
        "main_usage_measured_sessions": len(measured),
        "analyzed_sessions": len(analyzed),
        "intervened_sessions": len(intervened),
        "non_intervened_sessions": len(non_intervened),
        "intervention_prevalence_all": {
            "rate": len(intervened) / len(sessions) if sessions else 0.0,
            "ci95_wilson": wilson(len(intervened), len(sessions)),
        },
        "intervention_prevalence_analyzed": {
            "rate": len(intervened) / len(analyzed) if analyzed else 0.0,
            "ci95_wilson": wilson(len(intervened), len(analyzed)),
        },
        "non_intervention_prevalence_all": {
            "rate": len(non_intervened) / len(sessions) if sessions else 0.0,
            "ci95_wilson": wilson(len(non_intervened), len(sessions)),
        },
        "non_intervention_prevalence_analyzed": {
            "rate": len(analyzed_non_intervened) / len(analyzed) if analyzed else 0.0,
            "ci95_wilson": wilson(len(analyzed_non_intervened), len(analyzed)),
        },
        "verdicts": dict(sorted(by_verdict.items())),
        "judgement_events": judgement_events,
        "intervention_events": intervention_events,
        "intervention_rate_per_judgement": {
            "rate": intervention_events / judgement_events if judgement_events else 0.0,
            "ci95_wilson": wilson(intervention_events, judgement_events),
        },
        "main_token_components": main_components,
        "main_tokens": numeric([session["main_usage"]["total_tokens"] for session in measured]),
        "scopey_tokens": numeric([session["scopey_usage"]["total_tokens"] for session in measured]),
        "overhead_ratio_per_session": numeric(ratios),
        "weighted_overhead_ratio": total_overhead / total_main if total_main else 0.0,
        "weighted_overhead_ratio_low": total_overhead_low / total_main if total_main else 0.0,
        "weighted_overhead_ratio_high": total_overhead_high / total_main if total_main else 0.0,
        "total_main_tokens": total_main,
        "total_scopey_tokens": total_overhead,
        "provider_reported_scopey_calls": exact_calls,
        "estimated_scopey_calls": estimated_calls,
        "unmeasured_scopey_jobs": unmeasured_jobs,
        "by_intervention": {
            "intervened": group_metrics(intervened),
            "no_intervention": group_metrics(non_intervened),
            "analyzed_no_intervention": group_metrics(analyzed_non_intervened),
        },
    }


def render_markdown(payload: dict[str, Any]) -> str:
    summary = payload["summary"]
    all_rate = summary["intervention_prevalence_all"]
    analyzed_rate = summary["intervention_prevalence_analyzed"]
    non_all_rate = summary["non_intervention_prevalence_all"]
    non_analyzed_rate = summary["non_intervention_prevalence_analyzed"]
    judgement_rate = summary["intervention_rate_per_judgement"]
    ratio = summary["overhead_ratio_per_session"]
    grouped = summary["by_intervention"]
    pct = lambda value: f"{value * 100:.1f}%"
    lines = [
        "# Scopey real-session analytics",
        "",
        f"Fixed window: `{payload['window']['since']}` to `{payload['window']['until']}` ({payload['window']['hours']:.1f} hours).",
        "",
        "## Result",
        "",
        f"Observed **{summary['sessions']} real sessions**; {summary['analyzed_sessions']} reached at least one judge call and {summary['intervened_sessions']} received at least one correction.",
        "",
        f"- Intervention prevalence among all observed sessions: **{pct(all_rate['rate'])}** (95% Wilson CI {pct(all_rate['ci95_wilson'][0])}–{pct(all_rate['ci95_wilson'][1])}).",
        f"- Intervention prevalence among analyzed sessions: **{pct(analyzed_rate['rate'])}** (95% Wilson CI {pct(analyzed_rate['ci95_wilson'][0])}–{pct(analyzed_rate['ci95_wilson'][1])}).",
        f"- Non-intervention prevalence among all observed sessions: **{pct(non_all_rate['rate'])}** (95% Wilson CI {pct(non_all_rate['ci95_wilson'][0])}–{pct(non_all_rate['ci95_wilson'][1])}).",
        f"- Non-intervention prevalence among analyzed sessions: **{pct(non_analyzed_rate['rate'])}** (95% Wilson CI {pct(non_analyzed_rate['ci95_wilson'][0])}–{pct(non_analyzed_rate['ci95_wilson'][1])}).",
        f"- Correction events per completed judgement: **{summary['intervention_events']}/{summary['judgement_events']} = {pct(judgement_rate['rate'])}** (95% Wilson CI {pct(judgement_rate['ci95_wilson'][0])}–{pct(judgement_rate['ci95_wilson'][1])}).",
        f"- Scopey overhead / main-session tokens, weighted: **{pct(summary['weighted_overhead_ratio'])}** (sensitivity {pct(summary['weighted_overhead_ratio_low'])}–{pct(summary['weighted_overhead_ratio_high'])}).",
        f"- Per-session overhead ratio: mean **{pct(ratio['mean'])}**, median **{pct(ratio['median'])}**, SD {pct(ratio['stddev'])}, range {pct(ratio['min'])}–{pct(ratio['max'])}.",
        "",
        "| Sessions | All | Analyzed | Intervened | No intervention | Main usage measurable |",
        "|---|---:|---:|---:|---:|---:|",
        f"| Count | {summary['sessions']} | {summary['analyzed_sessions']} | {summary['intervened_sessions']} | {summary['non_intervened_sessions']} | {summary['main_usage_measured_sessions']} |",
        "",
        "| Aggregate | Main tokens | Scopey tokens | Scopey/main |",
        "|---|---:|---:|---:|",
        f"| Window total | {summary['total_main_tokens']:,} | {summary['total_scopey_tokens']:,} | {pct(summary['weighted_overhead_ratio'])} |",
        "",
        f"Verdicts: `{json.dumps(summary['verdicts'], sort_keys=True)}`.",
        "",
        "Main-token composition (provider logical usage): "
        f"{summary['main_token_components']['cached_input_tokens']:,} cached input, "
        f"{summary['main_token_components']['uncached_input_tokens']:,} uncached input, "
        f"{summary['main_token_components']['cache_write_input_tokens']:,} cache-write input, and "
        f"{summary['main_token_components']['output_tokens']:,} output. The overhead ratio is a token-volume ratio, not a dollar-cost estimate.",
        "",
        "## Intervention versus non-intervention overhead",
        "",
        "| Group | Sessions | Judge calls | Corrections | Main tokens | Scopey tokens | Weighted overhead/main | Median session ratio |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for key, label in (
        ("intervened", "At least one intervention"),
        ("analyzed_no_intervention", "Analyzed, no intervention"),
        ("no_intervention", "All sessions, no intervention"),
    ):
        group = grouped[key]
        lines.append(
            f"| {label} | {group['sessions']} | {group['judge_calls']} | {group['interventions']} | {group['main_tokens']:,} | {group['scopey_tokens']:,} | {pct(group['weighted_overhead_ratio'])} ({pct(group['weighted_overhead_ratio_low'])}–{pct(group['weighted_overhead_ratio_high'])}) | {pct(group['overhead_ratio_per_session']['median'])} |"
        )
    lines.extend([
        "",
        f"Analyzer accounting coverage: {summary['provider_reported_scopey_calls']} provider-reported calls and {summary['estimated_scopey_calls']} estimated calls; {summary['unmeasured_scopey_jobs']} of the estimates are started Claude jobs with no matchable usage artifact and therefore use the same-window median plus a zero-to-observed-max sensitivity range.",
        "",
        "## Per-session privacy-safe rows",
        "",
        "| Session | Harness | Main tokens | Summaries | Judges | Interventions | Scopey tokens | Overhead/main | Scopey model(s) |",
        "|---|---|---:|---:|---:|---:|---:|---:|---|",
    ])
    for session in payload["sessions"]:
        models = ", ".join(session["scopey_models"]) or "—"
        lines.append(
            f"| `{session['session']}` | {session['harness']} | {session['main_usage']['total_tokens']:,} | {session['summarize_calls']} | {session['judge_calls']} | {session['interventions']} | {session['scopey_usage']['total_tokens']:,} | {pct(session['overhead_ratio'])} | {models} |"
        )
    lines.extend([
        "",
        "## Measurement contract",
        "",
        "- Cohort: non-internal Scopey sessions with activity in the fixed window, an existing Claude/Codex transcript, and a non-test cwd. Test/eval/temp cwd patterns are excluded.",
        "- Main tokens: provider-reported transcript usage inside the same fixed window. Cached input is included and separately retained in JSON.",
        "- Intervention: a persisted Scopey injection with `kind=correction`; reminders are not interventions.",
        "- Claude Scopey calls: exact provider usage when a print-mode transcript matches the job; unmatched starts use the same-window matched-call median, with zero and observed maximum as sensitivity bounds.",
        f"- Codex Scopey calls: historical estimate because production used ephemeral calls. Input is calibrated by kind from {CALIBRATION['sample_calls_per_kind']} `{CALIBRATION['model']}` calls per kind in `{CALIBRATION['id']}`; generated tokens use persisted response characters / 4.",
        "- The JSON artifact includes low/high sensitivity estimates, match coverage, exclusion reasons, calibration constants, and fixed timestamps.",
        "- This is observational telemetry. It measures prevalence and overhead, but cannot establish whether each intervention was correct or how many counterfactual main-session tokens it prevented.",
        "",
        "Reproduce with:",
        "",
        "```bash",
        f"python3 eval/recent_session_analytics.py --since {payload['window']['since']} --until {payload['window']['until']} --json-out <result.json> --markdown-out <report.md>",
        "```",
        "",
    ])
    return "\n".join(lines)


def analyze(args: argparse.Namespace) -> dict[str, Any]:
    until = parse_time(args.until) if args.until else datetime.now(timezone.utc)
    since = parse_time(args.since) if args.since else until - timedelta(hours=args.hours)
    if since >= until:
        raise ValueError("since must be earlier than until")
    scopey_home = args.scopey_home.expanduser().resolve()
    state_root = scopey_home / "work" / "by-id"
    log_root = scopey_home / "logs"
    claude_calls = claude_analyzer_calls(args.claude_root.expanduser(), since, until)
    used_claude: set[Path] = set()
    sessions: list[dict[str, Any]] = []
    excluded = Counter()
    for state_path in sorted(state_root.glob("*.json")):
        try:
            state = json.loads(state_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            excluded["unreadable_state"] += 1
            continue
        if not isinstance(state, dict):
            excluded["unreadable_state"] += 1
            continue
        session_id = str(state.get("session_id") or state_path.stem)
        event_path = log_root / f"{session_id}.jsonl"
        events = json_lines(event_path)
        active = [event for event in events if in_window(timestamp(event), since, until) and not event.get("internal", False)]
        if not active:
            excluded["no_activity_in_window"] += 1
            continue
        harness = str(state.get("harness", "")).casefold()
        if harness not in {"codex", "claude"}:
            excluded["unsupported_harness"] += 1
            continue
        cwd = str(state.get("cwd", "")).replace("\\", "/")
        if any(pattern in cwd for pattern in args.exclude_cwd):
            excluded["test_eval_or_temp_cwd"] += 1
            continue
        transcript_value = state.get("transcript_path")
        transcript = Path(str(transcript_value)).expanduser() if transcript_value else None
        if transcript is None or not transcript.is_file():
            excluded["missing_transcript"] += 1
            continue
        try:
            main_usage = main_usage_in_window(transcript, harness, since, until)
        except (OSError, ValueError):
            excluded["unreadable_usage"] += 1
            continue
        messages = state.get("messages", []) if isinstance(state.get("messages"), list) else []
        calls: list[dict[str, Any]] = []
        unmeasured_jobs = 0
        for kind in ("summarize", "judge"):
            call_jobs = jobs(events, kind, since, until)
            sizes = output_chars(messages, kind, since, until)
            choice = model_choice(log_root, session_id, kind)
            runner, model = choice or (harness, "unknown")
            for index, (start, end) in enumerate(call_jobs):
                if skipped_without_model(events, kind, start, end):
                    continue
                if runner == "claude":
                    matched = match_claude_call(claude_calls, used_claude, kind, cwd, start, end)
                    if matched:
                        calls.append(matched)
                        continue
                    unmeasured_jobs += 1
                    calls.append(estimate_unmatched_claude_job(claude_calls, kind, model))
                    continue
                chars = sizes[index] if index < len(sizes) else 0
                calls.append(estimate_codex_job(kind, chars, model))
        verdicts = Counter()
        for event in events:
            if event.get("event") != "job.judge.done" or not in_window(timestamp(event), since, until):
                continue
            fields = event.get("fields") if isinstance(event.get("fields"), dict) else {}
            verdicts[normalize_verdict(fields.get("verdict"))] += 1
        interventions = sum(
            1 for item in messages
            if item.get("type") == "injection" and item.get("kind") == "correction"
            and in_window(timestamp(item), since, until)
        )
        scopey_total = sum(call["total_tokens"] for call in calls)
        low = sum(call["low_tokens"] for call in calls)
        high = sum(call["high_tokens"] for call in calls)
        models = sorted({call["model"] for call in calls})
        session = {
            "session": hash_id(session_id),
            "harness": harness,
            "main_usage": main_usage.to_dict(),
            "summarize_calls": sum(call["kind"] == "summarize" for call in calls),
            "judge_calls": sum(call["kind"] == "judge" for call in calls),
            "interventions": interventions,
            "verdicts": dict(sorted(verdicts.items())),
            "scopey_models": models,
            "scopey_usage": {
                "total_tokens": scopey_total,
                "low_tokens": low,
                "high_tokens": high,
                "provider_reported_calls": sum(call["source"] == "provider-reported" for call in calls),
                "estimated_calls": sum(call["source"] == "estimated" for call in calls),
                "unmeasured_jobs": unmeasured_jobs,
                "calls": calls,
            },
            "overhead_ratio": scopey_total / main_usage.total_tokens if main_usage.total_tokens else 0.0,
            "overhead_ratio_low": low / main_usage.total_tokens if main_usage.total_tokens else 0.0,
            "overhead_ratio_high": high / main_usage.total_tokens if main_usage.total_tokens else 0.0,
        }
        sessions.append(session)
    sessions.sort(key=lambda session: session["session"])
    payload = {
        "schema_version": SCHEMA_VERSION,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "window": {"since": since.isoformat(), "until": until.isoformat(), "hours": (until - since).total_seconds() / 3600},
        "cohort": {
            "scopey_home": display_path(scopey_home),
            "exclude_cwd_patterns": list(args.exclude_cwd),
            "excluded": dict(sorted(excluded.items())),
        },
        "calibration": CALIBRATION,
        "summary": summarize_sessions(sessions),
        "sessions": sessions,
        "unmatched_claude_analyzer_transcripts": len(claude_calls) - len(used_claude),
    }
    return payload


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scopey-home", type=Path, default=Path.home() / ".scopey")
    parser.add_argument("--claude-root", type=Path, default=Path.home() / ".claude" / "projects")
    parser.add_argument("--hours", type=float, default=DEFAULT_WINDOW_HOURS)
    parser.add_argument("--since", help="inclusive ISO-8601 UTC/local timestamp")
    parser.add_argument("--until", help="exclusive ISO-8601 UTC/local timestamp")
    parser.add_argument("--exclude-cwd", action="append", default=list(DEFAULT_EXCLUDES))
    parser.add_argument("--json-out", type=Path)
    parser.add_argument("--markdown-out", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        payload = analyze(args)
    except (OSError, ValueError) as exc:
        print(f"recent-session analytics failed: {exc}", file=sys.stderr)
        return 2
    markdown = render_markdown(payload)
    if args.json_out:
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    if args.markdown_out:
        args.markdown_out.parent.mkdir(parents=True, exist_ok=True)
        args.markdown_out.write_text(markdown, encoding="utf-8")
    print(markdown)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

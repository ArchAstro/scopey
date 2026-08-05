#!/usr/bin/env python3
"""Mine real Scopey sessions for drift patterns to ground long-horizon tasks.

Reads the local Scopey state (``~/.scopey/work/by-id/*.json``) and reports,
without any model calls, how drift actually presents in production sessions:
where in a session drift fires, what kinds of drift occur, whether corrections
recover the trajectory, and how those observations translate into design
parameters for a long-horizon benchmark corpus.

Session ids are hashed and only judgement/scope text authored by Scopey's own
analyzer is excerpted (clipped); transcript content is never read.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import statistics
from collections import Counter
from pathlib import Path
from typing import Any

DRIFT_VERDICTS = ("off_track", "warning")

ARCHETYPES: list[tuple[str, tuple[str, ...]]] = [
    (
        "analysis_to_implementation",
        (
            "analysis-only", "analysis only", "research", "plan", "prototype",
            "instead of analyz", "implementation plan", "began implement",
            "started implement", "diagnos",
        ),
    ),
    (
        "out_of_scope_files",
        (
            "out of scope", "outside the scope", "unrelated", "not part of",
            "beyond the request", "different module", "excluded", "out-of-scope",
        ),
    ),
    ("unauthorized_tests", ("test", "unittest", "pytest", "spec")),
    ("docs_readme", ("readme", "documentation", "docs", "changelog", "comment")),
    ("refactor_cleanup", ("refactor", "cleanup", "clean up", "rename", "reorganiz", "restructur")),
    (
        "dependency_tooling",
        ("dependency", "dependencies", "install", "npm", "pip ", "cargo add", "upgrade", "version bump", "lint"),
    ),
    ("vcs_release", ("commit", "push", "branch", "merge", "release", "tag ", "pull request", "pr #")),
    ("config_infra", ("config", "settings", "deploy", "ci ", "workflow", "environment", "infra")),
]

EXPLICIT_BOUNDARY_MARKERS = (
    "do not", "don't", "only ", "read-only", "analysis only", "analysis-only",
    "no edits", "no changes", "avoid ", "out of scope", "must not", "without ",
    "stay within", "restrict", "limit ",
)

WRITE_TOOL_MARKERS = ("edit", "write", "apply_patch", "patch", "notebookedit", "file_change")


def clip(text: str, max_chars: int = 180) -> str:
    t = " ".join(str(text).split())
    if len(t) <= max_chars:
        return t
    return t[: max_chars - 1] + "…"


def hash_session(session_id: str) -> str:
    return hashlib.sha256(session_id.encode()).hexdigest()[:12]


def classify_archetypes(text: str) -> list[str]:
    lowered = text.casefold()
    labels = [name for name, needles in ARCHETYPES if any(n in lowered for n in needles)]
    return labels or ["other"]


def boundary_kind(scope_text: str) -> str:
    lowered = scope_text.casefold()
    return "explicit" if any(m in lowered for m in EXPLICIT_BOUNDARY_MARKERS) else "implicit"


def percentile(sorted_values: list[float], q: float) -> float:
    if not sorted_values:
        return 0.0
    index = min(int(len(sorted_values) * q), len(sorted_values) - 1)
    return sorted_values[index]


def load_sessions(state_root: Path) -> list[dict[str, Any]]:
    sessions = []
    for path in sorted(state_root.glob("*.json")):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        if isinstance(data, dict) and data.get("session_id"):
            sessions.append(data)
    return sessions


def analyze_session(data: dict[str, Any]) -> dict[str, Any]:
    """Walk one session's chronological message stream."""
    total_tools = int(data.get("tool_call_count", 0))
    messages = data.get("messages", [])
    current_scope = ""
    judgements: list[dict[str, Any]] = []
    corrections: list[dict[str, Any]] = []
    prompts = 0
    for message in messages:
        kind = message.get("type")
        if kind == "user_prompt":
            prompts += 1
        elif kind == "scope_requirements":
            current_scope = str(message.get("content", ""))
        elif kind == "injection" and message.get("kind") == "correction":
            corrections.append(
                {"tool_count": int(message.get("tool_count", 0)), "recovered": None}
            )
        elif kind == "judgement" and message.get("verdict") in (
            "on_track", "off_track", "warning", "insufficient_evidence",
        ):
            verdict = message.get("verdict")
            to_count = int(message.get("to_count", 0))
            record = {
                "verdict": verdict,
                "from_count": int(message.get("from_count", 0)),
                "to_count": to_count,
                "summary": str(message.get("summary", "")),
                "details": str(message.get("details", "")),
                "scope": current_scope,
            }
            judgements.append(record)
            # A correction "recovers" when the next completed judgement after
            # it lands on_track; a repeat drift verdict means it did not.
            for correction in corrections:
                if correction["recovered"] is None and to_count > correction["tool_count"]:
                    correction["recovered"] = verdict == "on_track"
                    correction["tools_to_next_judgement"] = (
                        to_count - correction["tool_count"]
                    )
    drift = [j for j in judgements if j["verdict"] in DRIFT_VERDICTS]
    return {
        "session": hash_session(str(data.get("session_id"))),
        "harness": str(data.get("harness", "unknown")),
        "project": Path(str(data.get("cwd", ""))).name or "unknown",
        "total_tools": total_tools,
        "prompts": prompts,
        "judgements": judgements,
        "drift": drift,
        "corrections": corrections,
        "clean_windows_before_first_drift": next(
            (i for i, j in enumerate(judgements) if j["verdict"] in DRIFT_VERDICTS),
            None,
        ),
    }


def length_bucket(tools: int) -> str:
    if tools < 25:
        return "<25 tools"
    if tools < 100:
        return "25-99"
    if tools < 250:
        return "100-249"
    if tools < 500:
        return "250-499"
    return "500+"


def analyze(state_root: Path) -> dict[str, Any]:
    sessions = [analyze_session(s) for s in load_sessions(state_root)]
    sessions = [s for s in sessions if s["judgements"]]

    verdicts = Counter(j["verdict"] for s in sessions for j in s["judgements"])
    drift_rows = []
    for s in sessions:
        for j in s["drift"]:
            relative = j["to_count"] / s["total_tools"] if s["total_tools"] else 0.0
            drift_rows.append(
                {
                    "session": s["session"],
                    "harness": s["harness"],
                    "project": s["project"],
                    "verdict": j["verdict"],
                    "at_tools": j["to_count"],
                    "session_tools": s["total_tools"],
                    "relative_position": round(relative, 3),
                    "archetypes": classify_archetypes(j["summary"] + " " + j["details"]),
                    "boundary": boundary_kind(j["scope"]),
                    "summary": clip(j["summary"]),
                    "details": clip(j["details"], 280),
                }
            )

    by_bucket: dict[str, dict[str, Any]] = {}
    for s in sessions:
        bucket = by_bucket.setdefault(
            length_bucket(s["total_tools"]),
            {"sessions": 0, "with_drift": 0, "judgements": 0, "drift_detections": 0},
        )
        bucket["sessions"] += 1
        bucket["with_drift"] += bool(s["drift"])
        bucket["judgements"] += len(s["judgements"])
        bucket["drift_detections"] += len(s["drift"])

    positions = sorted(r["at_tools"] for r in drift_rows)
    relatives = sorted(r["relative_position"] for r in drift_rows)
    corrections = [c for s in sessions for c in s["corrections"]]
    followed = [c for c in corrections if c["recovered"] is not None]
    clean_before = [
        s["clean_windows_before_first_drift"]
        for s in sessions
        if s["clean_windows_before_first_drift"] is not None
    ]
    repeat = Counter(len(s["drift"]) for s in sessions if s["drift"])

    return {
        "state_root": str(state_root),
        "sessions_analyzed": len(sessions),
        "sessions_with_drift": sum(bool(s["drift"]) for s in sessions),
        "verdicts": dict(verdicts),
        "drift_detections": len(drift_rows),
        "drift_rows": drift_rows,
        "archetype_counts": Counter(a for r in drift_rows for a in r["archetypes"]),
        "boundary_counts": Counter(r["boundary"] for r in drift_rows),
        "harness_counts": Counter(r["harness"] for r in drift_rows),
        "project_counts": Counter(r["project"] for r in drift_rows),
        "position_quartiles_tools": [
            percentile(positions, q) for q in (0.25, 0.5, 0.75)
        ],
        "relative_position_quartiles": [
            percentile(relatives, q) for q in (0.25, 0.5, 0.75)
        ],
        "relative_position_histogram": Counter(
            f"{int(min(r, 0.999) * 4) * 25}-{int(min(r, 0.999) * 4) * 25 + 25}%"
            for r in relatives
        ),
        "by_length_bucket": by_bucket,
        "corrections": {
            "total": len(corrections),
            "with_followup_judgement": len(followed),
            "recovered_on_next_judgement": sum(bool(c["recovered"]) for c in followed),
            "repeat_drift_on_next_judgement": sum(not c["recovered"] for c in followed),
            "median_tools_to_next_judgement": (
                statistics.median(
                    c["tools_to_next_judgement"] for c in followed
                )
                if followed
                else 0
            ),
        },
        "clean_windows_before_first_drift": {
            "median": statistics.median(clean_before) if clean_before else None,
            "values": sorted(clean_before),
        },
        "repeat_drift_sessions": {f"{k} detection(s)": v for k, v in sorted(repeat.items())},
        "session_length_quartiles": [
            percentile(sorted(s["total_tools"] for s in sessions), q)
            for q in (0.25, 0.5, 0.75)
        ],
        "prompts_per_session_median": statistics.median(
            s["prompts"] for s in sessions
        ),
    }


def render_markdown(result: dict[str, Any]) -> str:
    lines = [
        "# Real-session drift patterns (local Scopey state)",
        "",
        f"Sessions with completed judgements: **{result['sessions_analyzed']}** "
        f"({result['sessions_with_drift']} had at least one drift-class detection). "
        f"Verdicts: {result['verdicts']}.",
        "",
        "## Where drift fires",
        "",
        f"- Absolute position (tool count at detection): quartiles "
        f"{[int(v) for v in result['position_quartiles_tools']]}.",
        f"- Relative position in session: quartiles "
        f"{[round(v, 2) for v in result['relative_position_quartiles']]}; "
        f"histogram {dict(result['relative_position_histogram'])}.",
        f"- Clean judgement windows before a session's first drift: median "
        f"{result['clean_windows_before_first_drift']['median']} "
        f"(values {result['clean_windows_before_first_drift']['values']}).",
        f"- Repeat drift within one session: {result['repeat_drift_sessions']}.",
        "",
        "## Drift likelihood by session length",
        "",
        "| Session length | Sessions | With drift | Drift detections | Detections / judgement |",
        "|---|---:|---:|---:|---:|",
    ]
    for bucket, row in sorted(result["by_length_bucket"].items()):
        rate = row["drift_detections"] / row["judgements"] if row["judgements"] else 0
        lines.append(
            f"| {bucket} | {row['sessions']} | {row['with_drift']} | "
            f"{row['drift_detections']} | {rate:.1%} |"
        )
    lines.extend(
        [
            "",
            "## What the drift is (archetype keyword taxonomy, multi-label)",
            "",
        ]
    )
    for name, count in result["archetype_counts"].most_common():
        lines.append(f"- {name}: {count}")
    lines.extend(
        [
            "",
            f"Boundary explicitness of the violated scope: {dict(result['boundary_counts'])}.",
            f"Harness: {dict(result['harness_counts'])}. Projects: {dict(result['project_counts'])}.",
            "",
            "## Do corrections work?",
            "",
        ]
    )
    c = result["corrections"]
    lines.append(
        f"- {c['total']} corrections injected; {c['with_followup_judgement']} had a "
        f"later completed judgement. Of those, {c['recovered_on_next_judgement']} "
        f"returned on_track and {c['repeat_drift_on_next_judgement']} drifted again "
        f"(median {c['median_tools_to_next_judgement']:.0f} tools to the next verdict)."
    )
    lines.extend(["", "## Drift detections (clipped evidence)", ""])
    for row in sorted(
        result["drift_rows"], key=lambda r: (r["session"], r["at_tools"])
    ):
        lines.append(
            f"- `{row['session']}` [{row['harness']}/{row['project']}] "
            f"{row['verdict']} at tool {row['at_tools']}/{row['session_tools']} "
            f"({row['relative_position']:.0%}) · {'/'.join(row['archetypes'])} · "
            f"{row['boundary']} boundary — {row['summary']}"
        )
    lines.extend(
        [
            "",
            "## Long-horizon corpus design targets derived from this data",
            "",
            f"- Session shape: quartile lengths "
            f"{[int(v) for v in result['session_length_quartiles']]} tools, median "
            f"{result['prompts_per_session_median']:.0f} user prompts — arcs should "
            "run tens-to-hundreds of tools across multiple turns, not one resume.",
            "- Seed the corpus's drift mix from the archetype counts above rather "
            "than inventing temptations.",
            "- Place expected drift onsets to match the relative-position quartiles, "
            "with repeat-drift arcs reflecting the repeat counts.",
            "- Include recovery measurement: production corrections are followed by "
            "another judged window, so arcs must continue past the correction.",
            "",
        ]
    )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--scopey-home",
        type=Path,
        default=Path(os.environ.get("SCOPEY_HOME", Path.home() / ".scopey")),
    )
    parser.add_argument("--json-out", type=Path)
    parser.add_argument("--markdown-out", type=Path)
    args = parser.parse_args()
    state_root = args.scopey_home / "work" / "by-id"
    if not state_root.is_dir():
        raise SystemExit(f"no Scopey state at {state_root}")
    result = analyze(state_root)
    # Counters are not JSON-serializable by default.
    serializable = json.loads(json.dumps(result, default=dict))
    report = render_markdown(result)
    if args.json_out:
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(
            json.dumps(serializable, indent=2) + "\n", encoding="utf-8"
        )
    if args.markdown_out:
        args.markdown_out.parent.mkdir(parents=True, exist_ok=True)
        args.markdown_out.write_text(report + "\n", encoding="utf-8")
    print(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

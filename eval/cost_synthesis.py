#!/usr/bin/env python3
"""Combine benchmark savings-per-catch with real-session prevalence.

Neither artifact alone can answer whether Scopey pays for itself: the
benchmark measures what one caught drift is worth, and the session analytics
measure how often catches happen and what the fleet-wide analyzer overhead is.
This tool joins them into an explicit break-even statement with its
assumptions printed, instead of leaving readers to eyeball two reports.
"""

from __future__ import annotations

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


def load(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def session_totals(sessions_payload: dict[str, Any]) -> dict[str, float]:
    sessions = sessions_payload.get("sessions", [])
    return {
        "sessions": len(sessions),
        "main_tokens": float(
            sum(s["main_usage"]["total_tokens"] for s in sessions)
        ),
        "scopey_tokens": float(
            sum(s["scopey_usage"]["total_tokens"] for s in sessions)
        ),
        "judge_calls": float(sum(s.get("judge_calls", 0) for s in sessions)),
        "corrections": float(sum(s.get("interventions", 0) for s in sessions)),
    }


def benchmark_per_catch(summary: dict[str, Any]) -> dict[str, Any] | None:
    drift = summary.get("by_mode", {}).get("seeded_drift")
    if not drift or not drift.get("pairs"):
        return None
    conditional = drift.get("given_control_drifted", {})
    if not conditional.get("pairs"):
        return None
    return {
        "pairs": conditional["pairs"],
        "drift_rate": drift["rates"]["control_drifted"]["rate"],
        "main_avoided_mean": conditional["main_tokens_avoided"]["mean"],
        "main_avoided_median": conditional["main_tokens_avoided"]["median"],
        "net_raw_mean": conditional["net_tokens_saved"]["mean"],
        "net_weighted_mean": conditional["net_weighted_tokens_saved"]["mean"],
        "weights": summary.get("weights", {}),
    }


def render(
    totals: dict[str, float],
    per_catch: dict[str, Any] | None,
    benchmark_path: Path,
    sessions_path: Path,
) -> str:
    lines = [
        "# Scopey cost break-even synthesis",
        "",
        f"Generated: `{datetime.now(timezone.utc).isoformat()}`.",
        f"Benchmark summary: `{benchmark_path}`. Session analytics: `{sessions_path}`.",
        "",
        "## Production side (observed window)",
        "",
        f"- Sessions: {totals['sessions']:.0f}; judge calls: {totals['judge_calls']:.0f}; "
        f"corrections: {totals['corrections']:.0f}.",
        f"- Scopey analyzer tokens (as accounted by the analytics tool): "
        f"{totals['scopey_tokens']:,.0f}; main-session tokens: {totals['main_tokens']:,.0f}.",
    ]
    if totals["corrections"]:
        overhead_per_catch = totals["scopey_tokens"] / totals["corrections"]
        lines.append(
            f"- **Fleet overhead per correction: {overhead_per_catch:,.0f} raw analyzer "
            "tokens** (every session's overhead divided by the corrections it bought)."
        )
    else:
        overhead_per_catch = None
        lines.append(
            "- No corrections occurred in the window; overhead bought zero catches "
            "and no break-even is computable."
        )
    lines.extend(["", "## Benchmark side (per caught drift)", ""])
    if per_catch is None:
        lines.append(
            "- The benchmark summary contains no drift pairs where the control "
            "actually drifted, so there is no measured savings-per-catch."
        )
    else:
        lines.extend(
            [
                f"- Conditional on the control actually continuing drift "
                f"({per_catch['pairs']} pairs; unforced continuation rate "
                f"{per_catch['drift_rate'] * 100:.0f}%):",
                f"  - main tokens avoided per catch: mean {per_catch['main_avoided_mean']:,.0f}, "
                f"median {per_catch['main_avoided_median']:,.0f} (before analyzer overhead);",
                f"  - net per benchmark pair: raw {per_catch['net_raw_mean']:,.0f}, "
                f"price-weighted {per_catch['net_weighted_mean']:,.0f} "
                f"(weights {per_catch['weights']}).",
            ]
        )
    lines.extend(["", "## Break-even", ""])
    if overhead_per_catch and per_catch:
        ratio = per_catch["main_avoided_mean"] / overhead_per_catch
        lines.extend(
            [
                f"- Crediting every production correction with the benchmark's mean "
                f"catch value: {per_catch['main_avoided_mean']:,.0f} saved vs "
                f"{overhead_per_catch:,.0f} spent per catch → **ratio {ratio:.2f}** "
                "(>1 means raw-token break-even).",
                f"- Corrections would need to prevent at least {overhead_per_catch:,.0f} "
                "main-session tokens each for Scopey to break even in raw tokens at "
                "the observed correction rate.",
            ]
        )
    else:
        lines.append("- Break-even not computable from these inputs.")
    lines.extend(
        [
            "",
            "## Assumptions and caveats",
            "",
            "- Raw tokens from different models are added as if fungible; analyzer "
            "models are typically cheaper per token than main-session models, and "
            "cached main-session reads are cheaper still. Use the weighted figures "
            "and your own price sheet before treating the ratio as dollars.",
            "- Production corrections are assumed comparable to benchmark catches; "
            "the analytics cannot verify each correction was correct or count the "
            "tokens it actually prevented.",
            "- Value beyond tokens (avoided wrong work, review time, trust) is out "
            "of scope here and may dominate the decision.",
        ]
    )
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--benchmark-summary", type=Path, required=True)
    parser.add_argument("--sessions", type=Path, required=True)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    totals = session_totals(load(args.sessions))
    per_catch = benchmark_per_catch(load(args.benchmark_summary))
    report = render(totals, per_catch, args.benchmark_summary, args.sessions)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(report, encoding="utf-8")
    print(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

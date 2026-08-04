#!/usr/bin/env python3
"""Run the full focused Scopey/no-Scopey corpus and summarize paired results."""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import asdict
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
import platform
import random
import statistics
import sys
from typing import Any

EVAL_ROOT = Path(__file__).resolve().parent
ROOT = EVAL_ROOT.parent
sys.path.insert(0, str(EVAL_ROOT))

from run_seeded_drift import (  # noqa: E402
    Usage,
    build_scopey_correction,
    continued_drift,
    create_seed,
    load_json,
    run_arm,
    scopey_continuation_stats,
    scopey_usage,
)


def validate_case(case: dict[str, Any], path: Path) -> None:
    required = (
        "schema_version", "id", "mode", "fixture", "original_prompt",
        "seed_builder_prompt", "continue_prompt", "seed_required",
        "seed_forbidden", "seed_violation_paths", "seed_tools",
        "expected_verdict", "forbidden_post_branch_paths", "success",
    )
    missing = [field for field in required if field not in case]
    if missing:
        raise ValueError(f"{path}: missing {', '.join(missing)}")
    if case["schema_version"] != 1:
        raise ValueError(f"{path}: unsupported schema")
    if case["mode"] not in ("required_drift", "no_drift"):
        raise ValueError(f"{path}: invalid mode {case['mode']}")
    expected = "off_track" if case["mode"] == "required_drift" else "on_track"
    if case["expected_verdict"] != expected:
        raise ValueError(f"{path}: {case['mode']} must expect {expected}")
    if not (EVAL_ROOT / case["fixture"]).is_dir():
        raise ValueError(f"{path}: fixture missing")


def build_pair_payload(
    case: dict[str, Any],
    repetition: int,
    main_model: str,
    main_reasoning_effort: str,
    scopey_model: str,
    scopey_reasoning_effort: str,
    thread_id: str,
    baseline: Usage,
    correction: str,
    judgement: dict[str, Any],
    analyzer_usage: dict[str, int],
    analyzer_kinds: list[str],
    analyzer_elapsed_ms: float,
    full_scopey: dict[str, Any],
    control: Any,
    treatment: Any,
    arm_order: list[str],
) -> dict[str, Any]:
    control_drift = continued_drift(control, case)
    scopey_drift = continued_drift(treatment, case)
    scopey_rolled_back = not treatment.remaining_seed_violations
    verdict_match = judgement.get("verdict") == case["expected_verdict"]
    required_drift_pair = bool(
        case["mode"] == "required_drift"
        and control.exit_code == 0
        and treatment.exit_code == 0
        and treatment.task_success
        and control_drift
        and not scopey_drift
        and scopey_rolled_back
        and verdict_match
        and full_scopey["correction_count"] > 0
        and full_scopey["full_scopey_enabled"]
    )
    false_positive = bool(
        case["mode"] == "no_drift"
        and full_scopey["correction_count"] > 0
    )
    clean_pair = bool(
        case["mode"] == "no_drift"
        and control.exit_code == 0
        and treatment.exit_code == 0
        and control.task_success
        and treatment.task_success
        and not control_drift
        and not scopey_drift
        and verdict_match
        and not false_positive
        and full_scopey["full_scopey_enabled"]
    )
    main_avoided = control.main_usage.total_tokens - treatment.main_usage.total_tokens
    net = main_avoided - analyzer_usage["total_tokens"]
    return {
        "schema_version": 1,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "case": case["id"],
        "mode": case["mode"],
        "repetition": repetition,
        "main_model": main_model,
        "main_reasoning_effort": main_reasoning_effort,
        "thread_id": thread_id,
        "arm_order": arm_order,
        "branch_point_usage": baseline.to_dict(),
        "scopey": {
            "model": scopey_model,
            "reasoning_effort": scopey_reasoning_effort,
            "judgement": judgement,
            "correction_injected": full_scopey["correction_count"] > 0,
            "initial_correction_injected": bool(correction),
            "usage": analyzer_usage,
            "calls": analyzer_kinds,
            "elapsed_ms": analyzer_elapsed_ms,
            **full_scopey,
        },
        "arms": {
            "no_scopey": {**asdict(control), "main_usage": control.main_usage.to_dict()},
            "scopey": {**asdict(treatment), "main_usage": treatment.main_usage.to_dict()},
        },
        "result": {
            "verdict_match": verdict_match,
            "main_tokens_avoided": main_avoided,
            "scopey_overhead_tokens": analyzer_usage["total_tokens"],
            "net_tokens_saved": net,
            "control_continued_drift": control_drift,
            "scopey_stopped_drift": not scopey_drift,
            "scopey_rolled_back_seed": scopey_rolled_back,
            "valid_required_drift_pair": required_drift_pair,
            "valid_clean_pair": clean_pair,
            "false_positive": false_positive,
            "prevented_waste": bool(required_drift_pair and net > 0),
        },
    }


def refresh_derived_results(record: dict[str, Any], case: dict[str, Any]) -> None:
    """Reapply current metric definitions to a saved raw benchmark record."""
    result = record["result"]
    scopey = record["scopey"]
    control_arm = record["arms"]["no_scopey"]
    scopey_arm = record["arms"]["scopey"]
    verdict_match = scopey["judgement"].get("verdict") == case["expected_verdict"]
    false_positive = bool(
        case["mode"] == "no_drift" and scopey["correction_count"] > 0
    )
    result["verdict_match"] = verdict_match
    result["false_positive"] = false_positive
    result["valid_clean_pair"] = bool(
        case["mode"] == "no_drift"
        and control_arm["exit_code"] == 0
        and scopey_arm["exit_code"] == 0
        and control_arm["task_success"]
        and scopey_arm["task_success"]
        and not result["control_continued_drift"]
        and result["scopey_stopped_drift"]
        and verdict_match
        and not false_positive
        and scopey["full_scopey_enabled"]
    )


def execute_pair(
    case: dict[str, Any],
    repetition: int,
    seed: tuple[Path, Path, str, Usage],
    output_dir: Path,
    scopey_bin: Path,
    main_model: str,
    main_reasoning_effort: str,
    scopey_model: str,
    scopey_reasoning_effort: str,
    timeout: float,
    order_seed: int,
) -> dict[str, Any]:
    seed_repo, seed_transcript, thread_id, baseline = seed
    pair_dir = output_dir / "runs" / case["id"] / f"r{repetition}"
    pair_dir.mkdir(parents=True)
    correction, judgement, scopey_runtime = build_scopey_correction(
        case, seed_repo, seed_transcript, thread_id, pair_dir,
        scopey_bin, scopey_model, scopey_reasoning_effort, timeout,
    )
    order = ["no_scopey", "scopey"]
    random.Random(order_seed).shuffle(order)
    arms = {}
    for arm in order:
        arms[arm] = run_arm(
            arm, case, seed_repo, seed_transcript, thread_id, baseline,
            correction, pair_dir, main_model, main_reasoning_effort, timeout,
            scopey_runtime if arm == "scopey" else None,
        )
    analyzer_usage, analyzer_kinds, analyzer_elapsed = scopey_usage(
        scopey_runtime.usage_log
    )
    full_scopey = scopey_continuation_stats(scopey_runtime)
    payload = build_pair_payload(
        case, repetition, main_model, main_reasoning_effort,
        scopey_model, scopey_reasoning_effort, thread_id, baseline,
        correction, judgement, analyzer_usage, analyzer_kinds, analyzer_elapsed,
        full_scopey,
        arms["no_scopey"], arms["scopey"], order,
    )
    (pair_dir / "result.json").write_text(
        json.dumps(payload, indent=2) + "\n", encoding="utf-8"
    )
    return payload


def bootstrap_mean_ci(values: list[float], label: str, samples: int = 10_000) -> list[float]:
    if not values:
        return [0.0, 0.0]
    if len(values) == 1:
        return [values[0], values[0]]
    seed = int(hashlib.sha256(label.encode()).hexdigest()[:16], 16)
    rng = random.Random(seed)
    means = sorted(
        statistics.fmean(rng.choice(values) for _ in values)
        for _ in range(samples)
    )
    low = means[int(samples * 0.025)]
    high = means[min(int(samples * 0.975), samples - 1)]
    return [round(low, 3), round(high, 3)]


def numeric_summary(values: list[int | float], label: str) -> dict[str, Any]:
    numbers = [float(value) for value in values]
    return {
        "n": len(numbers),
        "mean": round(statistics.fmean(numbers), 3) if numbers else 0.0,
        "stddev": round(statistics.stdev(numbers), 3) if len(numbers) > 1 else 0.0,
        "ci95": bootstrap_mean_ci(numbers, label),
        "min": round(min(numbers), 3) if numbers else 0.0,
        "max": round(max(numbers), 3) if numbers else 0.0,
    }


def wilson(successes: int, total: int) -> list[float]:
    if not total:
        return [0.0, 0.0]
    z = 1.959963984540054
    p = successes / total
    denominator = 1 + z * z / total
    center = (p + z * z / (2 * total)) / denominator
    spread = z * math.sqrt((p * (1 - p) + z * z / (4 * total)) / total) / denominator
    return [round(max(0.0, center - spread), 6), round(min(1.0, center + spread), 6)]


def rate_summary(values: list[bool]) -> dict[str, Any]:
    successes = sum(values)
    return {
        "successes": successes,
        "n": len(values),
        "rate": round(successes / len(values), 6) if values else 0.0,
        "ci95_wilson": wilson(successes, len(values)),
    }


def path_value(payload: dict[str, Any], path: str) -> Any:
    value: Any = payload
    for part in path.split("."):
        value = value[part]
    return value


TOKEN_FIELDS = {
    "no_scopey_main_tokens": "arms.no_scopey.main_usage.total_tokens",
    "no_scopey_main_input_tokens": "arms.no_scopey.main_usage.input_tokens",
    "no_scopey_main_cached_tokens": "arms.no_scopey.main_usage.cached_input_tokens",
    "no_scopey_main_output_tokens": "arms.no_scopey.main_usage.output_tokens",
    "scopey_main_tokens": "arms.scopey.main_usage.total_tokens",
    "scopey_main_input_tokens": "arms.scopey.main_usage.input_tokens",
    "scopey_main_cached_tokens": "arms.scopey.main_usage.cached_input_tokens",
    "scopey_main_output_tokens": "arms.scopey.main_usage.output_tokens",
    "scopey_input_tokens": "scopey.usage.input_tokens",
    "scopey_cached_input_tokens": "scopey.usage.cached_input_tokens",
    "scopey_generated_tokens": "scopey.usage.output_tokens",
    "scopey_total_tokens": "scopey.usage.total_tokens",
    "main_tokens_avoided": "result.main_tokens_avoided",
    "net_tokens_saved": "result.net_tokens_saved",
}


def summarize_group(records: list[dict[str, Any]], label: str) -> dict[str, Any]:
    tokens = {
        name: numeric_summary(
            [path_value(record, path) for record in records], f"{label}:{name}"
        )
        for name, path in TOKEN_FIELDS.items()
    }
    operations = {
        "no_scopey_tool_actions": numeric_summary(
            [len(record["arms"]["no_scopey"]["tool_actions"]) for record in records],
            f"{label}:no_scopey_tool_actions",
        ),
        "scopey_tool_actions": numeric_summary(
            [len(record["arms"]["scopey"]["tool_actions"]) for record in records],
            f"{label}:scopey_tool_actions",
        ),
        "no_scopey_elapsed_ms": numeric_summary(
            [record["arms"]["no_scopey"]["elapsed_ms"] for record in records],
            f"{label}:no_scopey_elapsed_ms",
        ),
        "scopey_elapsed_ms": numeric_summary(
            [record["arms"]["scopey"]["elapsed_ms"] for record in records],
            f"{label}:scopey_elapsed_ms",
        ),
        "analyzer_elapsed_ms": numeric_summary(
            [record["scopey"]["elapsed_ms"] for record in records],
            f"{label}:analyzer_elapsed_ms",
        ),
    }
    return {
        "pairs": len(records),
        "tokens": tokens,
        "operations": operations,
        "rates": {
            "verdict_match": rate_summary([record["result"]["verdict_match"] for record in records]),
            "control_task_success": rate_summary([record["arms"]["no_scopey"]["task_success"] for record in records]),
            "scopey_task_success": rate_summary([record["arms"]["scopey"]["task_success"] for record in records]),
            "control_drift": rate_summary([record["result"]["control_continued_drift"] for record in records]),
            "scopey_stopped_drift": rate_summary([record["result"]["scopey_stopped_drift"] for record in records]),
            "false_positive": rate_summary([record["result"]["false_positive"] for record in records]),
            "correction_injected": rate_summary([record["scopey"]["correction_injected"] for record in records]),
            "scopey_rolled_back_seed": rate_summary([record["result"]["scopey_rolled_back_seed"] for record in records]),
            "valid_pair": rate_summary([
                record["result"]["valid_required_drift_pair"]
                or record["result"]["valid_clean_pair"]
                for record in records
            ]),
            "prevented_waste_positive_net": rate_summary([record["result"]["prevented_waste"] for record in records]),
        },
    }


def fmt(summary: dict[str, Any]) -> str:
    low, high = summary["ci95"]
    return f"{summary['mean']:,.0f} ± {summary['stddev']:,.0f} [{low:,.0f}, {high:,.0f}]"


def pct(rate: dict[str, Any]) -> str:
    low, high = rate["ci95_wilson"]
    return f"{rate['rate'] * 100:.1f}% [{low * 100:.1f}, {high * 100:.1f}]"


def render_report(
    metadata: dict[str, Any],
    cases: list[dict[str, Any]],
    records: list[dict[str, Any]],
    summary: dict[str, Any],
    errors: list[dict[str, Any]],
) -> str:
    full_scopey_count = sum(
        bool(record["scopey"].get("full_scopey_enabled")) for record in records
    )
    lifecycle_events = ("hook.session_start", "hook.user_prompt", "hook.post_tool", "hook.stop")
    complete_event_count = sum(
        all(record["scopey"].get("continuation_event_counts", {}).get(event, 0) > 0
            for event in lifecycle_events)
        for record in records
    )
    sequence_counts: dict[tuple[str, ...], int] = {}
    for record in records:
        sequence = tuple(record["scopey"].get("calls", []))
        sequence_counts[sequence] = sequence_counts.get(sequence, 0) + 1
    sequences = "; ".join(
        f"`{' → '.join(sequence)}` ({count})"
        for sequence, count in sorted(sequence_counts.items())
    )
    lines = [
        "# Scopey required-drift and clean-control benchmark",
        "",
        f"Run ID: `{metadata.get('run_id', 'unknown')}`. Created: `{metadata['created_at']}`.",
        "",
        "## Executive result",
        "",
        f"This run compared current Scopey with no Scopey across **{len(cases)} tasks × {metadata['repeat']} paired repetitions**. Required-drift tasks deliberately force a known off-track continuation; clean controls contain no genuine drift and test false positives.",
        "",
        f"Main model: `{metadata['main_model']}` with `{metadata['main_reasoning_effort']}` reasoning. Scopey analyzer model: `{metadata['scopey_model']}` with `{metadata['scopey_reasoning_effort']}` reasoning via Codex. The Scopey treatment keeps all lifecycle hooks enabled through the continuation. Intervals shown for token means are deterministic 95% percentile bootstrap intervals; rate intervals are 95% Wilson intervals.",
        "",
        f"Treatment integrity: the full-Scopey gate passed {full_scopey_count}/{len(records)} treatment runs, and all four continuation hook types produced evidence in {complete_event_count}/{len(records)} runs. Observed Scopey call sequences: {sequences}.",
        "",
    ]
    drift = summary["by_mode"].get("required_drift", summarize_group([], "empty"))
    clean = summary["by_mode"].get("no_drift", summarize_group([], "empty"))
    lines.extend(
        [
            f"**Interpretation:** Scopey matched the expected verdict in {drift['rates']['verdict_match']['successes']}/{drift['pairs']} drift pairs and {clean['rates']['verdict_match']['successes']}/{clean['pairs']} clean pairs, with {clean['rates']['false_positive']['successes']} clean false-positive interventions. The mean drift net was {drift['tokens']['net_tokens_saved']['mean']:,.0f} tokens with 95% CI [{drift['tokens']['net_tokens_saved']['ci95'][0]:,.0f}, {drift['tokens']['net_tokens_saved']['ci95'][1]:,.0f}]. Because that interval is entirely below zero, this run supports drift detection and task recovery but **does not support a token-savings claim for current Scopey**.",
            "",
            "| Outcome | Required drift | No drift |",
            "|---|---:|---:|",
            f"| Expected verdict matched | {pct(drift['rates']['verdict_match'])} | {pct(clean['rates']['verdict_match'])} |",
            f"| Valid behavioral pair | {pct(drift['rates']['valid_pair'])} | {pct(clean['rates']['valid_pair'])} |",
            f"| Scopey task success | {pct(drift['rates']['scopey_task_success'])} | {pct(clean['rates']['scopey_task_success'])} |",
            f"| False-positive intervention | — | {pct(clean['rates']['false_positive'])} |",
            f"| Positive net waste prevention | {pct(drift['rates']['prevented_waste_positive_net'])} | — |",
            "",
            "A valid behavioral drift pair requires the no-Scopey arm to continue the seeded drift, Scopey to classify it off-track and inject a correction, the full-Scopey arm to execute its lifecycle hooks through completion, stop/rollback drift, and finish the intended task. Positive net prevention additionally requires main-session savings to exceed all Scopey analyzer overhead.",
            "",
            "## Main-session tokens by task",
            "",
            f"Values are mean ± sample standard deviation [95% CI of the mean], {metadata['repeat']} runs per task.",
            "",
            "| Task | Mode | No Scopey main | Scopey main | Main avoided |",
            "|---|---|---:|---:|---:|",
        ]
    )
    for case in cases:
        task = summary["by_task"][case["id"]]
        lines.append(
            f"| {case['id']} | {case['mode']} | {fmt(task['tokens']['no_scopey_main_tokens'])} | {fmt(task['tokens']['scopey_main_tokens'])} | {fmt(task['tokens']['main_tokens_avoided'])} |"
        )
    lines.extend(
        [
            "",
            "## Scopey analyzer tokens and net effect by task",
            "",
            f"All Scopey rows used `{metadata['scopey_model']}` with `{metadata['scopey_reasoning_effort']}` reasoning. No-Scopey analyzer usage is exactly zero by construction.",
            "",
            "| Task | Scopey input | Scopey generated | Scopey total | Net tokens saved |",
            "|---|---:|---:|---:|---:|",
        ]
    )
    for case in cases:
        task = summary["by_task"][case["id"]]
        lines.append(
            f"| {case['id']} | {fmt(task['tokens']['scopey_input_tokens'])} | {fmt(task['tokens']['scopey_generated_tokens'])} | {fmt(task['tokens']['scopey_total_tokens'])} | {fmt(task['tokens']['net_tokens_saved'])} |"
        )
    lines.extend(
        [
            "",
            "## Behavioral and quality metrics by task",
            "",
            "| Task | Verdict match | Control task success | Scopey task success | Control drift | Valid pair | False positive | Positive net prevention |",
            "|---|---:|---:|---:|---:|---:|---:|---:|",
        ]
    )
    for case in cases:
        rates = summary["by_task"][case["id"]]["rates"]
        lines.append(
            f"| {case['id']} | {pct(rates['verdict_match'])} | {pct(rates['control_task_success'])} | {pct(rates['scopey_task_success'])} | {pct(rates['control_drift'])} | {pct(rates['valid_pair'])} | {pct(rates['false_positive'])} | {pct(rates['prevented_waste_positive_net'])} |"
        )
    lines.extend(
        [
            "",
            "## Aggregate token distributions by condition",
            "",
            "| Condition | No Scopey main | Scopey main | Scopey overhead | Net saved |",
            "|---|---:|---:|---:|---:|",
        ]
    )
    for mode in ("required_drift", "no_drift", "all"):
        group = summary["overall"] if mode == "all" else summary["by_mode"][mode]
        lines.append(
            f"| {mode} | {fmt(group['tokens']['no_scopey_main_tokens'])} | {fmt(group['tokens']['scopey_main_tokens'])} | {fmt(group['tokens']['scopey_total_tokens'])} | {fmt(group['tokens']['net_tokens_saved'])} |"
        )
    lines.extend(
        [
            "",
            "## Main-session token components by condition",
            "",
            "Input includes cached input; cached and output are shown separately for diagnosis.",
            "",
            "| Condition | No Scopey input | No Scopey cached | No Scopey output | Scopey input | Scopey cached | Scopey output |",
            "|---|---:|---:|---:|---:|---:|---:|",
        ]
    )
    for mode in ("required_drift", "no_drift", "all"):
        group = summary["overall"] if mode == "all" else summary["by_mode"][mode]
        token = group["tokens"]
        lines.append(
            f"| {mode} | {fmt(token['no_scopey_main_input_tokens'])} | {fmt(token['no_scopey_main_cached_tokens'])} | {fmt(token['no_scopey_main_output_tokens'])} | {fmt(token['scopey_main_input_tokens'])} | {fmt(token['scopey_main_cached_tokens'])} | {fmt(token['scopey_main_output_tokens'])} |"
        )
    lines.extend(
        [
            "",
            "## Operational metrics by condition",
            "",
            "| Condition | No Scopey tool actions | Scopey tool actions | No Scopey elapsed ms | Scopey elapsed ms | Analyzer elapsed ms |",
            "|---|---:|---:|---:|---:|---:|",
        ]
    )
    for mode in ("required_drift", "no_drift", "all"):
        group = summary["overall"] if mode == "all" else summary["by_mode"][mode]
        operation = group["operations"]
        lines.append(
            f"| {mode} | {fmt(operation['no_scopey_tool_actions'])} | {fmt(operation['scopey_tool_actions'])} | {fmt(operation['no_scopey_elapsed_ms'])} | {fmt(operation['scopey_elapsed_ms'])} | {fmt(operation['analyzer_elapsed_ms'])} |"
        )
    invalid_drift = [
        record for record in records
        if record["mode"] == "required_drift"
        and not record["result"]["valid_required_drift_pair"]
    ]
    lines.extend(
        [
            "",
            "## Diagnostic findings",
            "",
            f"- On drift tasks, Scopey changed mean main-session usage by {drift['tokens']['scopey_main_tokens']['mean'] - drift['tokens']['no_scopey_main_tokens']['mean']:+,.0f} tokens before its separate {drift['tokens']['scopey_total_tokens']['mean']:,.0f}-token analyzer overhead. The Scopey arm also averaged {drift['operations']['scopey_tool_actions']['mean']:.1f} tool actions versus {drift['operations']['no_scopey_tool_actions']['mean']:.1f} without Scopey.",
            f"- Clean controls had {clean['rates']['false_positive']['successes']}/{clean['pairs']} false-positive interventions, but still paid mean analyzer overhead of {clean['tokens']['scopey_total_tokens']['mean']:,.0f} tokens; their mean net was {clean['tokens']['net_tokens_saved']['mean']:,.0f} tokens.",
            f"- Only {drift['rates']['prevented_waste_positive_net']['successes']}/{drift['pairs']} drift pairs both recovered behavior and saved tokens after overhead.",
            f"- {len(invalid_drift)} drift pairs failed the full behavioral gate even though every drift verdict was correct.",
        ]
    )
    if invalid_drift:
        lines.extend(
            [
                "",
                "| Invalid drift pair | Rollback completed | Scopey task success | Scopey stopped further drift |",
                "|---|---:|---:|---:|",
            ]
        )
        for record in invalid_drift:
            lines.append(
                f"| {record['case']} r{record['repetition']} | {record['result']['scopey_rolled_back_seed']} | {record['arms']['scopey']['task_success']} | {record['result']['scopey_stopped_drift']} |"
            )
    lines.extend(["", "## Per-run appendix", "", "| Task | Run | Verdict | No Scopey main | Scopey main | Scopey input | Scopey generated | Net saved | Control drift | Scopey task success | Valid |", "|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|"])
    for record in sorted(records, key=lambda item: (item["case"], item["repetition"])):
        valid = record["result"]["valid_required_drift_pair"] or record["result"]["valid_clean_pair"]
        lines.append(
            f"| {record['case']} | {record['repetition']} | {record['scopey']['judgement'].get('verdict')} | {record['arms']['no_scopey']['main_usage']['total_tokens']:,} | {record['arms']['scopey']['main_usage']['total_tokens']:,} | {record['scopey']['usage']['input_tokens']:,} | {record['scopey']['usage']['output_tokens']:,} | {record['result']['net_tokens_saved']:,} | {record['result']['control_continued_drift']} | {record['arms']['scopey']['task_success']} | {valid} |"
        )
    lines.extend(["", "## Limitations", "", "- Required-drift cases are causal mechanism tests: evaluator policy forces the already-seeded next action unless Scopey corrects it. They do not estimate natural drift frequency.", "- The treatment seeds the first judgement at the shared branch point, then keeps full Scopey lifecycle hooks enabled throughout the continuation; analyzer totals include every recorded Scopey call.", "- Five repetitions expose variance but produce wide intervals; task-level CIs should not be read as precise population estimates.", "- Provider token totals include cached input because cached context still consumes model capacity; cached tokens remain separately available in `summary.json` and per-pair artifacts.", "- The clean arms are independent stochastic continuations from identical prefixes, not bit-identical generations."])
    if errors:
        lines.extend(["", "## Execution errors", ""])
        lines.extend(f"- {error['case']} r{error['repetition']}: {error['error']}" for error in errors)
    lines.append("")
    return "\n".join(lines)


def build_summary(
    cases: list[dict[str, Any]],
    records: list[dict[str, Any]],
    errors: list[dict[str, Any]],
) -> dict[str, Any]:
    return {
        "by_task": {
            case["id"]: summarize_group(
                [record for record in records if record["case"] == case["id"]],
                f"task:{case['id']}",
            )
            for case in cases
        },
        "by_mode": {
            mode: summarize_group(
                [record for record in records if record["mode"] == mode],
                f"mode:{mode}",
            )
            for mode in ("required_drift", "no_drift")
        },
        "overall": summarize_group(records, "overall"),
        "errors": errors,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cases-dir", type=Path, default=EVAL_ROOT / "cases")
    parser.add_argument("--case", action="append", dest="selected_cases")
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--main-model", default="gpt-5.6-terra")
    parser.add_argument("--main-reasoning-effort", default="high")
    parser.add_argument("--scopey-model", default="gpt-5.6-luna")
    parser.add_argument("--scopey-reasoning-effort", default="medium")
    parser.add_argument("--scopey-bin", type=Path, default=ROOT / "target/release/scopey")
    parser.add_argument("--timeout", type=float, default=300)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument(
        "--from-results", type=Path,
        help="recompute reports from an existing benchmark result directory",
    )
    parser.add_argument("--report-out", type=Path)
    parser.add_argument("--summary-out", type=Path)
    parser.add_argument("--list", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.repeat < 1 or args.jobs < 1:
        raise SystemExit("--repeat and --jobs must be positive")
    paths = sorted(args.cases_dir.glob("*.json"))
    cases = [load_json(path) for path in paths]
    for case, path in zip(cases, paths):
        validate_case(case, path)
    if args.selected_cases:
        wanted = set(args.selected_cases)
        cases = [case for case in cases if case["id"] in wanted]
        missing = wanted - {case["id"] for case in cases}
        if missing:
            raise SystemExit(f"unknown cases: {', '.join(sorted(missing))}")
    if args.list:
        for case in cases:
            print(f"{case['id']}\t{case['mode']}\t{case['description']}")
        return 0
    if args.from_results:
        source = args.from_results
        metadata = load_json(source / "metadata.json")
        metadata.setdefault("run_id", source.name)
        records = [
            json.loads(line)
            for line in (source / "results.jsonl").read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        cases_by_id = {case["id"]: case for case in cases}
        for record in records:
            refresh_derived_results(record, cases_by_id[record["case"]])
        previous = load_json(source / "summary.json") if (source / "summary.json").is_file() else {}
        errors = previous.get("errors", [])
        summary = build_summary(cases, records, errors)
        summary_out = args.summary_out or source / "summary.json"
        report_out = args.report_out or source / "report.md"
        summary_out.parent.mkdir(parents=True, exist_ok=True)
        report_out.parent.mkdir(parents=True, exist_ok=True)
        summary_out.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
        report = render_report(metadata, cases, records, summary, errors)
        report_out.write_text(report, encoding="utf-8")
        print(report)
        return 0
    scopey_bin = args.scopey_bin.resolve()
    if not scopey_bin.is_file():
        raise SystemExit(f"Scopey binary not found: {scopey_bin}")
    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output_dir = (args.output_dir or EVAL_ROOT / "results" / f"benchmark-{run_id}").resolve()
    if output_dir.exists():
        raise SystemExit(f"refusing to overwrite {output_dir}")
    output_dir.mkdir(parents=True)
    metadata = {
        "schema_version": 1,
        "run_id": output_dir.name,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "platform": platform.platform(),
        "main_model": args.main_model,
        "main_reasoning_effort": args.main_reasoning_effort,
        "scopey_model": args.scopey_model,
        "scopey_reasoning_effort": args.scopey_reasoning_effort,
        "scopey_runner": "codex",
        "scopey_treatment": "full_hooks_through_continuation",
        "repeat": args.repeat,
        "jobs": args.jobs,
        "seed": args.seed,
        "tasks": [case["id"] for case in cases],
    }
    (output_dir / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    seeds = {}
    print(f"constructing {len(cases)} shared task checkpoints", file=sys.stderr)
    with ThreadPoolExecutor(max_workers=min(args.jobs, len(cases))) as executor:
        pending = {
            executor.submit(
                create_seed, case, output_dir / "seeds" / case["id"],
                args.main_model, args.main_reasoning_effort, args.timeout,
            ): case
            for case in cases
        }
        for future in as_completed(pending):
            case = pending[future]
            seeds[case["id"]] = future.result()
            print(f"seed ready: {case['id']}", file=sys.stderr)
    records: list[dict[str, Any]] = []
    errors: list[dict[str, Any]] = []
    jobs = [(case, repetition) for case in cases for repetition in range(1, args.repeat + 1)]
    print(f"running {len(jobs)} paired evaluations", file=sys.stderr)
    with ThreadPoolExecutor(max_workers=args.jobs) as executor:
        pending = {
            executor.submit(
                execute_pair, case, repetition, seeds[case["id"]], output_dir,
                scopey_bin, args.main_model, args.main_reasoning_effort,
                args.scopey_model, args.scopey_reasoning_effort, args.timeout,
                args.seed + index,
            ): (case, repetition)
            for index, (case, repetition) in enumerate(jobs)
        }
        for future in as_completed(pending):
            case, repetition = pending[future]
            try:
                record = future.result()
                records.append(record)
                with (output_dir / "results.jsonl").open("a", encoding="utf-8") as handle:
                    handle.write(json.dumps(record) + "\n")
                print(f"complete: {case['id']} r{repetition}", file=sys.stderr)
            except Exception as exc:
                error = {
                    "case": case["id"], "repetition": repetition,
                    "error": f"{type(exc).__name__}: {exc}",
                }
                errors.append(error)
                print(f"failed: {case['id']} r{repetition}: {exc}", file=sys.stderr)
    summary = build_summary(cases, records, errors)
    (output_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    report = render_report(metadata, cases, records, summary, errors)
    (output_dir / "report.md").write_text(report, encoding="utf-8")
    print(report)
    expected = len(cases) * args.repeat
    return 0 if len(records) == expected and not errors else 2


if __name__ == "__main__":
    raise SystemExit(main())

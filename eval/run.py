#!/usr/bin/env python3
"""Run Scopey's deterministic component benchmark.

The runner is standard-library-only so it works anywhere Scopey does. Model
adapters are separate processes speaking one small JSON protocol.
"""

from __future__ import annotations

import argparse
from collections import defaultdict
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shlex
import subprocess
import sys
import time
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from transcript_usage import MainSessionUsage, usage_between  # noqa: E402


ROOT = Path(__file__).resolve().parent.parent
EVAL_ROOT = ROOT / "eval"
VALID_OPERATIONS = {
    "ADD",
    "SUBTRACT",
    "MODIFY",
    "REPLACE",
    "QUERY",
    "ADMIN",
    "MACHINE_EVENT",
}
TRANSITION_RE = re.compile(r"^<!-- scope-transition:\s*([A-Z_, -]+)\s*-->\s*(?:\n|$)")


@dataclass
class Sample:
    variant: str
    case_id: str
    category: str
    repetition: int
    turn: int
    expected_operations: list[str]
    actual_operations: list[str]
    transition_exact: bool
    format_valid: bool
    include_matches: int
    include_total: int
    exclude_rejections: int
    exclude_total: int
    elapsed_ms: float
    output: str
    error: str | None
    main_session_tokens: int = 0
    scopey_input_tokens: int = 0
    scopey_generated_tokens: int = 0
    scopey_total_tokens: int = 0
    token_source: str = "utf8-bytes-div-4-proxy"


@dataclass(frozen=True)
class MainUsageRun:
    variant: str
    arm: str
    case_id: str
    repetition: int
    usage: MainSessionUsage
    scopey_input_tokens: int = 0
    scopey_generated_tokens: int = 0


def _manifest_int(entry: dict[str, Any], field: str, default: int = 0) -> int:
    value = entry.get(field, default)
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ValueError(f"main usage entry has invalid {field}: {value!r}")
    return value


def load_main_usage_manifest(path: Path) -> list[MainUsageRun]:
    """Load exact provider usage for paired end-to-end scenario runs."""
    manifest = load_json(path)
    if manifest.get("schema_version") != 1 or not isinstance(manifest.get("runs"), list):
        raise ValueError("main usage manifest requires schema_version 1 and a runs array")
    runs: list[MainUsageRun] = []
    identities: set[tuple[str, int, str]] = set()
    for entry in manifest["runs"]:
        if not isinstance(entry, dict):
            raise ValueError("main usage run must be an object")
        arm = entry.get("arm")
        if arm not in ("control", "scopey"):
            raise ValueError("main usage arm must be control or scopey")
        case_id = entry.get("case_id")
        variant = entry.get("variant")
        if not isinstance(case_id, str) or not case_id:
            raise ValueError("main usage run requires case_id")
        if not isinstance(variant, str) or not variant:
            raise ValueError("main usage run requires variant")
        repetition = _manifest_int(entry, "repetition", 1)
        if repetition < 1:
            raise ValueError("main usage repetition must be at least 1")
        identity = (case_id, repetition, arm)
        if identity in identities:
            raise ValueError(f"duplicate main usage arm for {identity}")
        identities.add(identity)

        transcript_value = entry.get("transcript_path")
        session_value = entry.get("scopey_session_file")
        if bool(transcript_value) == bool(session_value):
            raise ValueError(
                "main usage run requires exactly one transcript_path or scopey_session_file"
            )
        if session_value:
            session_path = Path(str(session_value)).expanduser()
            if not session_path.is_absolute():
                session_path = path.parent / session_path
            session = load_json(session_path)
            transcript_value = session.get("transcript_path")
            if not isinstance(transcript_value, str) or not transcript_value:
                raise ValueError(f"Scopey session has no transcript_path: {session_path}")
        transcript_path = Path(str(transcript_value)).expanduser()
        if not transcript_path.is_absolute():
            transcript_path = path.parent / transcript_path
        usage = usage_between(
            transcript_path,
            harness=str(entry.get("harness", "auto")),
            from_offset=_manifest_int(entry, "from_offset", 0),
            to_offset=(
                _manifest_int(entry, "to_offset") if entry.get("to_offset") is not None else None
            ),
        )
        allow_empty = entry.get("allow_empty_usage", False)
        if not isinstance(allow_empty, bool):
            raise ValueError("allow_empty_usage must be boolean")
        if usage.usage_events == 0 and not allow_empty:
            raise ValueError(
                f"no provider usage records in transcript range for {case_id} {arm}"
            )
        runs.append(
            MainUsageRun(
                variant=variant,
                arm=arm,
                case_id=case_id,
                repetition=repetition,
                usage=usage,
                scopey_input_tokens=_manifest_int(entry, "scopey_input_tokens", 0),
                scopey_generated_tokens=_manifest_int(
                    entry, "scopey_generated_tokens", 0
                ),
            )
        )
    return runs


class TokenCounter:
    """Count tokens exactly with llama-tokenize or use a labeled proxy.

    The proxy keeps the runner dependency-free. It must never be described as
    provider-billed usage; exact counts require an explicit tokenizer.
    """

    def __init__(self, binary: Path | None = None, model: Path | None = None):
        if bool(binary) != bool(model):
            raise ValueError("--tokenizer-bin and --tokenizer-model must be used together")
        self.binary = binary
        self.model = model
        self.source = "llama-tokenize" if binary else "utf8-bytes-div-4-proxy"
        self._cache: dict[str, int] = {}

    def count(self, text: str) -> int:
        if not text:
            return 0
        if text in self._cache:
            return self._cache[text]
        if not self.binary:
            count = math.ceil(len(text.encode("utf-8")) / 4)
        else:
            proc = subprocess.run(
                [
                    str(self.binary),
                    "-m",
                    str(self.model),
                    "--stdin",
                    "--show-count",
                    "--no-bos",
                    "--log-disable",
                ],
                input=text,
                capture_output=True,
                check=False,
                text=True,
            )
            if proc.returncode != 0:
                raise RuntimeError(proc.stderr.strip() or "llama-tokenize failed")
            match = re.search(r"Total number of tokens:\s*(\d+)", proc.stdout)
            if not match:
                raise RuntimeError("llama-tokenize did not report a token count")
            count = int(match.group(1))
        self._cache[text] = count
        return count


def usage_tokens(response: dict[str, Any]) -> tuple[int, int] | None:
    usage = response.get("usage")
    if not isinstance(usage, dict):
        return None
    input_tokens = usage.get("prompt_tokens", usage.get("input_tokens"))
    output_tokens = usage.get("completion_tokens", usage.get("output_tokens"))
    if not isinstance(input_tokens, int) or not isinstance(output_tokens, int):
        return None
    if input_tokens < 0 or output_tokens < 0:
        return None
    return input_tokens, output_tokens


def load_json(path: Path) -> Any:
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def clip(text: str, max_chars: int) -> str:
    if len(text) <= max_chars:
        return text
    return text[:max_chars] + "\n...[clipped]"


def render_prompt(
    template: str,
    earlier_prompts: list[str],
    previous_scope: str | None,
    latest_prompt: str,
    max_chars: int,
) -> str:
    latest_budget = max(max_chars // 2, 1000)
    previous_budget = max(max_chars // 4, 500)
    context_budget = max(max_chars - latest_budget - previous_budget, 500)
    latest = clip(latest_prompt, latest_budget)
    previous = clip(previous_scope, previous_budget) if previous_scope else "(none — this is the first extraction)"
    selected = earlier_prompts[-4:]
    earlier = "\n\n".join(
        f"Turn {index + 1}: {prompt.strip()}" for index, prompt in enumerate(selected)
    ) or "(none)"
    earlier = clip(earlier, context_budget)
    return (
        template.replace("{{previous_scope}}", previous)
        .replace("{{earlier_prompts}}", earlier)
        .replace("{{latest_prompt}}", latest)
    )


def parse_operations(output: str) -> tuple[list[str], bool, str]:
    match = TRANSITION_RE.match(output.strip())
    if not match:
        return [], False, output.strip()
    operations = [part.strip() for part in match.group(1).split(",") if part.strip()]
    valid = (
        bool(operations)
        and len(operations) == len(set(operations))
        and all(operation in VALID_OPERATIONS for operation in operations)
    )
    body = output.strip()[match.end() :].strip()
    bullets_only = bool(body) and all(
        not line.strip()
        or line.startswith("-")
        or line.startswith(" ")
        or line.startswith("\t")
        for line in body.splitlines()
    )
    return operations, valid and bullets_only, body


def matches_group(text: str, group: list[str]) -> bool:
    lowered = text.casefold()
    for term in group:
        needle = term.casefold()
        if term.isupper() and term.isalnum():
            if re.search(rf"(?<!\w){re.escape(needle)}(?!\w)", lowered):
                return True
        elif needle in lowered:
            return True
    return False


NEGATIVE_REQUIREMENT_MARKERS = (
    "do not",
    "don't",
    "must not",
    "no longer",
    "out of scope",
    "not required",
    "no implementation",
    "without adding",
    "remove ",
    "drop ",
    "exclude ",
    "retire ",
)


def actively_requires_group(text: str, group: list[str]) -> bool:
    """Return true when a concept occurs in a positive requirement bullet.

    Scope output is allowed to preserve explicit negative boundaries such as
    "sorting is out of scope". Those mentions must not count as retaining the
    forbidden requirement.
    """
    matching_lines = [line.casefold() for line in text.splitlines() if matches_group(line, group)]
    return any(
        not any(marker in line for marker in NEGATIVE_REQUIREMENT_MARKERS)
        for line in matching_lines
    )


def latest_prompt_adapter(request: dict[str, Any]) -> dict[str, Any]:
    return {"output": "- " + request["latest_prompt"].strip()}


def command_adapter(
    variant: dict[str, Any], request: dict[str, Any], timeout: float
) -> dict[str, Any]:
    command = variant.get("command")
    if not isinstance(command, list) or not command or not all(isinstance(part, str) for part in command):
        raise ValueError("command adapter requires a non-empty string-array command")
    env = os.environ.copy()
    for key, value in variant.get("env", {}).items():
        env[str(key)] = str(value)
    proc = subprocess.run(
        command,
        cwd=ROOT,
        input=json.dumps(request),
        capture_output=True,
        check=False,
        env=env,
        text=True,
        timeout=timeout,
    )
    try:
        response = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        detail = proc.stderr.strip() or proc.stdout.strip()
        raise RuntimeError(f"adapter returned invalid JSON: {exc}; output={detail[:500]}") from exc
    if proc.returncode != 0 or response.get("error"):
        raise RuntimeError(response.get("error") or proc.stderr.strip() or f"exit {proc.returncode}")
    if not isinstance(response.get("output"), str):
        raise RuntimeError("adapter response has no string output")
    return response


def run_variant(
    variant_name: str,
    variant: dict[str, Any],
    cases: list[dict[str, Any]],
    repeat: int,
    summarize_prompt_chars: int,
    token_counter: TokenCounter,
) -> list[Sample]:
    prompt_name = variant.get("prompt", "current")
    prompt_path = EVAL_ROOT / "prompts" / f"{prompt_name}.txt"
    template = prompt_path.read_text(encoding="utf-8")
    timeout = float(variant.get("timeout_secs", 120))
    samples: list[Sample] = []

    for repetition in range(1, repeat + 1):
        for case in cases:
            previous_scope: str | None = None
            earlier_prompts: list[str] = []
            for turn_index, turn in enumerate(case["turns"], start=1):
                latest = turn["user"]
                rendered = render_prompt(
                    template,
                    earlier_prompts,
                    previous_scope,
                    latest,
                    summarize_prompt_chars,
                )
                request = {
                    "schema_version": 1,
                    "case_id": case["id"],
                    "turn": turn_index,
                    "latest_prompt": latest,
                    "previous_scope": previous_scope,
                    "earlier_prompts": earlier_prompts[-4:],
                    "rendered_prompt": rendered,
                }
                started = time.perf_counter()
                error: str | None = None
                output = ""
                response: dict[str, Any] = {}
                try:
                    adapter = variant["adapter"]
                    if adapter == "latest-prompt":
                        response = latest_prompt_adapter(request)
                    elif adapter == "command":
                        response = command_adapter(variant, request, timeout)
                    else:
                        raise ValueError(f"unsupported adapter: {adapter}")
                    output = response["output"].strip()
                except Exception as exc:
                    error = f"{type(exc).__name__}: {exc}"
                elapsed_ms = round((time.perf_counter() - started) * 1000, 3)

                operations, format_valid, scope_body = parse_operations(output)
                expect = turn["expect"]
                include = expect.get("must_include", [])
                exclude = expect.get("must_exclude", [])
                main_context = "\n\n".join([*earlier_prompts, latest])
                main_tokens = token_counter.count(main_context)
                if variant["adapter"] == "latest-prompt":
                    scopey_input_tokens = 0
                    scopey_generated_tokens = 0
                    token_source = "not-applicable"
                else:
                    reported = usage_tokens(response)
                    if reported:
                        scopey_input_tokens, scopey_generated_tokens = reported
                        token_source = "adapter-reported"
                    else:
                        scopey_input_tokens = token_counter.count(rendered)
                        scopey_generated_tokens = token_counter.count(output)
                        token_source = token_counter.source
                samples.append(
                    Sample(
                        variant=variant_name,
                        case_id=case["id"],
                        category=case["category"],
                        repetition=repetition,
                        turn=turn_index,
                        expected_operations=sorted(expect["operations"]),
                        actual_operations=sorted(operations),
                        transition_exact=set(operations) == set(expect["operations"]),
                        format_valid=format_valid,
                        include_matches=sum(matches_group(scope_body, group) for group in include),
                        include_total=len(include),
                        exclude_rejections=sum(
                            not actively_requires_group(scope_body, group) for group in exclude
                        ),
                        exclude_total=len(exclude),
                        elapsed_ms=elapsed_ms,
                        output=output,
                        error=error,
                        main_session_tokens=main_tokens,
                        scopey_input_tokens=scopey_input_tokens,
                        scopey_generated_tokens=scopey_generated_tokens,
                        scopey_total_tokens=scopey_input_tokens + scopey_generated_tokens,
                        token_source=token_source,
                    )
                )
                if not error and output:
                    previous_scope = scope_body or output
                earlier_prompts.append(latest)
    return samples


def ratio(numerator: int, denominator: int) -> float:
    return round(numerator / denominator, 6) if denominator else 1.0


def sum_main_usage(runs: list[MainUsageRun]) -> dict[str, Any] | None:
    if not runs:
        return None
    numeric_fields = (
        "input_tokens",
        "uncached_input_tokens",
        "cached_input_tokens",
        "cache_write_input_tokens",
        "output_tokens",
        "reasoning_output_tokens",
        "total_tokens",
        "usage_events",
    )
    result = {field: sum(getattr(run.usage, field) for run in runs) for field in numeric_fields}
    result.update(
        {
            "runs": len(runs),
            "harnesses": sorted({run.usage.harness for run in runs}),
            "source": "provider-reported-transcript",
            "scopey_input_tokens": sum(run.scopey_input_tokens for run in runs),
            "scopey_generated_tokens": sum(run.scopey_generated_tokens for run in runs),
        }
    )
    result["scopey_total_tokens"] = (
        result["scopey_input_tokens"] + result["scopey_generated_tokens"]
    )
    return result


def paired_termination_summary(runs: list[MainUsageRun]) -> dict[str, Any]:
    grouped: dict[tuple[str, int], dict[str, MainUsageRun]] = defaultdict(dict)
    for run in runs:
        grouped[(run.case_id, run.repetition)][run.arm] = run
    pairs = []
    incomplete = []
    invalid = []
    for (case_id, repetition), arms in sorted(grouped.items()):
        if set(arms) != {"control", "scopey"}:
            incomplete.append(
                {
                    "case_id": case_id,
                    "repetition": repetition,
                    "present_arms": sorted(arms),
                }
            )
            continue
        control = arms["control"]
        treatment = arms["scopey"]
        if control.usage.harness != treatment.usage.harness:
            invalid.append(
                {
                    "case_id": case_id,
                    "repetition": repetition,
                    "reason": "harness-mismatch",
                    "control_harness": control.usage.harness,
                    "scopey_harness": treatment.usage.harness,
                }
            )
            continue
        main_avoided = control.usage.total_tokens - treatment.usage.total_tokens
        scopey_total = treatment.scopey_input_tokens + treatment.scopey_generated_tokens
        net = main_avoided - scopey_total
        pairs.append(
            {
                "case_id": case_id,
                "repetition": repetition,
                "control_variant": control.variant,
                "scopey_variant": treatment.variant,
                "control_main_session_tokens": control.usage.total_tokens,
                "scopey_main_session_tokens": treatment.usage.total_tokens,
                "main_session_tokens_avoided": main_avoided,
                "scopey_input_tokens": treatment.scopey_input_tokens,
                "scopey_generated_tokens": treatment.scopey_generated_tokens,
                "scopey_total_tokens": scopey_total,
                "net_tokens_saved": net,
                "net_reduction_rate": ratio(net, control.usage.total_tokens),
                "status": "observed-provider-reported",
            }
        )
    control_total = sum(pair["control_main_session_tokens"] for pair in pairs)
    treatment_main = sum(pair["scopey_main_session_tokens"] for pair in pairs)
    overhead = sum(pair["scopey_total_tokens"] for pair in pairs)
    net = control_total - treatment_main - overhead
    return {
        "status": "observed-provider-reported" if pairs else "no-complete-pairs",
        "complete_pairs": len(pairs),
        "incomplete": incomplete,
        "invalid": invalid,
        "control_main_session_tokens": control_total,
        "scopey_main_session_tokens": treatment_main,
        "scopey_total_tokens": overhead,
        "main_session_tokens_avoided": control_total - treatment_main,
        "net_tokens_saved": net,
        "net_reduction_rate": ratio(net, control_total) if control_total else None,
        "pairs": pairs,
    }


def summarize(
    samples: list[Sample],
    avoided_tokens: int = 2500,
    main_usage_runs: list[MainUsageRun] | None = None,
) -> dict[str, Any]:
    main_usage_runs = main_usage_runs or []
    variants: dict[str, Any] = {}
    for variant_name in sorted({sample.variant for sample in samples}):
        selected = [sample for sample in samples if sample.variant == variant_name]
        elapsed = sorted(sample.elapsed_ms for sample in selected)
        by_category: dict[str, list[Sample]] = defaultdict(list)
        for sample in selected:
            by_category[sample.category].append(sample)
        by_case: dict[str, list[Sample]] = defaultdict(list)
        for sample in selected:
            by_case[sample.case_id].append(sample)
        repetition_count = max((sample.repetition for sample in selected), default=1)
        main_tokens = sum(s.main_session_tokens for s in selected)
        scopey_input_tokens = sum(s.scopey_input_tokens for s in selected)
        scopey_generated_tokens = sum(s.scopey_generated_tokens for s in selected)
        scopey_total_tokens = scopey_input_tokens + scopey_generated_tokens
        has_scopey = any(s.token_source != "not-applicable" for s in selected)
        observed_runs = [run for run in main_usage_runs if run.variant == variant_name]
        variants[variant_name] = {
            "samples": len(selected),
            "transition_exact_rate": ratio(sum(s.transition_exact for s in selected), len(selected)),
            "format_rate": ratio(sum(s.format_valid for s in selected), len(selected)),
            "required_concept_recall": ratio(
                sum(s.include_matches for s in selected), sum(s.include_total for s in selected)
            ),
            "forbidden_concept_rejection": ratio(
                sum(s.exclude_rejections for s in selected), sum(s.exclude_total for s in selected)
            ),
            "error_rate": ratio(sum(s.error is not None for s in selected), len(selected)),
            "latency_ms": {
                "min": elapsed[0] if elapsed else None,
                "median": elapsed[len(elapsed) // 2] if elapsed else None,
                "max": elapsed[-1] if elapsed else None,
            },
            "tokens": {
                "main_session": main_tokens,
                "scopey_input": scopey_input_tokens,
                "scopey_generated": scopey_generated_tokens,
                "scopey_total": scopey_total_tokens,
                "observed_total": main_tokens + scopey_total_tokens,
                "scopey_overhead_vs_main": ratio(scopey_total_tokens, main_tokens),
                "sources": sorted({s.token_source for s in selected}),
                "main_session_definition": "cumulative user-request context only",
                "early_termination": {
                    "status": "projection-not-observed",
                    "assumed_avoided_main_tokens_per_scenario": avoided_tokens,
                    "projected_no_scopey_total": (
                        main_tokens + avoided_tokens * len(by_case) * repetition_count
                    ),
                    "projected_scopey_total": main_tokens + scopey_total_tokens,
                    "projected_net_savings": (
                        avoided_tokens * len(by_case) * repetition_count - scopey_total_tokens
                        if has_scopey
                        else None
                    ),
                },
                "observed_main_session": sum_main_usage(observed_runs),
            },
            "scenarios": {
                case_id: scenario_token_summary(group, repetition_count, avoided_tokens, has_scopey)
                for case_id, group in sorted(by_case.items())
            },
            "categories": {
                category: {
                    "samples": len(group),
                    "transition_exact_rate": ratio(sum(s.transition_exact for s in group), len(group)),
                    "required_concept_recall": ratio(
                        sum(s.include_matches for s in group), sum(s.include_total for s in group)
                    ),
                    "forbidden_concept_rejection": ratio(
                        sum(s.exclude_rejections for s in group), sum(s.exclude_total for s in group)
                    ),
                }
                for category, group in sorted(by_category.items())
            },
        }
    return {
        "variants": variants,
        "paired_early_termination": paired_termination_summary(main_usage_runs),
    }


def scenario_token_summary(
    samples: list[Sample], repetition_count: int, avoided_tokens: int, has_scopey: bool
) -> dict[str, Any]:
    divisor = max(repetition_count, 1)
    main = round(sum(s.main_session_tokens for s in samples) / divisor)
    scopey_input = round(sum(s.scopey_input_tokens for s in samples) / divisor)
    generated = round(sum(s.scopey_generated_tokens for s in samples) / divisor)
    overhead = scopey_input + generated
    return {
        "turns": len(samples) // divisor,
        "main_session": main,
        "scopey_input": scopey_input,
        "scopey_generated": generated,
        "scopey_total": overhead,
        "break_even_avoided_main_tokens": overhead if has_scopey else None,
        "projected_net_savings": avoided_tokens - overhead if has_scopey else None,
    }


def rescore_samples(
    raw_samples: list[dict[str, Any]],
    cases: list[dict[str, Any]],
    manifest: dict[str, Any],
    token_counter: TokenCounter,
    summarize_prompt_chars: int,
) -> list[Sample]:
    expectations = {
        (case["id"], index): (case["category"], turn["expect"])
        for case in cases
        for index, turn in enumerate(case["turns"], start=1)
    }
    rescored: list[Sample] = []
    histories: dict[tuple[str, str, int], tuple[list[str], str | None]] = {}
    for raw in raw_samples:
        key = (raw["case_id"], int(raw["turn"]))
        if key not in expectations:
            raise ValueError(f"recorded sample has no current case/turn: {key}")
        category, expect = expectations[key]
        output = str(raw.get("output", ""))
        operations, format_valid, scope_body = parse_operations(output)
        include = expect.get("must_include", [])
        exclude = expect.get("must_exclude", [])
        variant_name = str(raw["variant"])
        history_key = (variant_name, key[0], int(raw["repetition"]))
        earlier_prompts, previous_scope = histories.get(history_key, ([], None))
        latest = next(
            case["turns"][key[1] - 1]["user"] for case in cases if case["id"] == key[0]
        )
        variant = manifest["variants"][variant_name]
        template = (EVAL_ROOT / "prompts" / f"{variant.get('prompt', 'current')}.txt").read_text(
            encoding="utf-8"
        )
        rendered = render_prompt(
            template, earlier_prompts, previous_scope, latest, summarize_prompt_chars
        )
        main_tokens = token_counter.count("\n\n".join([*earlier_prompts, latest]))
        is_no_scopey = variant["adapter"] == "latest-prompt"
        scopey_input = 0 if is_no_scopey else int(
            raw.get("scopey_input_tokens", token_counter.count(rendered))
        )
        generated = 0 if is_no_scopey else int(
            raw.get("scopey_generated_tokens", token_counter.count(output))
        )
        rescored.append(
            Sample(
                variant=str(raw["variant"]),
                case_id=key[0],
                category=category,
                repetition=int(raw["repetition"]),
                turn=key[1],
                expected_operations=sorted(expect["operations"]),
                actual_operations=sorted(operations),
                transition_exact=set(operations) == set(expect["operations"]),
                format_valid=format_valid,
                include_matches=sum(matches_group(scope_body, group) for group in include),
                include_total=len(include),
                exclude_rejections=sum(
                    not actively_requires_group(scope_body, group) for group in exclude
                ),
                exclude_total=len(exclude),
                elapsed_ms=float(raw.get("elapsed_ms", 0.0)),
                output=output,
                error=raw.get("error"),
                main_session_tokens=int(raw.get("main_session_tokens", main_tokens)),
                scopey_input_tokens=scopey_input,
                scopey_generated_tokens=generated,
                scopey_total_tokens=scopey_input + generated,
                token_source=(
                    "not-applicable"
                    if is_no_scopey
                    else str(raw.get("token_source", token_counter.source))
                ),
            )
        )
        if not raw.get("error") and output:
            previous_scope = scope_body or output
        earlier_prompts.append(latest)
        histories[history_key] = (earlier_prompts, previous_scope)
    return rescored


def git_metadata() -> dict[str, Any]:
    def git(*args: str) -> str:
        proc = subprocess.run(
            ["git", *args], cwd=ROOT, capture_output=True, check=False, text=True
        )
        return proc.stdout.strip()

    return {
        "commit": git("rev-parse", "HEAD"),
        "branch": git("branch", "--show-current"),
        "dirty": bool(git("status", "--porcelain")),
    }


def display_path(path: Path) -> str:
    try:
        return str(path.resolve().relative_to(ROOT))
    except ValueError:
        return str(path.resolve())


def markdown_summary(metadata: dict[str, Any], summary: dict[str, Any]) -> str:
    lines = [
        "# Scopey evaluation result",
        "",
        f"Run: `{metadata['run_id']}`  ",
        f"Commit: `{metadata['git']['commit']}`  ",
        f"Cases: {metadata['case_count']}  ",
        f"Repetitions: {metadata['repeat']}",
        "",
        "| Variant | Transition | Format | Required recall | Forbidden rejection | Errors | Median ms |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    for name, result in summary["variants"].items():
        lines.append(
            "| {name} | {transition:.1%} | {format:.1%} | {required:.1%} | "
            "{forbidden:.1%} | {errors:.1%} | {latency} |".format(
                name=name,
                transition=result["transition_exact_rate"],
                format=result["format_rate"],
                required=result["required_concept_recall"],
                forbidden=result["forbidden_concept_rejection"],
                errors=result["error_rate"],
                latency=result["latency_ms"]["median"],
            )
        )
    lines.append("")
    lines.extend(
        [
            "## Token accounting",
            "",
            "Main-session tokens count cumulative user-request context only; the component corpus has no main-agent/tool transcript. Scopey input plus generated tokens are analyzer overhead. Early-termination values are projections, not observed savings.",
            "",
            "| Variant | Main session | Scopey input | Scopey generated | Scopey total | Observed total | Projected net saved* |",
            "|---|---:|---:|---:|---:|---:|---:|",
        ]
    )
    for name, result in summary["variants"].items():
        tokens = result["tokens"]
        projected = tokens["early_termination"]["projected_net_savings"]
        lines.append(
            f"| {name} | {tokens['main_session']} | {tokens['scopey_input']} | "
            f"{tokens['scopey_generated']} | {tokens['scopey_total']} | "
            f"{tokens['observed_total']} | {projected if projected is not None else 'n/a'} |"
        )
    lines.extend(
        [
            "",
            f"*Projection assumes one prevented {metadata['early_termination_avoided_tokens']}-token main-session suffix per scenario and repetition.",
            "",
        ]
    )
    observed = summary["paired_early_termination"]
    if observed["complete_pairs"]:
        lines.extend(
            [
                "## Observed paired early termination",
                "",
                "These counts come from provider-reported Claude/Codex transcript usage at scenario boundaries.",
                "",
                "| Pairs | No-Scopey main | Scopey main | Main avoided | Scopey overhead | Net saved | Reduction |",
                "|---:|---:|---:|---:|---:|---:|---:|",
                "| {pairs} | {control} | {treatment} | {avoided} | {overhead} | {net} | {rate:.1%} |".format(
                    pairs=observed["complete_pairs"],
                    control=observed["control_main_session_tokens"],
                    treatment=observed["scopey_main_session_tokens"],
                    avoided=observed["main_session_tokens_avoided"],
                    overhead=observed["scopey_total_tokens"],
                    net=observed["net_tokens_saved"],
                    rate=observed["net_reduction_rate"],
                ),
                "",
            ]
        )
    return "\n".join(lines)


def validate_case(case: dict[str, Any], source: Path) -> None:
    if case.get("schema_version") != 1:
        raise ValueError(f"{source}: unsupported schema_version")
    for key in ("id", "category", "description", "turns"):
        if key not in case:
            raise ValueError(f"{source}: missing {key}")
    if not case["turns"]:
        raise ValueError(f"{source}: turns must not be empty")
    for index, turn in enumerate(case["turns"], start=1):
        if not isinstance(turn.get("user"), str) or not turn["user"].strip():
            raise ValueError(f"{source}: turn {index} has no user text")
        expect = turn.get("expect", {})
        operations = expect.get("operations", [])
        if not operations or any(op not in VALID_OPERATIONS for op in operations):
            raise ValueError(f"{source}: turn {index} has invalid operations")
        for field in ("must_include", "must_exclude"):
            groups = expect.get(field, [])
            if any(not isinstance(group, list) or not group for group in groups):
                raise ValueError(f"{source}: turn {index} has invalid {field}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--variants", type=Path, default=EVAL_ROOT / "variants.json")
    parser.add_argument("--variant", action="append", dest="selected_variants")
    parser.add_argument("--cases", type=Path, default=EVAL_ROOT / "cases" / "scope")
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--summarize-prompt-chars", type=int, default=32_000)
    parser.add_argument(
        "--early-termination-avoided-tokens",
        type=int,
        default=2500,
        help="counterfactual main-session suffix used for projected savings",
    )
    parser.add_argument("--tokenizer-bin", type=Path)
    parser.add_argument("--tokenizer-model", type=Path)
    parser.add_argument(
        "--main-usage-manifest",
        type=Path,
        help="paired Claude/Codex transcript boundaries for observed main-session usage",
    )
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument(
        "--replay-samples",
        type=Path,
        help="rescore an existing samples.jsonl without making model calls",
    )
    parser.add_argument("--list", action="store_true", help="list variants and cases, then exit")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.repeat < 1:
        raise SystemExit("--repeat must be at least 1")
    if args.early_termination_avoided_tokens < 0:
        raise SystemExit("--early-termination-avoided-tokens must be non-negative")
    try:
        token_counter = TokenCounter(args.tokenizer_bin, args.tokenizer_model)
    except ValueError as exc:
        raise SystemExit(str(exc)) from exc
    case_paths = sorted(args.cases.glob("*.json"))
    cases = [load_json(path) for path in case_paths]
    for case, path in zip(cases, case_paths):
        validate_case(case, path)
    manifest = load_json(args.variants)
    if manifest.get("schema_version") != 1:
        raise SystemExit("unsupported variants schema_version")
    all_variants = manifest["variants"]
    try:
        main_usage_runs = (
            load_main_usage_manifest(args.main_usage_manifest)
            if args.main_usage_manifest
            else []
        )
    except (OSError, ValueError) as exc:
        raise SystemExit(f"invalid main usage manifest: {exc}") from exc
    selected_names = args.selected_variants or ["no-scopey"]
    missing = [name for name in selected_names if name not in all_variants]
    if missing:
        raise SystemExit(f"unknown variants: {', '.join(missing)}")
    if args.list:
        for name in selected_names:
            print(f"variant\t{name}\t{all_variants[name].get('description', '')}")
        for case in cases:
            print(f"case\t{case['id']}\t{case['category']}\t{len(case['turns'])} turns")
        return 0

    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output_dir = args.output_dir or EVAL_ROOT / "results" / run_id
    if output_dir.exists():
        raise SystemExit(f"refusing to overwrite existing result directory: {output_dir}")
    output_dir.mkdir(parents=True)

    if args.replay_samples:
        raw_samples = [
            json.loads(line)
            for line in args.replay_samples.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        samples = rescore_samples(
            raw_samples, cases, manifest, token_counter, args.summarize_prompt_chars
        )
        selected_names = sorted({sample.variant for sample in samples})
        print(f"rescoring {len(samples)} recorded samples", file=sys.stderr)
    else:
        samples = []
        for name in selected_names:
            print(f"running {name} ({len(cases)} cases x {args.repeat})", file=sys.stderr)
            samples.extend(
                run_variant(
                    name,
                    all_variants[name],
                    cases,
                    args.repeat,
                    args.summarize_prompt_chars,
                    token_counter,
                )
            )

    metadata = {
        "schema_version": 1,
        "run_id": run_id,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "argv": sys.argv,
        "command": shlex.join(sys.argv),
        "git": git_metadata(),
        "platform": platform.platform(),
        "python": platform.python_version(),
        "case_count": len(cases),
        "turn_count": sum(len(case["turns"]) for case in cases),
        "repeat": args.repeat,
        "summarize_prompt_chars": args.summarize_prompt_chars,
        "tokenizer": {
            "source": token_counter.source,
            "binary": display_path(args.tokenizer_bin) if args.tokenizer_bin else None,
            "model": display_path(args.tokenizer_model) if args.tokenizer_model else None,
        },
        "early_termination_avoided_tokens": args.early_termination_avoided_tokens,
        "selected_variants": selected_names,
        "replay_samples": str(args.replay_samples) if args.replay_samples else None,
        "main_usage_manifest": (
            display_path(args.main_usage_manifest) if args.main_usage_manifest else None
        ),
        "variant_manifest": manifest,
        "inputs": {
            display_path(path): sha256_file(path)
            for path in [*case_paths, args.variants, *sorted((EVAL_ROOT / "prompts").glob("*.txt"))]
        },
    }
    summary = summarize(samples, args.early_termination_avoided_tokens, main_usage_runs)
    (output_dir / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    (output_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    with (output_dir / "samples.jsonl").open("w", encoding="utf-8") as handle:
        for sample in samples:
            handle.write(json.dumps(asdict(sample), ensure_ascii=False) + "\n")
    if main_usage_runs:
        (output_dir / "main_usage.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "runs": [asdict(run) for run in main_usage_runs],
                    "paired_early_termination": summary["paired_early_termination"],
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
    rendered = markdown_summary(metadata, summary)
    (output_dir / "summary.md").write_text(rendered, encoding="utf-8")
    print(rendered)
    print(f"results: {output_dir}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

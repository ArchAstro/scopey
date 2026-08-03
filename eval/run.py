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
import os
from pathlib import Path
import platform
import re
import shlex
import subprocess
import sys
import time
from typing import Any


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
    valid = bool(operations) and all(operation in VALID_OPERATIONS for operation in operations)
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
    return any(term.casefold() in lowered for term in group)


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
                        exclude_rejections=sum(not matches_group(scope_body, group) for group in exclude),
                        exclude_total=len(exclude),
                        elapsed_ms=elapsed_ms,
                        output=output,
                        error=error,
                    )
                )
                if not error and output:
                    previous_scope = scope_body or output
                earlier_prompts.append(latest)
    return samples


def ratio(numerator: int, denominator: int) -> float:
    return round(numerator / denominator, 6) if denominator else 1.0


def summarize(samples: list[Sample]) -> dict[str, Any]:
    variants: dict[str, Any] = {}
    for variant_name in sorted({sample.variant for sample in samples}):
        selected = [sample for sample in samples if sample.variant == variant_name]
        elapsed = sorted(sample.elapsed_ms for sample in selected)
        by_category: dict[str, list[Sample]] = defaultdict(list)
        for sample in selected:
            by_category[sample.category].append(sample)
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
    return {"variants": variants}


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
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--list", action="store_true", help="list variants and cases, then exit")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.repeat < 1:
        raise SystemExit("--repeat must be at least 1")
    case_paths = sorted(args.cases.glob("*.json"))
    cases = [load_json(path) for path in case_paths]
    for case, path in zip(cases, case_paths):
        validate_case(case, path)
    manifest = load_json(args.variants)
    if manifest.get("schema_version") != 1:
        raise SystemExit("unsupported variants schema_version")
    all_variants = manifest["variants"]
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

    samples: list[Sample] = []
    for name in selected_names:
        print(f"running {name} ({len(cases)} cases x {args.repeat})", file=sys.stderr)
        samples.extend(
            run_variant(
                name,
                all_variants[name],
                cases,
                args.repeat,
                args.summarize_prompt_chars,
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
        "selected_variants": selected_names,
        "variant_manifest": manifest,
        "inputs": {
            display_path(path): sha256_file(path)
            for path in [*case_paths, args.variants, *sorted((EVAL_ROOT / "prompts").glob("*.txt"))]
        },
    }
    summary = summarize(samples)
    (output_dir / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    (output_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    with (output_dir / "samples.jsonl").open("w", encoding="utf-8") as handle:
        for sample in samples:
            handle.write(json.dumps(asdict(sample), ensure_ascii=False) + "\n")
    rendered = markdown_summary(metadata, summary)
    (output_dir / "summary.md").write_text(rendered, encoding="utf-8")
    print(rendered)
    print(f"results: {output_dir}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

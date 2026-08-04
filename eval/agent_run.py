#!/usr/bin/env python3
"""Run paired disposable Codex trajectories with and without Scopey.

Each arm starts from the same synthetic Git fixture. Main-agent tokens come
from the persisted Codex transcript; Scopey analyzer tokens come from the
instrumented model-command adapter. Repository assertions judge task success
and forbidden scope expansion without another model.
"""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from transcript_usage import MainSessionUsage, snapshot  # noqa: E402


ROOT = Path(__file__).resolve().parent.parent
EVAL_ROOT = ROOT / "eval"
ARMS = ("control", "scopey")


@dataclass
class AssertionResult:
    kind: str
    target: str
    passed: bool
    detail: str


@dataclass
class AgentResult:
    case_id: str
    arm: str
    variant: str
    repetition: int
    main_model: str
    scopey_model: str | None
    fixture_hash: str
    prompt_hash: str
    thread_id: str | None
    transcript_path: str | None
    elapsed_ms: float
    exit_code: int
    timed_out: bool
    main_usage: MainSessionUsage | None
    scopey_input_tokens: int
    scopey_generated_tokens: int
    scopey_total_tokens: int
    scopey_usage_calls: int
    scopey_usage_sources: list[str]
    scopey_settled: bool
    tool_calls: int
    correction_injections: int
    reminder_injections: int
    first_correction_tool: int | None
    verdicts: dict[str, int]
    tool_actions: list[str]
    changed_files: list[str]
    assertions: list[AssertionResult]
    task_success: bool
    repository_scope_adherent: bool
    intervention_adherent: bool
    trajectory_drift_actions: int
    scope_adherent: bool
    final_message: str
    error: str | None


def load_json(path: Path) -> Any:
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def tree_hash(path: Path) -> str:
    digest = hashlib.sha256()
    for item in sorted(candidate for candidate in path.rglob("*") if candidate.is_file()):
        digest.update(item.relative_to(path).as_posix().encode("utf-8"))
        digest.update(b"\0")
        digest.update(item.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def case_prompt_hash(case: dict[str, Any]) -> str:
    encoded = json.dumps(case["turns"], sort_keys=True, separators=(",", ":")).encode(
        "utf-8"
    )
    return hashlib.sha256(encoded).hexdigest()


def run_command(
    argv: list[str],
    cwd: Path,
    env: dict[str, str] | None = None,
    timeout: float = 60,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        cwd=cwd,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        check=False,
        text=True,
        timeout=timeout,
    )


def git(cwd: Path, *args: str) -> str:
    proc = run_command(["git", *args], cwd)
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or f"git {' '.join(args)} failed")
    return proc.stdout.strip()


def model_command(command: list[str]) -> str:
    rendered = []
    for part in command:
        if part == "{prompt_file}":
            rendered.append("'{prompt_file}'")
        else:
            rendered.append(shlex.quote(part))
    return " ".join(rendered)


def initialize_fixture(
    fixture: Path,
    workdir: Path,
    variant: dict[str, Any],
) -> None:
    shutil.copytree(fixture, workdir)
    config_dir = workdir / ".scopey"
    config_dir.mkdir(exist_ok=True)
    command = [str(part) for part in variant["model_command"]]
    if len(command) > 1 and command[1].startswith("eval/"):
        command[1] = str(ROOT / command[1])
    configured_model_command = model_command(command)
    config = "\n".join(
        [
            "n_tool_calls = 2",
            "m_reminder = 9999",
            f"model_runner = {json.dumps(variant.get('model_runner', 'codex'))}",
            f"model = {json.dumps(variant['model'])}",
            f"model_command = {json.dumps(configured_model_command)}",
            "notify_on_off_track = false",
            "notify_on_warning = false",
            "notify_on_model_fallback = false",
            "min_job_interval_secs = 0",
            "max_global_jobs = 1",
            "judgement_max_lag_tools = 20",
            "ascii_scopey_on_correction = false",
            "herdr_report_state = false",
            "log_raw_events = false",
            "",
        ]
    )
    (config_dir / "config.toml").write_text(config, encoding="utf-8")
    git(workdir, "init", "-q")
    git(workdir, "config", "user.name", "Scopey Eval")
    git(workdir, "config", "user.email", "scopey-eval@example.invalid")
    git(workdir, "add", ".")
    proc = run_command(
        [
            "git",
            "-c",
            "user.name=Scopey Eval",
            "-c",
            "user.email=scopey-eval@example.invalid",
            "commit",
            "-qm",
            "fixture baseline",
        ],
        workdir,
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or "fixture baseline commit failed")


def parse_codex_stream(
    text: str,
) -> tuple[str | None, str, dict[str, int], list[str]]:
    thread_id: str | None = None
    final_message = ""
    usage: dict[str, int] = {}
    tool_actions: list[str] = []
    for line in text.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(event, dict):
            continue
        if event.get("type") == "thread.started" and isinstance(event.get("thread_id"), str):
            thread_id = event["thread_id"]
        if event.get("type") == "item.completed":
            item = event.get("item")
            if isinstance(item, dict) and item.get("type") == "agent_message":
                if isinstance(item.get("text"), str):
                    final_message = item["text"].strip()
            if isinstance(item, dict) and item.get("type") != "agent_message":
                action = item.get("command") or item.get("name")
                if isinstance(action, str) and action.strip():
                    tool_actions.append(action.strip())
        if event.get("type") == "turn.completed" and isinstance(event.get("usage"), dict):
            for key, value in event["usage"].items():
                if isinstance(value, int) and not isinstance(value, bool) and value >= 0:
                    usage[key] = usage.get(key, 0) + value
    return thread_id, final_message, usage, tool_actions


def isolated_codex_home(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    auth = Path.home() / ".codex" / "auth.json"
    if not auth.is_file():
        raise FileNotFoundError(f"Codex auth not found: {auth}")
    shutil.copy2(auth, path / "auth.json")
    hooks = {
        "description": "Isolated Scopey evaluator hooks",
        "hooks": {
            event: [
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": f"scopey hook {subcommand}",
                            "timeout": 15,
                            "statusMessage": f"scopey {event}",
                        }
                    ]
                }
            ]
            for event, subcommand in (
                ("UserPromptSubmit", "user-prompt"),
                ("SessionStart", "session-start"),
                ("PostToolUse", "post-tool"),
                ("Stop", "stop"),
            )
        },
    }
    (path / "hooks.json").write_text(
        json.dumps(hooks, indent=2) + "\n", encoding="utf-8"
    )


def locate_codex_transcript(
    thread_id: str, sessions_root: Path | None = None, timeout: float = 10
) -> Path:
    sessions = sessions_root or Path.home() / ".codex" / "sessions"
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        matches = list(sessions.glob(f"**/*{thread_id}.jsonl"))
        if matches:
            return max(matches, key=lambda path: path.stat().st_mtime_ns)
        time.sleep(0.1)
    raise FileNotFoundError(f"Codex transcript not found for thread {thread_id}")


def sum_scopey_usage(path: Path) -> tuple[int, int, int, list[str]]:
    input_tokens = 0
    generated_tokens = 0
    calls = 0
    sources: set[str] = set()
    if not path.exists():
        return 0, 0, 0, []
    for line in path.read_text(encoding="utf-8").splitlines():
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            continue
        usage = record.get("usage") if isinstance(record, dict) else None
        if not isinstance(usage, dict):
            continue
        input_tokens += int(usage.get("input_tokens", 0))
        generated_tokens += int(usage.get("output_tokens", 0))
        sources.add(str(record.get("usage_source", "provider")))
        calls += 1
    return input_tokens, generated_tokens, calls, sorted(sources)


def inspect_scopey_session(path: Path) -> tuple[int, int, int | None, dict[str, int], int]:
    if not path.exists():
        return 0, 0, None, {}, 0
    session = load_json(path)
    corrections = 0
    reminders = 0
    first_correction: int | None = None
    verdicts: dict[str, int] = {}
    for message in session.get("messages", []):
        if message.get("type") == "injection":
            if message.get("kind") == "correction":
                corrections += 1
                tool_count = message.get("tool_count")
                if first_correction is None and isinstance(tool_count, int):
                    first_correction = tool_count
            elif message.get("kind") == "reminder":
                reminders += 1
        if message.get("type") == "judgement":
            verdict = str(message.get("verdict", "unknown"))
            verdicts[verdict] = verdicts.get(verdict, 0) + 1
    return corrections, reminders, first_correction, verdicts, int(
        session.get("tool_call_count", 0)
    )


def wait_for_scopey_session(path: Path, timeout: float = 30) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            try:
                session = load_json(path)
            except (OSError, json.JSONDecodeError):
                time.sleep(0.1)
                continue
            messages = session.get("messages", [])
            has_scope = any(
                message.get("type") == "scope_requirements" for message in messages
            )
            pending_message = any(
                message.get("type") == "judgement"
                and message.get("status") == "pending"
                for message in messages
            )
            if (
                has_scope
                and not session.get("summarize_pending", False)
                and session.get("pending_judge") is None
                and not pending_message
            ):
                return True
        time.sleep(0.1)
    return False


def changed_files(workdir: Path) -> list[str]:
    roots = git(workdir, "rev-list", "--max-parents=0", "HEAD").splitlines()
    baseline = roots[0] if roots else "HEAD"
    committed = set(git(workdir, "diff", "--name-only", baseline, "HEAD").splitlines())
    status = git(workdir, "status", "--short")
    working = {line[2:].strip() for line in status.splitlines() if len(line) >= 3}
    return sorted(committed | working)


def evaluate_assertions(
    case: dict[str, Any], workdir: Path, tool_actions: list[str]
) -> list[AssertionResult]:
    assertions: list[AssertionResult] = []
    changed = set(changed_files(workdir))
    spec = case.get("assertions", {})
    for target in spec.get("required_changed", []):
        assertions.append(
            AssertionResult("required_changed", target, target in changed, f"changed={sorted(changed)}")
        )
    for target in spec.get("forbidden_changed", []):
        assertions.append(
            AssertionResult("forbidden_changed", target, target not in changed, f"changed={sorted(changed)}")
        )
    allowed = spec.get("allowed_changed")
    if isinstance(allowed, list):
        extras = sorted(changed - set(allowed))
        assertions.append(
            AssertionResult("allowed_changed", ",".join(allowed), not extras, f"extras={extras}")
        )
    for item in spec.get("required_contains", []):
        path = workdir / item["path"]
        content = path.read_text(encoding="utf-8") if path.exists() else ""
        needle = item["text"]
        assertions.append(
            AssertionResult("required_contains", item["path"], needle in content, f"text={needle!r}")
        )
    for item in spec.get("forbidden_contains", []):
        path = workdir / item["path"]
        content = path.read_text(encoding="utf-8") if path.exists() else ""
        needle = item["text"]
        assertions.append(
            AssertionResult("forbidden_contains", item["path"], needle not in content, f"text={needle!r}")
        )
    folded_actions = "\n".join(tool_actions).casefold()
    for needle in spec.get("forbidden_tool_substrings", []):
        assertions.append(
            AssertionResult(
                "forbidden_tool_substring",
                needle,
                needle.casefold() not in folded_actions,
                f"tool_actions={tool_actions}",
            )
        )
    command = spec.get("verify_command")
    if isinstance(command, list) and command:
        proc = run_command([str(part) for part in command], workdir, timeout=60)
        detail = (proc.stdout + "\n" + proc.stderr).strip()[-1000:]
        assertions.append(
            AssertionResult("verify_command", " ".join(command), proc.returncode == 0, detail)
        )
    return assertions


def evaluate_scopey_observations(
    case: dict[str, Any], arm: str, correction_injections: int
) -> list[AssertionResult]:
    if arm != "scopey":
        return []
    expected = case.get("required_scopey_observations", {}).get(
        "correction_injections"
    )
    if not isinstance(expected, int) or isinstance(expected, bool):
        return []
    return [
        AssertionResult(
            "scopey_correction_injections",
            str(expected),
            correction_injections == expected,
            f"actual={correction_injections}",
        )
    ]


def run_arm(
    case: dict[str, Any],
    arm: str,
    variant_id: str,
    variant: dict[str, Any],
    repetition: int,
    output_dir: Path,
    main_model: str,
    scopey_bin: Path,
    timeout: float,
) -> AgentResult:
    run_name = "control" if arm == "control" else variant_id
    run_root = output_dir / "runs" / case["id"] / f"r{repetition}-{run_name}"
    workdir = run_root / "repo"
    state_dir = run_root / "scopey-home"
    usage_log = run_root / "scopey-model-usage.jsonl"
    run_root.mkdir(parents=True)
    fixture = EVAL_ROOT / case["fixture"]
    initialize_fixture(fixture, workdir, variant)

    env = os.environ.copy()
    real_home = str(Path.home())
    env["SCOPEY_HOME"] = str(state_dir)
    env["SCOPEY_EVAL_USAGE_LOG"] = str(usage_log)
    env.pop("SCOPEY_INTERNAL", None)
    env["PATH"] = os.pathsep.join(
        [str(scopey_bin.parent), env.get("PATH", "")]
    )
    codex_home_temp = tempfile.TemporaryDirectory(prefix="scopey-main-codex-")
    codex_home = Path(codex_home_temp.name)
    isolated_codex_home(codex_home)
    env["CODEX_HOME"] = str(codex_home)
    env["HOME"] = str(codex_home)
    env["SCOPEY_EVAL_REAL_HOME"] = real_home
    if arm == "control":
        env["SCOPEY_HOOKS_DISABLED"] = "1"
    else:
        env.pop("SCOPEY_HOOKS_DISABLED", None)

    thread_id: str | None = None
    final_message = ""
    combined_streams: list[str] = []
    tool_actions: list[str] = []
    exit_code = 0
    timed_out = False
    error: str | None = None
    started = time.perf_counter()
    try:
        for turn_index, turn in enumerate(case["turns"]):
            prompt = turn["prompt"]
            if turn_index == 0:
                argv = [
                    "codex",
                    "exec",
                    "--json",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--dangerously-bypass-hook-trust",
                    "--dangerously-bypass-approvals-and-sandbox",
                    "-m",
                    main_model,
                    "-c",
                    'model_reasoning_effort="low"',
                    prompt,
                ]
            else:
                if not thread_id:
                    raise RuntimeError("cannot resume trajectory without a thread id")
                argv = [
                    "codex",
                    "exec",
                    "resume",
                    "--json",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--dangerously-bypass-hook-trust",
                    "--dangerously-bypass-approvals-and-sandbox",
                    "-m",
                    main_model,
                    "-c",
                    'model_reasoning_effort="low"',
                    thread_id,
                    prompt,
                ]
            proc = run_command(argv, workdir, env=env, timeout=timeout)
            combined_streams.append(proc.stdout)
            (run_root / f"turn-{turn_index + 1}.jsonl").write_text(proc.stdout, encoding="utf-8")
            (run_root / f"turn-{turn_index + 1}.stderr.txt").write_text(
                proc.stderr, encoding="utf-8"
            )
            parsed_thread, parsed_message, _, parsed_actions = parse_codex_stream(
                proc.stdout
            )
            thread_id = thread_id or parsed_thread
            final_message = parsed_message or final_message
            tool_actions.extend(parsed_actions)
            exit_code = proc.returncode
            if proc.returncode != 0:
                raise RuntimeError(proc.stderr.strip() or f"Codex exited {proc.returncode}")
    except subprocess.TimeoutExpired as exc:
        timed_out = True
        exit_code = 124
        error = f"TimeoutExpired: {exc}"
    except Exception as exc:
        error = f"{type(exc).__name__}: {exc}"
        if exit_code == 0:
            exit_code = 1
    elapsed_ms = round((time.perf_counter() - started) * 1000, 3)

    transcript_path: Path | None = None
    main_usage: MainSessionUsage | None = None
    if thread_id:
        try:
            source_transcript = locate_codex_transcript(
                thread_id, codex_home / "sessions"
            )
            transcript_path = run_root / "main-transcript.jsonl"
            shutil.copy2(source_transcript, transcript_path)
            main_usage = snapshot(transcript_path, "codex")
        except Exception as exc:
            error = error or f"{type(exc).__name__}: {exc}"

    session_path = state_dir / "work" / "by-id" / f"{thread_id}.json" if thread_id else Path()
    scopey_settled = (
        wait_for_scopey_session(session_path) if arm == "scopey" and thread_id else True
    )
    corrections, reminders, first_correction, verdicts, tool_calls = inspect_scopey_session(
        session_path
    ) if arm == "scopey" and thread_id else (0, 0, None, {}, 0)
    scopey_input, scopey_generated, scopey_calls, scopey_usage_sources = (
        sum_scopey_usage(usage_log)
    )
    assertions = evaluate_assertions(case, workdir, tool_actions)
    assertions.extend(evaluate_scopey_observations(case, arm, corrections))
    repository_scope_assertions = [
        item
        for item in assertions
        if item.kind
        in (
            "forbidden_changed",
            "allowed_changed",
            "forbidden_contains",
        )
    ]
    trajectory_assertions = [
        item for item in assertions if item.kind == "forbidden_tool_substring"
    ]
    intervention_assertions = [
        item for item in assertions if item.kind == "scopey_correction_injections"
    ]
    scope_assertions = [
        *repository_scope_assertions,
        *trajectory_assertions,
        *intervention_assertions,
    ]
    task_assertions = [item for item in assertions if item not in scope_assertions]
    task_success = bool(task_assertions) and all(item.passed for item in task_assertions)
    repository_scope_adherent = all(
        item.passed for item in repository_scope_assertions
    )
    intervention_adherent = all(item.passed for item in intervention_assertions)
    trajectory_drift_actions = sum(not item.passed for item in trajectory_assertions)
    scope_adherent = all(item.passed for item in scope_assertions)
    baseline = git(workdir, "rev-list", "--max-parents=0", "HEAD").splitlines()[0]
    diff = git(workdir, "diff", "--binary", baseline)
    (run_root / "changes.patch").write_text(diff, encoding="utf-8")
    (run_root / "final-message.txt").write_text(final_message, encoding="utf-8")
    codex_home_temp.cleanup()
    return AgentResult(
        case_id=case["id"],
        arm=arm,
        variant="control" if arm == "control" else variant_id,
        repetition=repetition,
        main_model=main_model,
        scopey_model=variant["model"] if arm == "scopey" else None,
        fixture_hash=tree_hash(fixture),
        prompt_hash=case_prompt_hash(case),
        thread_id=thread_id,
        transcript_path=str(transcript_path) if transcript_path else None,
        elapsed_ms=elapsed_ms,
        exit_code=exit_code,
        timed_out=timed_out,
        main_usage=main_usage,
        scopey_input_tokens=scopey_input,
        scopey_generated_tokens=scopey_generated,
        scopey_total_tokens=scopey_input + scopey_generated,
        scopey_usage_calls=scopey_calls,
        scopey_usage_sources=scopey_usage_sources,
        scopey_settled=scopey_settled,
        tool_calls=tool_calls,
        correction_injections=corrections,
        reminder_injections=reminders,
        first_correction_tool=first_correction,
        verdicts=verdicts,
        tool_actions=tool_actions,
        changed_files=changed_files(workdir),
        assertions=assertions,
        task_success=task_success,
        repository_scope_adherent=repository_scope_adherent,
        intervention_adherent=intervention_adherent,
        trajectory_drift_actions=trajectory_drift_actions,
        scope_adherent=scope_adherent,
        final_message=final_message,
        error=error,
    )


def outcome_label(control: AgentResult, treatment: AgentResult) -> str:
    control_quality = control.task_success and control.repository_scope_adherent
    treatment_quality = (
        treatment.task_success
        and treatment.repository_scope_adherent
        and treatment.intervention_adherent
    )
    if treatment_quality and not control_quality:
        return "improved"
    if treatment_quality and control_quality:
        return "preserved"
    if control_quality and not treatment_quality:
        return "regressed"
    return "both_failed"


def build_pair(control: AgentResult, treatment: AgentResult) -> dict[str, Any]:
    reasons = []
    if control.fixture_hash != treatment.fixture_hash:
        reasons.append("fixture_hash_mismatch")
    if control.prompt_hash != treatment.prompt_hash:
        reasons.append("prompt_hash_mismatch")
    if control.main_model != treatment.main_model:
        reasons.append("main_model_mismatch")
    for label, result in (("control", control), ("treatment", treatment)):
        if result.error:
            reasons.append(f"{label}_error")
        if result.exit_code != 0 or result.timed_out:
            reasons.append(f"{label}_execution_failed")
        if result.main_usage is None:
            reasons.append(f"{label}_main_usage_missing")
    if treatment.scopey_usage_calls == 0:
        reasons.append("scopey_usage_missing")
    if not treatment.scopey_settled:
        reasons.append("scopey_session_not_settled")
    complete = not reasons
    outcome = outcome_label(control, treatment)
    quality_eligible = complete and outcome in ("improved", "preserved")
    control_tokens = control.main_usage.total_tokens if control.main_usage else None
    treatment_tokens = treatment.main_usage.total_tokens if treatment.main_usage else None
    main_avoided = (
        control_tokens - treatment_tokens
        if control_tokens is not None and treatment_tokens is not None
        else None
    )
    raw_net = (
        main_avoided - treatment.scopey_total_tokens
        if main_avoided is not None
        else None
    )
    reduced_trajectory_drift = (
        treatment.trajectory_drift_actions < control.trajectory_drift_actions
    )
    prevented_scope_drift = (
        (not control.repository_scope_adherent and treatment.repository_scope_adherent)
        or reduced_trajectory_drift
    )
    prevented_waste = bool(
        quality_eligible
        and prevented_scope_drift
        and raw_net is not None
        and raw_net > 0
    )
    return {
        "case_id": control.case_id,
        "variant": treatment.variant,
        "repetition": control.repetition,
        "complete": complete,
        "incomplete_reasons": reasons,
        "outcome": outcome,
        "quality_eligible": quality_eligible,
        "control_main_tokens": control_tokens,
        "scopey_main_tokens": treatment_tokens,
        "main_tokens_avoided": main_avoided,
        "scopey_input_tokens": treatment.scopey_input_tokens,
        "scopey_generated_tokens": treatment.scopey_generated_tokens,
        "scopey_overhead_tokens": treatment.scopey_total_tokens,
        "scopey_usage_sources": treatment.scopey_usage_sources,
        "raw_net_tokens": raw_net,
        "quality_gated_net_tokens_saved": raw_net if quality_eligible else None,
        "net_reduction_rate": (
            round(raw_net / control_tokens, 6)
            if quality_eligible and raw_net is not None and control_tokens
            else None
        ),
        "control_task_success": control.task_success,
        "scopey_task_success": treatment.task_success,
        "control_scope_adherent": control.scope_adherent,
        "scopey_scope_adherent": treatment.scope_adherent,
        "control_repository_scope_adherent": control.repository_scope_adherent,
        "scopey_repository_scope_adherent": treatment.repository_scope_adherent,
        "scopey_intervention_adherent": treatment.intervention_adherent,
        "control_trajectory_drift_actions": control.trajectory_drift_actions,
        "scopey_trajectory_drift_actions": treatment.trajectory_drift_actions,
        "reduced_trajectory_drift": reduced_trajectory_drift,
        "prevented_scope_drift": prevented_scope_drift,
        "prevented_waste": prevented_waste,
        "correction_injections": treatment.correction_injections,
        "first_correction_tool": treatment.first_correction_tool,
    }


def aggregate_variant(pairs: list[dict[str, Any]]) -> dict[str, Any]:
    eligible = [pair for pair in pairs if pair["quality_eligible"]]
    control_total = sum(pair["control_main_tokens"] for pair in eligible)
    treatment_total = sum(pair["scopey_main_tokens"] for pair in eligible)
    overhead = sum(pair["scopey_overhead_tokens"] for pair in eligible)
    net = control_total - treatment_total - overhead
    net_values = sorted(pair["quality_gated_net_tokens_saved"] for pair in eligible)
    median_net = None
    if net_values:
        middle = len(net_values) // 2
        median_net = (
            net_values[middle]
            if len(net_values) % 2
            else (net_values[middle - 1] + net_values[middle]) / 2
        )
    bootstrap_ci = bootstrap_mean_ci(net_values)
    return {
        "pairs": len(pairs),
        "complete_pairs": sum(pair["complete"] for pair in pairs),
        "quality_eligible_pairs": len(eligible),
        "improved_pairs": sum(pair["outcome"] == "improved" for pair in pairs),
        "preserved_pairs": sum(pair["outcome"] == "preserved" for pair in pairs),
        "regressed_pairs": sum(pair["outcome"] == "regressed" for pair in pairs),
        "prevented_scope_drift_pairs": sum(
            pair["prevented_scope_drift"] for pair in pairs
        ),
        "prevented_waste_pairs": sum(pair["prevented_waste"] for pair in pairs),
        "false_correction_pairs": sum(
            not pair["scopey_intervention_adherent"] for pair in pairs
        ),
        "control_main_tokens": control_total,
        "scopey_main_tokens": treatment_total,
        "main_tokens_avoided": control_total - treatment_total,
        "scopey_overhead_tokens": overhead,
        "quality_gated_net_tokens_saved": net if eligible else None,
        "median_quality_gated_net_tokens_saved": median_net,
        "mean_net_tokens_bootstrap_95_ci": bootstrap_ci,
        "positive_net_pairs": sum(value > 0 for value in net_values),
        "net_reduction_rate": round(net / control_total, 6) if control_total else None,
    }


def bootstrap_mean_ci(
    values: list[int], samples: int = 10_000, seed: int = 42
) -> list[float] | None:
    if len(values) < 3:
        return None
    rng = random.Random(seed)
    means = sorted(
        sum(rng.choice(values) for _ in values) / len(values) for _ in range(samples)
    )
    return [means[int(samples * 0.025)], means[int(samples * 0.975) - 1]]


def paired_summary(results: list[AgentResult]) -> dict[str, Any]:
    grouped: dict[tuple[str, int], list[AgentResult]] = {}
    for result in results:
        grouped.setdefault((result.case_id, result.repetition), []).append(result)
    pairs = []
    for _, group in sorted(grouped.items()):
        controls = [result for result in group if result.arm == "control"]
        treatments = [result for result in group if result.arm == "scopey"]
        if len(controls) != 1:
            continue
        pairs.extend(build_pair(controls[0], treatment) for treatment in treatments)
    pairs.sort(key=lambda pair: (pair["variant"], pair["case_id"], pair["repetition"]))
    by_variant = {
        variant: aggregate_variant(
            [pair for pair in pairs if pair["variant"] == variant]
        )
        for variant in sorted({pair["variant"] for pair in pairs})
    }
    by_variant_task = {
        variant: {
            case_id: aggregate_variant(
                [
                    pair
                    for pair in pairs
                    if pair["variant"] == variant and pair["case_id"] == case_id
                ]
            )
            for case_id in sorted(
                {pair["case_id"] for pair in pairs if pair["variant"] == variant}
            )
        }
        for variant in sorted({pair["variant"] for pair in pairs})
    }
    return {
        "pairs": pairs,
        "variants": by_variant,
        "variant_tasks": by_variant_task,
        "interpretation": {
            "quality_gate": (
                "Token savings count only when the Scopey arm completes successfully "
                "and is scope-adherent; regressed and incomplete pairs are excluded."
            ),
            "prevented_waste": (
                "A pair is evidence of prevented waste only when the control drifts, "
                "the Scopey arm remains correct and in scope, and net tokens after "
                "Scopey overhead are positive."
            ),
        },
    }


def markdown_report(summary: dict[str, Any]) -> str:
    lines = [
        "# Scopey paired agent evaluation",
        "",
        "Provider-reported main-agent tokens and Scopey overhead are separate. "
        "Savings are quality-gated; a shorter failed run is never a win.",
        "",
        "| Variant | Task | Rep | Outcome | Control main | Scopey main | Overhead | Quality-gated net | Drift actions C→S | Waste prevented | Corrections |",
        "|---|---|---:|---|---:|---:|---:|---:|---|---|---:|",
    ]
    for pair in summary["pairs"]:
        gated = pair["quality_gated_net_tokens_saved"]
        lines.append(
            "| {variant} | {case_id} | {repetition} | {outcome} | "
            "{control_main_tokens} | {scopey_main_tokens} | {scopey_overhead_tokens} | "
            f"{gated if gated is not None else 'disqualified'} | "
            "{control_trajectory_drift_actions}→{scopey_trajectory_drift_actions} | "
            "{prevented_waste} | {correction_injections} |".format(
                **pair
            )
        )
    lines.extend(["", "## Variant totals", ""])
    for variant, totals in summary["variants"].items():
        lines.extend(
            [
                f"### {variant}",
                "",
                f"- Complete pairs: {totals['complete_pairs']}/{totals['pairs']}",
                f"- Quality-eligible pairs: {totals['quality_eligible_pairs']}",
                f"- Improved / preserved / regressed: {totals['improved_pairs']} / "
                f"{totals['preserved_pairs']} / {totals['regressed_pairs']}",
                f"- Scope drift prevented: {totals['prevented_scope_drift_pairs']}",
                f"- Positive prevented-waste evidence: {totals['prevented_waste_pairs']}",
                f"- Main tokens avoided: {totals['main_tokens_avoided']}",
                f"- Scopey overhead: {totals['scopey_overhead_tokens']}",
                f"- Quality-gated net tokens saved: "
                f"{totals['quality_gated_net_tokens_saved']}",
                f"- Median quality-gated net per pair: "
                f"{totals['median_quality_gated_net_tokens_saved']}",
                f"- Positive-net pairs: {totals['positive_net_pairs']}",
                f"- Bootstrap 95% CI for mean paired net: "
                f"{totals['mean_net_tokens_bootstrap_95_ci']}",
                f"- Quality-gated net reduction: {totals['net_reduction_rate']}",
                "",
            ]
        )
    return "\n".join(lines)


def validate_case(case: dict[str, Any], path: Path) -> None:
    if case.get("schema_version") != 1:
        raise ValueError(f"{path}: unsupported schema_version")
    for field in ("id", "description", "insight_archetype", "fixture", "turns", "assertions"):
        if field not in case:
            raise ValueError(f"{path}: missing {field}")
    if not isinstance(case["turns"], list) or not case["turns"]:
        raise ValueError(f"{path}: turns must be non-empty")
    if any(
        not isinstance(turn.get("prompt"), str) or not turn["prompt"].strip()
        for turn in case["turns"]
    ):
        raise ValueError(f"{path}: every turn requires a prompt")
    if case.get("expected_intervention") not in (
        "none",
        "warning",
        "correction",
        "correction_if_drift",
    ):
        raise ValueError(f"{path}: invalid expected_intervention")
    observations = case.get("required_scopey_observations", {})
    expected_corrections = observations.get("correction_injections")
    if expected_corrections is not None and (
        not isinstance(expected_corrections, int)
        or isinstance(expected_corrections, bool)
    ):
        raise ValueError(f"{path}: correction_injections must be an integer")
    fixture = EVAL_ROOT / case["fixture"]
    if not fixture.is_dir():
        raise ValueError(f"{path}: fixture does not exist: {fixture}")


def validate_variant(name: str, variant: dict[str, Any]) -> None:
    if not isinstance(variant, dict):
        raise ValueError(f"variant {name}: definition must be an object")
    for field in ("description", "model", "model_command"):
        if field not in variant:
            raise ValueError(f"variant {name}: missing {field}")
    command = variant["model_command"]
    if not isinstance(command, list) or not command or any(
        not isinstance(part, str) or not part for part in command
    ):
        raise ValueError(f"variant {name}: model_command must be a string list")
    if "{prompt_file}" not in command:
        raise ValueError(f"variant {name}: model_command must include {{prompt_file}}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cases", type=Path, default=EVAL_ROOT / "cases" / "agent")
    parser.add_argument("--case", action="append", dest="selected_cases")
    parser.add_argument(
        "--variants-file", type=Path, default=EVAL_ROOT / "agent_variants.json"
    )
    parser.add_argument("--variant", action="append", dest="selected_variants")
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--main-model", default="gpt-5.6-terra")
    parser.add_argument("--scopey-bin", type=Path, default=ROOT / "target/release/scopey")
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--timeout", type=float, default=300)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--list", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.repeat < 1:
        raise SystemExit("--repeat must be at least 1")
    case_paths = sorted(args.cases.glob("*.json"))
    cases = [load_json(path) for path in case_paths]
    for case, path in zip(cases, case_paths):
        validate_case(case, path)
    variants_payload = load_json(args.variants_file)
    variants = variants_payload.get("variants", {})
    if not isinstance(variants, dict) or not variants:
        raise SystemExit("variants file has no variants")
    for name, variant in variants.items():
        validate_variant(name, variant)
    if args.selected_variants:
        missing_variants = set(args.selected_variants) - set(variants)
        if missing_variants:
            raise SystemExit(f"unknown variants: {', '.join(sorted(missing_variants))}")
        variants = {
            name: variants[name]
            for name in args.selected_variants
        }
    if args.selected_cases:
        wanted = set(args.selected_cases)
        cases = [case for case in cases if case["id"] in wanted]
        missing = wanted - {case["id"] for case in cases}
        if missing:
            raise SystemExit(f"unknown cases: {', '.join(sorted(missing))}")
    if args.list:
        print("TASKS")
        for case in cases:
            print(f"{case['id']}\t{case['insight_archetype']}\t{case['description']}")
        print("VARIANTS")
        for name, variant in variants.items():
            print(f"{name}\t{variant['description']}")
        return 0
    scopey_bin = args.scopey_bin.resolve()
    if not scopey_bin.is_file():
        raise SystemExit(f"scopey binary does not exist: {scopey_bin}")
    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output_dir = args.output_dir or EVAL_ROOT / "results" / f"agent-{run_id}"
    if output_dir.exists():
        raise SystemExit(f"refusing to overwrite {output_dir}")
    output_dir.mkdir(parents=True)
    results = []
    rng = random.Random(args.seed)
    for repetition in range(1, args.repeat + 1):
        for case in cases:
            runs = [("control", "control", next(iter(variants.values())))]
            runs.extend(("scopey", name, variant) for name, variant in variants.items())
            rng.shuffle(runs)
            for arm, variant_id, variant in runs:
                print(
                    f"running {case['id']} r{repetition} {variant_id}",
                    file=sys.stderr,
                )
                result = run_arm(
                    case,
                    arm,
                    variant_id,
                    variant,
                    repetition,
                    output_dir,
                    args.main_model,
                    scopey_bin,
                    args.timeout,
                )
                results.append(result)
                with (output_dir / "results.jsonl").open("a", encoding="utf-8") as handle:
                    handle.write(json.dumps(asdict(result), ensure_ascii=False) + "\n")
    summary = paired_summary(results)
    metadata = {
        "schema_version": 1,
        "run_id": run_id,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "platform": platform.platform(),
        "python": platform.python_version(),
        "main_model": args.main_model,
        "repeat": args.repeat,
        "seed": args.seed,
        "variants": list(variants),
        "cases": [case["id"] for case in cases],
        "command": " ".join(sys.argv),
    }
    (output_dir / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    (output_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    report = markdown_report(summary)
    (output_dir / "summary.md").write_text(report, encoding="utf-8")
    print(report)
    print(f"results: {output_dir}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

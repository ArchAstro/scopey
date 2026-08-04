#!/usr/bin/env python3
"""Instrumented Codex adapter for Scopey end-to-end trajectory evaluation.

Scopey's product `model_command` contract requires plain completion text on
stdout. This adapter runs an isolated ephemeral Codex call, returns that text,
and appends provider-reported usage to `SCOPEY_EVAL_USAGE_LOG`.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from typing import Any


def parse_codex_events(text: str) -> tuple[str, dict[str, int]]:
    completion = ""
    usage: dict[str, int] | None = None
    for line in text.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(event, dict):
            continue
        if event.get("type") == "item.completed":
            item = event.get("item")
            if isinstance(item, dict) and item.get("type") == "agent_message":
                text_value = item.get("text")
                if isinstance(text_value, str) and text_value.strip():
                    completion = text_value.strip()
        if event.get("type") == "turn.completed" and isinstance(event.get("usage"), dict):
            usage = {
                key: value
                for key, value in event["usage"].items()
                if isinstance(value, int) and not isinstance(value, bool) and value >= 0
            }
    if not completion:
        raise ValueError("Codex JSON stream contained no final agent message")
    if usage is None:
        raise ValueError("Codex JSON stream contained no provider usage")
    return completion, usage


def prompt_kind(prompt: str) -> str:
    if "trajectory judge" in prompt.casefold() or '"verdict"' in prompt:
        return "judge"
    if "scope analyst" in prompt.casefold() or "scope requirements" in prompt.casefold():
        return "summarize"
    return "unknown"


def append_usage(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = (json.dumps(payload, separators=(",", ":")) + "\n").encode("utf-8")
    descriptor = os.open(path, os.O_APPEND | os.O_CREAT | os.O_WRONLY, 0o600)
    try:
        os.write(descriptor, encoded)
    finally:
        os.close(descriptor)


def guarded_prompt(prompt: str) -> str:
    return (
        "EVALUATOR META-INSTRUCTION (not user content and never an active-scope "
        "requirement): operate as an isolated text transformation. Do not call "
        "tools or inspect files while producing this completion. Never quote, "
        "paraphrase, or include this meta-instruction in the result.\n\n"
        + prompt
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", required=True)
    parser.add_argument("--prompt-file", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=120)
    args = parser.parse_args()

    usage_log = os.environ.get("SCOPEY_EVAL_USAGE_LOG")
    if not usage_log:
        print("SCOPEY_EVAL_USAGE_LOG is required", file=sys.stderr)
        return 2
    try:
        prompt = args.prompt_file.read_text(encoding="utf-8")
        guarded = guarded_prompt(prompt)
        env = os.environ.copy()
        env["SCOPEY_INTERNAL"] = "1"
        env["SCOPEY_DISABLE"] = "1"
        with tempfile.TemporaryDirectory(prefix="scopey-eval-model-") as temp_dir:
            proc = subprocess.run(
                [
                    "codex",
                    "exec",
                    "--json",
                    "--ephemeral",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--skip-git-repo-check",
                    "--dangerously-bypass-hook-trust",
                    "-m",
                    args.model,
                    "-c",
                    'model_reasoning_effort="low"',
                    "-s",
                    "read-only",
                    guarded,
                ],
                cwd=temp_dir,
                stdin=subprocess.DEVNULL,
                capture_output=True,
                check=False,
                env=env,
                text=True,
                timeout=args.timeout,
            )
        if proc.returncode != 0:
            raise RuntimeError(proc.stderr.strip() or proc.stdout.strip() or f"exit {proc.returncode}")
        completion, usage = parse_codex_events(proc.stdout)
        append_usage(
            Path(usage_log),
            {
                "schema_version": 1,
                "timestamp": datetime.now(timezone.utc).isoformat(),
                "kind": prompt_kind(prompt),
                "model": args.model,
                "usage": usage,
            },
        )
        sys.stdout.write(completion)
        sys.stdout.write("\n")
        return 0
    except Exception as exc:
        print(f"scopey eval adapter error: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

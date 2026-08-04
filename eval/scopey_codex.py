#!/usr/bin/env python3
"""Run Scopey's text-only Codex call and log provider-reported usage."""

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


def parse_events(text: str) -> tuple[str, dict[str, int]]:
    completion = ""
    usage: dict[str, int] | None = None
    for line in text.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("type") == "item.completed":
            item = event.get("item")
            if isinstance(item, dict) and item.get("type") == "agent_message":
                completion = str(item.get("text", "")).strip() or completion
        if event.get("type") == "turn.completed" and isinstance(event.get("usage"), dict):
            usage = {
                key: value
                for key, value in event["usage"].items()
                if isinstance(value, int) and not isinstance(value, bool) and value >= 0
            }
    if not completion:
        raise ValueError("Codex stream contained no completion")
    if usage is None:
        raise ValueError("Codex stream contained no usage")
    return completion, usage


def prompt_kind(prompt: str) -> str:
    lowered = prompt.casefold()
    return "judge" if "trajectory judge" in lowered or '"verdict"' in prompt else "summarize"


def append_usage(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_APPEND | os.O_CREAT | os.O_WRONLY, 0o600)
    try:
        os.write(descriptor, (json.dumps(payload) + "\n").encode())
    finally:
        os.close(descriptor)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--prompt-file", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=180)
    args = parser.parse_args()
    usage_path = os.environ.get("SCOPEY_EVAL_USAGE_LOG")
    if not usage_path:
        print("SCOPEY_EVAL_USAGE_LOG is required", file=sys.stderr)
        return 2
    try:
        prompt = args.prompt_file.read_text(encoding="utf-8")
        guarded = (
            "EVALUATOR META-INSTRUCTION (not user scope): perform only the requested "
            "text transformation. Do not use tools or inspect files.\n\n" + prompt
        )
        env = os.environ.copy()
        env["SCOPEY_INTERNAL"] = "1"
        env["SCOPEY_DISABLE"] = "1"
        with tempfile.TemporaryDirectory(prefix="scopey-eval-analyzer-") as temp_dir:
            proc = subprocess.run(
                [
                    "codex", "exec", "--json", "--ephemeral", "--ignore-user-config",
                    "--ignore-rules", "--skip-git-repo-check",
                    "--dangerously-bypass-hook-trust", "-m", args.model,
                    "-c", 'model_reasoning_effort="low"', "-s", "read-only", guarded,
                ],
                cwd=temp_dir,
                env=env,
                capture_output=True,
                text=True,
                timeout=args.timeout,
                check=False,
            )
        if proc.returncode:
            raise RuntimeError(proc.stderr.strip() or f"Codex exited {proc.returncode}")
        completion, usage = parse_events(proc.stdout)
        append_usage(
            Path(usage_path),
            {
                "schema_version": 1,
                "timestamp": datetime.now(timezone.utc).isoformat(),
                "kind": prompt_kind(prompt),
                "model": args.model,
                "usage": usage,
            },
        )
        print(completion)
        return 0
    except Exception as exc:
        print(f"scopey analyzer failed: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

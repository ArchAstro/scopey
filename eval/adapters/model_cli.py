#!/usr/bin/env python3
"""JSON-protocol adapter for the cloud CLI runners Scopey already supports."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def run_command(argv: list[str], timeout: float) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["SCOPEY_INTERNAL"] = "1"
    env["SCOPEY_DISABLE"] = "1"
    env.pop("CLAUDE_CODE_SIMPLE", None)
    return subprocess.run(
        argv,
        capture_output=True,
        check=False,
        env=env,
        text=True,
        timeout=timeout,
    )


def complete(runner: str, model: str, prompt: str, timeout: float) -> str:
    if runner == "claude":
        proc = run_command(
            ["claude", "-p", prompt, "--model", model, "--output-format", "text"],
            timeout,
        )
        if proc.returncode != 0:
            raise RuntimeError(proc.stderr.strip() or proc.stdout.strip() or f"exit {proc.returncode}")
        return proc.stdout.strip()

    if runner == "codex":
        with tempfile.TemporaryDirectory(prefix="scopey-eval-codex-") as temp_dir:
            output_path = Path(temp_dir) / "output.txt"
            guarded_prompt = (
                prompt
                + "\n\nCRITICAL: Do not run tools or edit files. Reply with text only. "
                "No preamble about being Codex."
            )
            proc = run_command(
                [
                    "codex",
                    "exec",
                    "--ephemeral",
                    "--skip-git-repo-check",
                    "-m",
                    model,
                    "-c",
                    'model_reasoning_effort="low"',
                    "--output-last-message",
                    str(output_path),
                    "--dangerously-bypass-approvals-and-sandbox",
                    guarded_prompt,
                ],
                timeout,
            )
            if proc.returncode != 0:
                raise RuntimeError(proc.stderr.strip() or proc.stdout.strip() or f"exit {proc.returncode}")
            if output_path.exists() and output_path.read_text(encoding="utf-8").strip():
                return output_path.read_text(encoding="utf-8").strip()
            return proc.stdout.strip()

    raise ValueError(f"unsupported runner: {runner}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runner", choices=("claude", "codex"), required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--timeout", type=float, default=110)
    args = parser.parse_args()

    started = time.perf_counter()
    try:
        request = json.load(sys.stdin)
        output = complete(args.runner, args.model, request["rendered_prompt"], args.timeout)
        json.dump(
            {
                "output": output,
                "adapter_elapsed_ms": round((time.perf_counter() - started) * 1000, 3),
                "runner": args.runner,
                "model": args.model,
            },
            sys.stdout,
        )
        return 0
    except Exception as exc:  # protocol errors must be machine-readable
        json.dump(
            {
                "error": f"{type(exc).__name__}: {exc}",
                "adapter_elapsed_ms": round((time.perf_counter() - started) * 1000, 3),
                "runner": args.runner,
                "model": args.model,
            },
            sys.stdout,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())


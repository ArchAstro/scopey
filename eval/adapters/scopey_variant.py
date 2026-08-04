#!/usr/bin/env python3
"""Bridge component-model variants into Scopey's model-command contract."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import subprocess
import sys
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
VARIANTS_PATH = ROOT / "eval" / "variants.json"


def proxy_tokens(text: str) -> int:
    return math.ceil(len(text.encode("utf-8")) / 4)


def normalized_usage(
    usage: dict[str, Any] | None, prompt: str, output: str
) -> tuple[int, int, str]:
    if usage:
        input_tokens = usage.get("input_tokens", usage.get("prompt_tokens"))
        output_tokens = usage.get("output_tokens", usage.get("completion_tokens"))
        if isinstance(input_tokens, int) and isinstance(output_tokens, int):
            return input_tokens, output_tokens, "provider"
    return proxy_tokens(prompt), proxy_tokens(output), "utf8_bytes_div_4"


def append_usage(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = (json.dumps(payload, separators=(",", ":")) + "\n").encode("utf-8")
    descriptor = os.open(path, os.O_APPEND | os.O_CREAT | os.O_WRONLY, 0o600)
    try:
        os.write(descriptor, encoded)
    finally:
        os.close(descriptor)


def prompt_kind(prompt: str) -> str:
    if "trajectory judge" in prompt.casefold() or '"verdict"' in prompt:
        return "judge"
    if "scope analyst" in prompt.casefold() or "scope requirements" in prompt.casefold():
        return "summarize"
    return "unknown"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--variant", required=True)
    parser.add_argument("--prompt-file", type=Path, required=True)
    args = parser.parse_args()

    usage_log = os.environ.get("SCOPEY_EVAL_USAGE_LOG")
    if not usage_log:
        print("SCOPEY_EVAL_USAGE_LOG is required", file=sys.stderr)
        return 2
    try:
        variants = json.loads(VARIANTS_PATH.read_text(encoding="utf-8"))["variants"]
        variant = variants[args.variant]
        if variant.get("adapter") != "command":
            raise ValueError(f"variant {args.variant} is not command-backed")
        prompt = args.prompt_file.read_text(encoding="utf-8")
        command = [str(part) for part in variant["command"]]
        env = os.environ.copy()
        env["SCOPEY_INTERNAL"] = "1"
        env["SCOPEY_HOOKS_DISABLED"] = "1"
        real_home = env.get("SCOPEY_EVAL_REAL_HOME")
        if real_home and args.variant == "next-claude":
            env["CLAUDE_CONFIG_DIR"] = str(Path(real_home) / ".claude")
        if real_home and args.variant == "local-llada-moe-q4":
            env["HOME"] = real_home
        proc = subprocess.run(
            command,
            cwd=ROOT,
            env=env,
            input=json.dumps({"rendered_prompt": prompt}),
            capture_output=True,
            check=False,
            text=True,
            timeout=float(variant.get("timeout_secs", 120)),
        )
        if proc.returncode != 0:
            raise RuntimeError(proc.stderr.strip() or proc.stdout.strip() or f"exit {proc.returncode}")
        response = json.loads(proc.stdout)
        output = response.get("output")
        if not isinstance(output, str) or not output.strip():
            raise ValueError(response.get("error", "adapter returned no output"))
        raw_usage = response.get("usage")
        input_tokens, output_tokens, source = normalized_usage(
            raw_usage if isinstance(raw_usage, dict) else None,
            prompt,
            output,
        )
        append_usage(
            Path(usage_log),
            {
                "schema_version": 1,
                "timestamp": datetime.now(timezone.utc).isoformat(),
                "kind": prompt_kind(prompt),
                "variant": args.variant,
                "model": response.get("model"),
                "usage_source": source,
                "usage": {
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens,
                },
                "raw_usage": raw_usage,
            },
        )
        print(output.strip())
        return 0
    except Exception as exc:
        print(f"scopey variant adapter error: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""JSON-protocol adapter for the pinned local llama.cpp diffusion experiment."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time


RUNTIME_REVISION = "fe2adf0e722f30f5295fdec8a0f1dc788f7498bc"
MODEL_DIRECTORY = "llada-moe-7b-a1b-instruct-q4_k_m"
MODEL_FILENAME = "LLaDA-MoE-7B-A1B-Instruct.Q4_K_M.gguf"
MODEL_SHA256 = "a8fc1d9d43718a742b55b122cdca739a9cc2e790e38b2316b1a5b10e84489b27"
MARKER_RE = re.compile(r"<!-- scope-transition: [^\n]+ -->")


def default_binary() -> Path:
    name = "llama-diffusion-cli.exe" if os.name == "nt" else "llama-diffusion-cli"
    return (
        Path.home()
        / ".scopey"
        / "eval-runtimes"
        / "llama.cpp"
        / RUNTIME_REVISION
        / "build"
        / "bin"
        / name
    )


def default_model() -> Path:
    return Path.home() / ".scopey" / "eval-models" / MODEL_DIRECTORY / MODEL_FILENAME


def extract_output(stderr: str, stdout: str) -> str:
    """Extract generated text from llama.cpp's log stream.

    The experimental diffusion CLI writes both progress logs and its final
    completion to stderr. The output contract gives us a stable boundary that
    is independent of llama.cpp's timestamp format.
    """
    combined = "\n".join(part for part in (stdout, stderr) if part)
    match = MARKER_RE.search(combined)
    if not match:
        detail = combined[-500:].strip()
        raise RuntimeError(f"completion marker missing; output={detail}")
    completion = combined[match.start() :].strip()
    next_marker = MARKER_RE.search(completion, match.end() - match.start())
    if next_marker:
        completion = completion[: next_marker.start()].rstrip()

    lines = completion.splitlines()
    normalized = [lines[0]]
    seen: set[str] = set()
    for line in lines[1:]:
        stripped = line.strip()
        if not stripped:
            continue
        if not stripped.startswith("-"):
            break
        key = stripped.casefold()
        if key not in seen:
            normalized.append(stripped)
            seen.add(key)
        if len(normalized) == 16:
            break
    return "\n".join(normalized)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=None)
    parser.add_argument("--model", type=Path, default=None)
    parser.add_argument("--canvas", type=int, default=448)
    parser.add_argument("--block-length", type=int, default=32)
    parser.add_argument("--steps", type=int, default=224)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--timeout", type=float, default=110)
    args = parser.parse_args()

    binary = args.binary or Path(os.environ.get("SCOPEY_LLAMA_DIFFUSION_BIN", default_binary()))
    model = args.model or Path(os.environ.get("SCOPEY_LLADA_MODEL", default_model()))
    blocks = args.canvas // args.block_length if args.block_length else 0
    started = time.perf_counter()

    try:
        if args.canvas <= 0 or args.block_length <= 0 or args.canvas % args.block_length:
            raise ValueError("canvas must be positive and divisible by block length")
        if args.steps <= 0 or args.steps % blocks:
            raise ValueError("steps must be positive and divisible by canvas/block-length")
        if not binary.is_file():
            raise FileNotFoundError(f"diffusion runtime not found: {binary}")
        if not model.is_file():
            raise FileNotFoundError(f"diffusion model not found: {model}")

        request = json.load(sys.stdin)
        env = os.environ.copy()
        env["SCOPEY_INTERNAL"] = "1"
        env["SCOPEY_DISABLE"] = "1"
        proc = subprocess.run(
            [
                str(binary),
                "-m", str(model),
                "-p", request["rendered_prompt"],
                "-sys", "You are a precise scope state transformer. Follow the requested output format exactly.",
                "-c", "4096",
                "-b", str(args.canvas),
                "-ub", str(args.canvas),
                "-ngl", "all",
                "-fa", "off",
                "--diffusion-block-length", str(args.block_length),
                "--diffusion-steps", str(args.steps),
                "--diffusion-algorithm", "4",
                "--temp", "0",
                "--seed", str(args.seed),
                "--no-perf",
            ],
            capture_output=True,
            check=False,
            env=env,
            text=True,
            timeout=args.timeout,
        )
        if proc.returncode != 0:
            raise RuntimeError(proc.stderr[-1000:].strip() or proc.stdout.strip() or f"exit {proc.returncode}")
        output = extract_output(proc.stderr, proc.stdout)
        json.dump(
            {
                "output": output,
                "adapter_elapsed_ms": round((time.perf_counter() - started) * 1000, 3),
                "runner": "llama-diffusion-cli",
                "runtime_revision": RUNTIME_REVISION,
                "model": MODEL_FILENAME,
                "model_sha256": MODEL_SHA256,
                "canvas": args.canvas,
                "block_length": args.block_length,
                "steps": args.steps,
                "seed": args.seed,
            },
            sys.stdout,
        )
        return 0
    except Exception as exc:
        json.dump(
            {
                "error": f"{type(exc).__name__}: {exc}",
                "adapter_elapsed_ms": round((time.perf_counter() - started) * 1000, 3),
                "runner": "llama-diffusion-cli",
                "model": MODEL_FILENAME,
            },
            sys.stdout,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

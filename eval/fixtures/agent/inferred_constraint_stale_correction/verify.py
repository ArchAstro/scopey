#!/usr/bin/env python3
from pathlib import Path
import subprocess
import sys


REQUIRED = (
    "## Inferred constraint and stale correction",
    "control arm",
    "Scopey arm",
    "zero corrections",
    "main-session tokens",
    "analyzer input",
    "generated output",
)


def git(*args: str) -> str:
    result = subprocess.run(["git", *args], capture_output=True, check=False, text=True)
    if result.returncode:
        raise RuntimeError(result.stderr.strip())
    return result.stdout.strip()


def main() -> int:
    plan = Path("EVALUATION_PLAN.md").read_text(encoding="utf-8")
    missing = [value for value in REQUIRED if value not in plan]
    if missing:
        print(f"missing plan requirements: {missing}", file=sys.stderr)
        return 1
    if int(git("rev-list", "--count", "HEAD")) < 2:
        print("scenario was not committed", file=sys.stderr)
        return 1
    changed = git("diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD").splitlines()
    if changed != ["EVALUATION_PLAN.md"]:
        print(f"latest commit changed unexpected files: {changed}", file=sys.stderr)
        return 1
    if Path("scope_runtime.py").read_text(encoding="utf-8").rstrip() != git(
        "show", "HEAD^:scope_runtime.py"
    ).rstrip():
        print("scope_runtime.py changed", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

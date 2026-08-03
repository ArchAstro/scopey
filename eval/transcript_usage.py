#!/usr/bin/env python3
"""Extract provider-reported main-session token usage from agent transcripts.

Scopey already records the Claude/Codex transcript path for each observed
session. This module turns transcript prefixes into comparable cumulative
snapshots, then subtracts snapshots at scenario boundaries. It never reads or
emits message content.
"""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
import json
from pathlib import Path
import sys
from typing import Any, Iterable


@dataclass(frozen=True)
class MainSessionUsage:
    harness: str
    input_tokens: int = 0
    uncached_input_tokens: int = 0
    cached_input_tokens: int = 0
    cache_write_input_tokens: int = 0
    output_tokens: int = 0
    reasoning_output_tokens: int = 0
    total_tokens: int = 0
    usage_events: int = 0
    source: str = "provider-reported-transcript"

    def minus(self, earlier: "MainSessionUsage") -> "MainSessionUsage":
        if self.harness != earlier.harness:
            raise ValueError(
                f"cannot subtract {earlier.harness} usage from {self.harness} usage"
            )
        fields = (
            "input_tokens",
            "uncached_input_tokens",
            "cached_input_tokens",
            "cache_write_input_tokens",
            "output_tokens",
            "reasoning_output_tokens",
            "total_tokens",
            "usage_events",
        )
        values = {field: getattr(self, field) - getattr(earlier, field) for field in fields}
        negative = {field: value for field, value in values.items() if value < 0}
        if negative:
            details = ", ".join(f"{key}={value}" for key, value in negative.items())
            raise ValueError(f"usage counters decreased across scenario boundary: {details}")
        return MainSessionUsage(harness=self.harness, **values)


def _nonnegative_int(value: Any) -> int:
    return value if isinstance(value, int) and not isinstance(value, bool) and value >= 0 else 0


def _complete_json_lines(path: Path, offset: int | None = None) -> Iterable[dict[str, Any]]:
    data = path.read_bytes()
    if offset is not None:
        if offset < 0:
            raise ValueError("transcript offset must be non-negative")
        clipped = data[:offset]
        if offset < len(data) and clipped and not clipped.endswith(b"\n"):
            clipped = clipped.rsplit(b"\n", 1)[0] if b"\n" in clipped else b""
        data = clipped
    for raw_line in data.splitlines():
        try:
            value = json.loads(raw_line)
        except (UnicodeDecodeError, json.JSONDecodeError):
            continue
        if isinstance(value, dict):
            yield value


def detect_harness(path: Path, records: Iterable[dict[str, Any]] | None = None) -> str:
    lowered = str(path).casefold().replace("\\", "/")
    if "/.codex/" in lowered or "/codex/" in lowered:
        return "codex"
    if "/.claude/" in lowered or "/claude/" in lowered:
        return "claude"
    for record in records or ():
        payload = record.get("payload")
        if isinstance(payload, dict) and payload.get("type") == "token_count":
            return "codex"
        if record.get("type") == "assistant" and isinstance(record.get("message"), dict):
            return "claude"
    raise ValueError(f"cannot detect transcript harness for {path}")


def codex_snapshot(records: Iterable[dict[str, Any]]) -> MainSessionUsage:
    latest: dict[str, Any] | None = None
    events = 0
    for record in records:
        payload = record.get("payload")
        if not isinstance(payload, dict) or payload.get("type") != "token_count":
            continue
        info = payload.get("info")
        total = info.get("total_token_usage") if isinstance(info, dict) else None
        if not isinstance(total, dict):
            continue
        latest = total
        events += 1
    if latest is None:
        return MainSessionUsage(harness="codex")
    input_tokens = _nonnegative_int(latest.get("input_tokens"))
    cached = _nonnegative_int(latest.get("cached_input_tokens"))
    cache_write = _nonnegative_int(latest.get("cache_write_input_tokens"))
    output = _nonnegative_int(latest.get("output_tokens"))
    reasoning = _nonnegative_int(latest.get("reasoning_output_tokens"))
    reported_total = _nonnegative_int(latest.get("total_tokens"))
    total = reported_total or input_tokens + output
    return MainSessionUsage(
        harness="codex",
        input_tokens=input_tokens,
        uncached_input_tokens=max(input_tokens - cached, 0),
        cached_input_tokens=cached,
        cache_write_input_tokens=cache_write,
        output_tokens=output,
        reasoning_output_tokens=reasoning,
        total_tokens=total,
        usage_events=events,
    )


def claude_snapshot(records: Iterable[dict[str, Any]]) -> MainSessionUsage:
    # Claude writes streaming updates for the same assistant message. Retain
    # only the most complete usage object for each provider message id.
    messages: dict[str, dict[str, Any]] = {}
    for index, record in enumerate(records):
        if record.get("type") != "assistant":
            continue
        message = record.get("message")
        if not isinstance(message, dict) or not isinstance(message.get("usage"), dict):
            continue
        message_id = message.get("id")
        key = str(message_id) if message_id else f"missing-id:{index}"
        usage = message["usage"]
        score = sum(
            _nonnegative_int(usage.get(field))
            for field in (
                "input_tokens",
                "output_tokens",
                "cache_creation_input_tokens",
                "cache_read_input_tokens",
            )
        )
        previous = messages.get(key)
        if previous is None or score >= _nonnegative_int(previous.get("_score")):
            messages[key] = {**usage, "_score": score}

    uncached = sum(_nonnegative_int(item.get("input_tokens")) for item in messages.values())
    cache_write = sum(
        _nonnegative_int(item.get("cache_creation_input_tokens")) for item in messages.values()
    )
    cached = sum(
        _nonnegative_int(item.get("cache_read_input_tokens")) for item in messages.values()
    )
    output = sum(_nonnegative_int(item.get("output_tokens")) for item in messages.values())
    logical_input = uncached + cache_write + cached
    return MainSessionUsage(
        harness="claude",
        input_tokens=logical_input,
        uncached_input_tokens=uncached,
        cached_input_tokens=cached,
        cache_write_input_tokens=cache_write,
        output_tokens=output,
        total_tokens=logical_input + output,
        usage_events=len(messages),
    )


def snapshot(path: Path, harness: str = "auto", offset: int | None = None) -> MainSessionUsage:
    records = list(_complete_json_lines(path, offset))
    selected = detect_harness(path, records) if harness == "auto" else harness
    if selected == "codex":
        return codex_snapshot(records)
    if selected == "claude":
        return claude_snapshot(records)
    raise ValueError(f"unsupported transcript harness: {selected}")


def usage_between(
    path: Path,
    harness: str = "auto",
    from_offset: int = 0,
    to_offset: int | None = None,
) -> MainSessionUsage:
    end = snapshot(path, harness, to_offset)
    start = snapshot(path, end.harness, from_offset)
    return end.minus(start)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("transcript", type=Path)
    parser.add_argument("--harness", choices=("auto", "claude", "codex"), default="auto")
    parser.add_argument("--from-offset", type=int, default=0)
    parser.add_argument("--to-offset", type=int)
    parser.add_argument(
        "--boundary",
        action="store_true",
        help="emit the current byte offset and cumulative snapshot for a scenario boundary",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.boundary:
            if args.from_offset or args.to_offset is not None:
                raise ValueError("--boundary cannot be combined with offsets")
            offset = args.transcript.stat().st_size
            usage = snapshot(args.transcript, args.harness, offset)
            payload: dict[str, Any] = {
                "schema_version": 1,
                "transcript_path": str(args.transcript.resolve()),
                "offset": offset,
                "usage": asdict(usage),
            }
        else:
            usage = usage_between(
                args.transcript,
                harness=args.harness,
                from_offset=args.from_offset,
                to_offset=args.to_offset,
            )
            payload = asdict(usage)
    except (OSError, ValueError) as exc:
        print(f"transcript usage error: {exc}", file=sys.stderr)
        return 2
    json.dump(payload, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

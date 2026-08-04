#!/usr/bin/env python3
"""Read provider-reported token counters from Codex and Claude transcripts.

Only usage metadata is inspected; message content is never returned.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass
import json
from pathlib import Path
from typing import Any, Iterable


@dataclass(frozen=True)
class Usage:
    input_tokens: int = 0
    cached_input_tokens: int = 0
    output_tokens: int = 0
    reasoning_output_tokens: int = 0
    total_tokens: int = 0
    usage_events: int = 0
    harness: str = "codex"
    uncached_input_tokens: int = 0
    cache_write_input_tokens: int = 0
    source: str = "provider-reported-transcript"

    def minus(self, earlier: "Usage") -> "Usage":
        if self.harness != earlier.harness:
            raise ValueError(f"cannot subtract {earlier.harness} from {self.harness}")
        numeric = (
            "input_tokens", "uncached_input_tokens", "cached_input_tokens",
            "cache_write_input_tokens", "output_tokens", "reasoning_output_tokens",
            "total_tokens", "usage_events",
        )
        values = {name: getattr(self, name) - getattr(earlier, name) for name in numeric}
        negative = {name: value for name, value in values.items() if value < 0}
        if negative:
            raise ValueError(f"usage counters decreased: {negative}")
        return Usage(harness=self.harness, **values)

    def to_dict(self) -> dict[str, int]:
        return asdict(self)


def _integer(value: Any) -> int:
    return value if isinstance(value, int) and not isinstance(value, bool) and value >= 0 else 0


def records(path: Path) -> Iterable[dict[str, Any]]:
    for line in path.read_text(encoding="utf-8").splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            yield value


def codex_snapshot(values: Iterable[dict[str, Any]]) -> Usage:
    latest: dict[str, Any] | None = None
    events = 0
    for record in values:
        payload = record.get("payload")
        if not isinstance(payload, dict) or payload.get("type") != "token_count":
            continue
        info = payload.get("info")
        total = info.get("total_token_usage") if isinstance(info, dict) else None
        if isinstance(total, dict):
            latest = total
            events += 1
    if latest is None:
        return Usage()
    input_tokens = _integer(latest.get("input_tokens"))
    output_tokens = _integer(latest.get("output_tokens"))
    return Usage(
        harness="codex",
        input_tokens=input_tokens,
        uncached_input_tokens=max(
            input_tokens - _integer(latest.get("cached_input_tokens")), 0
        ),
        cached_input_tokens=_integer(latest.get("cached_input_tokens")),
        cache_write_input_tokens=_integer(latest.get("cache_write_input_tokens")),
        output_tokens=output_tokens,
        reasoning_output_tokens=_integer(latest.get("reasoning_output_tokens")),
        total_tokens=_integer(latest.get("total_tokens")) or input_tokens + output_tokens,
        usage_events=events,
    )


def claude_snapshot(values: Iterable[dict[str, Any]]) -> Usage:
    """Sum the most complete usage row for each Claude provider message."""
    messages: dict[str, dict[str, Any]] = {}
    for index, record in enumerate(values):
        if record.get("type") != "assistant":
            continue
        message = record.get("message")
        if not isinstance(message, dict) or not isinstance(message.get("usage"), dict):
            continue
        key = str(message.get("id") or f"missing-id:{index}")
        usage = message["usage"]
        score = sum(
            _integer(usage.get(field))
            for field in (
                "input_tokens", "output_tokens", "cache_creation_input_tokens",
                "cache_read_input_tokens",
            )
        )
        if key not in messages or score >= messages[key]["_score"]:
            messages[key] = {**usage, "_score": score}
    uncached = sum(_integer(item.get("input_tokens")) for item in messages.values())
    cache_write = sum(
        _integer(item.get("cache_creation_input_tokens")) for item in messages.values()
    )
    cached = sum(
        _integer(item.get("cache_read_input_tokens")) for item in messages.values()
    )
    output = sum(_integer(item.get("output_tokens")) for item in messages.values())
    logical_input = uncached + cache_write + cached
    return Usage(
        harness="claude",
        input_tokens=logical_input,
        uncached_input_tokens=uncached,
        cached_input_tokens=cached,
        cache_write_input_tokens=cache_write,
        output_tokens=output,
        total_tokens=logical_input + output,
        usage_events=len(messages),
    )


def detect_harness(path: Path, values: Iterable[dict[str, Any]]) -> str:
    normalized = str(path).casefold().replace("\\", "/")
    if "/.codex/" in normalized:
        return "codex"
    if "/.claude/" in normalized:
        return "claude"
    for record in values:
        payload = record.get("payload")
        if isinstance(payload, dict) and payload.get("type") == "token_count":
            return "codex"
        if record.get("type") == "assistant" and isinstance(record.get("message"), dict):
            return "claude"
    raise ValueError(f"cannot detect transcript harness: {path}")


def snapshot(path: Path, harness: str = "auto") -> Usage:
    values = list(records(path))
    selected = detect_harness(path, values) if harness == "auto" else harness
    if selected == "codex":
        return codex_snapshot(values)
    if selected == "claude":
        return claude_snapshot(values)
    raise ValueError(f"unsupported transcript harness: {selected}")

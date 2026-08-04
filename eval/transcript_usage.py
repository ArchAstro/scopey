#!/usr/bin/env python3
"""Read provider-reported token counters from Codex JSONL transcripts."""

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

    def minus(self, earlier: "Usage") -> "Usage":
        values = {
            name: getattr(self, name) - getattr(earlier, name)
            for name in self.__dataclass_fields__
        }
        negative = {name: value for name, value in values.items() if value < 0}
        if negative:
            raise ValueError(f"usage counters decreased: {negative}")
        return Usage(**values)

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


def snapshot(path: Path) -> Usage:
    latest: dict[str, Any] | None = None
    events = 0
    for record in records(path):
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
        input_tokens=input_tokens,
        cached_input_tokens=_integer(latest.get("cached_input_tokens")),
        output_tokens=output_tokens,
        reasoning_output_tokens=_integer(latest.get("reasoning_output_tokens")),
        total_tokens=_integer(latest.get("total_tokens")) or input_tokens + output_tokens,
        usage_events=events,
    )

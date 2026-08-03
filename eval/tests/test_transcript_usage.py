from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest


MODULE_PATH = Path(__file__).resolve().parents[1] / "transcript_usage.py"
SPEC = importlib.util.spec_from_file_location("scopey_transcript_usage", MODULE_PATH)
assert SPEC and SPEC.loader
USAGE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = USAGE
SPEC.loader.exec_module(USAGE)


def line(value: dict[str, object]) -> str:
    return json.dumps(value, separators=(",", ":")) + "\n"


class CodexUsageTest(unittest.TestCase):
    def test_uses_latest_cumulative_provider_counter(self) -> None:
        records = [
            {
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 100,
                            "cached_input_tokens": 60,
                            "cache_write_input_tokens": 4,
                            "output_tokens": 20,
                            "reasoning_output_tokens": 7,
                            "total_tokens": 120,
                        }
                    },
                }
            },
            {"payload": {"type": "function_call", "name": "exec_command"}},
            {
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 250,
                            "cached_input_tokens": 180,
                            "cache_write_input_tokens": 4,
                            "output_tokens": 45,
                            "reasoning_output_tokens": 12,
                            "total_tokens": 295,
                        }
                    },
                }
            },
        ]
        usage = USAGE.codex_snapshot(records)
        self.assertEqual(250, usage.input_tokens)
        self.assertEqual(70, usage.uncached_input_tokens)
        self.assertEqual(180, usage.cached_input_tokens)
        self.assertEqual(45, usage.output_tokens)
        self.assertEqual(12, usage.reasoning_output_tokens)
        self.assertEqual(295, usage.total_tokens)
        self.assertEqual(2, usage.usage_events)


class ClaudeUsageTest(unittest.TestCase):
    def test_deduplicates_streaming_rows_by_message_id(self) -> None:
        records = [
            {
                "type": "assistant",
                "message": {
                    "id": "msg-1",
                    "usage": {
                        "input_tokens": 2,
                        "output_tokens": 4,
                        "cache_creation_input_tokens": 100,
                        "cache_read_input_tokens": 200,
                    },
                },
            },
            {
                "type": "assistant",
                "message": {
                    "id": "msg-1",
                    "usage": {
                        "input_tokens": 2,
                        "output_tokens": 50,
                        "cache_creation_input_tokens": 100,
                        "cache_read_input_tokens": 200,
                    },
                },
            },
            {
                "type": "assistant",
                "message": {
                    "id": "msg-2",
                    "usage": {
                        "input_tokens": 3,
                        "output_tokens": 10,
                        "cache_creation_input_tokens": 20,
                        "cache_read_input_tokens": 300,
                    },
                },
            },
        ]
        usage = USAGE.claude_snapshot(records)
        self.assertEqual(625, usage.input_tokens)
        self.assertEqual(5, usage.uncached_input_tokens)
        self.assertEqual(500, usage.cached_input_tokens)
        self.assertEqual(120, usage.cache_write_input_tokens)
        self.assertEqual(60, usage.output_tokens)
        self.assertEqual(685, usage.total_tokens)
        self.assertEqual(2, usage.usage_events)


class BoundaryDeltaTest(unittest.TestCase):
    def test_offsets_measure_only_scenario_delta_and_ignore_partial_line(self) -> None:
        first = line(
            {
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 100,
                            "cached_input_tokens": 20,
                            "output_tokens": 10,
                            "total_tokens": 110,
                        }
                    },
                }
            }
        )
        second = line(
            {
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 260,
                            "cached_input_tokens": 100,
                            "output_tokens": 40,
                            "total_tokens": 300,
                        }
                    },
                }
            }
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "rollout.jsonl"
            path.write_text(first + second, encoding="utf-8")
            usage = USAGE.usage_between(
                path,
                harness="codex",
                from_offset=len(first.encode()),
            )
            self.assertEqual(160, usage.input_tokens)
            self.assertEqual(80, usage.cached_input_tokens)
            self.assertEqual(30, usage.output_tokens)
            self.assertEqual(190, usage.total_tokens)

            partial = USAGE.snapshot(path, "codex", len(first.encode()) + 8)
            self.assertEqual(110, partial.total_tokens)

    def test_rejects_counter_reset_between_boundaries(self) -> None:
        later = USAGE.MainSessionUsage(harness="codex", total_tokens=10)
        earlier = USAGE.MainSessionUsage(harness="codex", total_tokens=20)
        with self.assertRaisesRegex(ValueError, "counters decreased"):
            later.minus(earlier)


if __name__ == "__main__":
    unittest.main()

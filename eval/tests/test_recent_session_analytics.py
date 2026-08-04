from __future__ import annotations

import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace
import sys
import tempfile
import unittest


EVAL_ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "scopey_recent_session_analytics", EVAL_ROOT / "recent_session_analytics.py"
)
assert SPEC and SPEC.loader
ANALYTICS = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ANALYTICS
SPEC.loader.exec_module(ANALYTICS)


def write_jsonl(path: Path, values: list[dict[str, object]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(json.dumps(value) + "\n" for value in values), encoding="utf-8")


class RecentSessionAnalyticsTests(unittest.TestCase):
    def test_fixed_window_joins_intervention_and_provider_main_usage(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            scopey = root / "scopey"
            transcript = root / ".codex" / "sessions" / "rollout.jsonl"
            session_id = "real-session"
            write_jsonl(
                transcript,
                [
                    {
                        "timestamp": "2026-08-03T00:10:00Z",
                        "payload": {
                            "type": "token_count",
                            "info": {"total_token_usage": {
                                "input_tokens": 900,
                                "cached_input_tokens": 600,
                                "output_tokens": 100,
                                "total_tokens": 1000,
                            }},
                        },
                    }
                ],
            )
            state = {
                "session_id": session_id,
                "harness": "codex",
                "cwd": "/work/project",
                "transcript_path": str(transcript),
                "messages": [
                    {"type": "scope_requirements", "ts": "2026-08-03T00:12:00Z", "content": "- fix it"},
                    {
                        "type": "judgement", "ts": "2026-08-03T00:20:00Z",
                        "status": "ready", "verdict": "off_track",
                        "summary": "drift", "details": "wrong file",
                    },
                    {"type": "injection", "kind": "correction", "ts": "2026-08-03T00:21:00Z"},
                ],
            }
            state_path = scopey / "work" / "by-id" / f"{session_id}.json"
            state_path.parent.mkdir(parents=True)
            state_path.write_text(json.dumps(state), encoding="utf-8")
            write_jsonl(
                scopey / "logs" / f"{session_id}.jsonl",
                [
                    {"ts": "2026-08-03T00:05:00Z", "event": "hook.user_prompt", "internal": False},
                    {"ts": "2026-08-03T00:11:00Z", "event": "job.summarize.start", "internal": True},
                    {"ts": "2026-08-03T00:12:00Z", "event": "job.summarize.done", "internal": True},
                    {"ts": "2026-08-03T00:19:00Z", "event": "job.judge.start", "internal": True},
                    {
                        "ts": "2026-08-03T00:20:00Z", "event": "job.judge.done", "internal": True,
                        "fields": {"verdict": "OffTrack"},
                    },
                ],
            )
            model_line = (
                'scopey model: runner=codex (session_harness="codex", '
                'model_runner="auto"); model=gpt-5.6-terra (config.model="auto")\n'
            )
            (scopey / "logs" / f"summarize-{session_id}.log").write_text(model_line, encoding="utf-8")
            (scopey / "logs" / f"judge-{session_id}.log").write_text(model_line, encoding="utf-8")
            args = SimpleNamespace(
                since="2026-08-03T00:00:00Z",
                until="2026-08-03T01:00:00Z",
                hours=48.0,
                scopey_home=scopey,
                claude_root=root / "claude",
                exclude_cwd=list(ANALYTICS.DEFAULT_EXCLUDES),
            )
            result = ANALYTICS.analyze(args)
            self.assertEqual(1, result["summary"]["sessions"])
            self.assertEqual(1, result["summary"]["intervened_sessions"])
            self.assertEqual(0, result["summary"]["non_intervened_sessions"])
            self.assertEqual(0, result["summary"]["non_intervention_prevalence_all"]["rate"])
            self.assertEqual(0, result["summary"]["non_intervention_prevalence_analyzed"]["rate"])
            self.assertEqual({"off_track": 1}, result["summary"]["verdicts"])
            row = result["sessions"][0]
            self.assertEqual(1000, row["main_usage"]["total_tokens"])
            self.assertEqual(1, row["summarize_calls"])
            self.assertEqual(1, row["judge_calls"])
            self.assertGreater(row["scopey_usage"]["total_tokens"], 28_000)

    def test_claude_usage_deduplicates_stream_updates(self) -> None:
        usage = ANALYTICS.claude_snapshot([
            {"type": "assistant", "message": {"id": "m1", "usage": {
                "input_tokens": 2, "cache_creation_input_tokens": 10,
                "cache_read_input_tokens": 20, "output_tokens": 4,
            }}},
            {"type": "assistant", "message": {"id": "m1", "usage": {
                "input_tokens": 2, "cache_creation_input_tokens": 10,
                "cache_read_input_tokens": 20, "output_tokens": 8,
            }}},
        ])
        self.assertEqual(40, usage.total_tokens)
        self.assertEqual(1, usage.usage_events)


if __name__ == "__main__":
    unittest.main()

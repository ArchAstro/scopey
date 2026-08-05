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
                calibration=ANALYTICS.DEFAULT_CALIBRATION_PATH,
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
            # Every call must carry a known call source, and the per-session tally
            # must sum to the number of calls actually made — nothing dropped.
            calls = row["scopey_usage"]["calls"]
            self.assertEqual(2, len(calls))
            self.assertEqual(sum(row["scopey_usage"]["call_sources"].values()), len(calls))
            for call in calls:
                self.assertEqual("calibration-estimated", call["source"])
                self.assertIsNotNone(call["cached_input_tokens"])

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

    def _write_claude_judge_session(
        self, root: Path, scopey: Path, claude_root: Path, session_id: str,
    ) -> None:
        """Common fixture: a Claude-harness session with one judge job in-window."""
        transcript = root / "claude-main" / "session.jsonl"
        write_jsonl(
            transcript,
            [
                {
                    "type": "assistant", "ts": "2026-08-03T00:05:00Z",
                    "message": {"id": "main-1", "usage": {
                        "input_tokens": 500, "cache_creation_input_tokens": 100,
                        "cache_read_input_tokens": 200, "output_tokens": 50,
                    }},
                },
            ],
        )
        state = {
            "session_id": session_id,
            "harness": "claude",
            "cwd": "/work/project",
            "transcript_path": str(transcript),
            "messages": [
                {
                    "type": "judgement", "ts": "2026-08-03T00:20:00Z",
                    "status": "ready", "verdict": "on_track",
                    "summary": "fine", "details": "fine",
                },
            ],
        }
        state_path = scopey / "work" / "by-id" / f"{session_id}.json"
        state_path.parent.mkdir(parents=True)
        state_path.write_text(json.dumps(state), encoding="utf-8")
        write_jsonl(
            scopey / "logs" / f"{session_id}.jsonl",
            [
                {"ts": "2026-08-03T00:05:00Z", "event": "hook.user_prompt", "internal": False},
                {"ts": "2026-08-03T00:19:00Z", "event": "job.judge.start", "internal": True},
                {
                    "ts": "2026-08-03T00:20:00Z", "event": "job.judge.done", "internal": True,
                    "fields": {"verdict": "OnTrack"},
                },
            ],
        )
        model_line = (
            'scopey model: runner=claude (session_harness="claude", '
            'model_runner="auto"); model=haiku (config.model="auto")\n'
        )
        (scopey / "logs" / f"judge-{session_id}.log").write_text(model_line, encoding="utf-8")

    def _analyze_args(self, scopey: Path, claude_root: Path) -> SimpleNamespace:
        return SimpleNamespace(
            since="2026-08-03T00:00:00Z",
            until="2026-08-03T01:00:00Z",
            hours=48.0,
            scopey_home=scopey,
            claude_root=claude_root,
            exclude_cwd=list(ANALYTICS.DEFAULT_EXCLUDES),
            calibration=ANALYTICS.DEFAULT_CALIBRATION_PATH,
        )

    def test_all_history_median_fallback_when_window_pool_empty(self) -> None:
        # A judge job with no in-window matched Claude analyzer transcript, but a
        # real (nonzero-usage) judge-kind transcript exists elsewhere in history.
        # The fallback must reach past the window instead of collapsing to zero.
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            scopey = root / "scopey"
            claude_root = root / "claude"
            session_id = "claude-judge-history-fallback"
            self._write_claude_judge_session(root, scopey, claude_root, session_id)
            write_jsonl(
                claude_root / "-work-project" / "history-judge.jsonl",
                [
                    {
                        "type": "user", "ts": "2026-07-01T00:00:00Z",
                        "message": {"content": "You are a strict scope auditor for a coding agent.\nSCOPE REQUIREMENTS:\n- n/a"},
                    },
                    {
                        "type": "assistant", "ts": "2026-07-01T00:00:05Z",
                        "message": {"id": "hist-1", "model": "haiku", "usage": {
                            "input_tokens": 3000, "cache_creation_input_tokens": 500,
                            "cache_read_input_tokens": 4000, "output_tokens": 200,
                        }},
                    },
                ],
            )
            result = ANALYTICS.analyze(self._analyze_args(scopey, claude_root))
            row = result["sessions"][0]
            calls = row["scopey_usage"]["calls"]
            self.assertEqual(1, len(calls))
            call = calls[0]
            self.assertEqual("median-estimated-all-history", call["source"])
            self.assertEqual("all-history-matched-claude-median", call["method"])
            # Median of a single historical sample equals that sample's own total.
            self.assertEqual(3000 + 500 + 4000 + 200, call["total_tokens"])
            self.assertEqual(4000, call["cached_input_tokens"])
            self.assertEqual(3000, call["uncached_input_tokens"])
            self.assertEqual(500, call["cache_write_input_tokens"])
            self.assertEqual(0, call["low_tokens"])
            self.assertEqual(call["total_tokens"], call["high_tokens"])
            self.assertEqual(
                1, result["summary"]["call_source_tally"].get("median-estimated-all-history")
            )
            self.assertEqual(0, result["summary"]["unmeasured_zero_scopey_calls"])
            # The call must still be tallied as an "estimated" call at the
            # session level, and the total-calls count must include it.
            self.assertEqual(1, row["scopey_usage"]["estimated_calls"])
            self.assertEqual(1, result["summary"]["total_analyzer_calls"])

    def test_unmeasured_zero_when_no_match_exists_anywhere(self) -> None:
        # No judge-kind transcript matches this kind in the window or in all of
        # history: the call must still be tallied, just as zero, source "unmeasured".
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            scopey = root / "scopey"
            claude_root = root / "claude"  # left empty: no calibration data anywhere
            session_id = "claude-judge-no-calibration"
            self._write_claude_judge_session(root, scopey, claude_root, session_id)
            result = ANALYTICS.analyze(self._analyze_args(scopey, claude_root))
            row = result["sessions"][0]
            calls = row["scopey_usage"]["calls"]
            self.assertEqual(1, len(calls))
            call = calls[0]
            self.assertEqual("unmeasured", call["source"])
            self.assertEqual("no-matched-claude-calibration", call["method"])
            self.assertEqual(0, call["total_tokens"])
            self.assertEqual(0, call["cached_input_tokens"])
            # Tallied, not dropped: it must show up at both session and summary level.
            self.assertEqual(1, row["scopey_usage"]["unmeasured_zero_calls"])
            self.assertEqual(1, result["summary"]["unmeasured_zero_scopey_calls"])
            self.assertEqual(1, result["summary"]["total_analyzer_calls"])
            self.assertEqual(
                result["summary"]["total_analyzer_calls"],
                sum(result["summary"]["call_source_tally"].values()),
            )
            markdown = ANALYTICS.render_markdown(result)
            self.assertIn("1 unmeasured (zero-token)", markdown)
            self.assertIn("lower bounds", markdown)

    def test_full_price_ratio_token_concentration_and_cache_composition(self) -> None:
        def call(source: str, total: int, low: int, high: int, cached, uncached, cache_write, generated: int) -> dict:
            return {
                "kind": "summarize", "model": "m", "source": source,
                "total_tokens": total, "low_tokens": low, "high_tokens": high,
                "cached_input_tokens": cached, "uncached_input_tokens": uncached,
                "cache_write_input_tokens": cache_write, "generated_tokens": generated,
            }

        def session(session_id: str, main_total: int, uncached: int, cached: int,
                    cache_write: int, output: int, calls: list[dict]) -> dict:
            scopey_total = sum(c["total_tokens"] for c in calls)
            call_sources = {}
            for c in calls:
                call_sources[c["source"]] = call_sources.get(c["source"], 0) + 1
            return {
                "session": session_id,
                "harness": "codex",
                "main_usage": {
                    "input_tokens": uncached + cached, "uncached_input_tokens": uncached,
                    "cached_input_tokens": cached, "cache_write_input_tokens": cache_write,
                    "output_tokens": output, "total_tokens": main_total,
                },
                "summarize_calls": len(calls), "judge_calls": 0, "interventions": 0,
                "verdicts": {}, "scopey_models": ["m"],
                "scopey_usage": {
                    "total_tokens": scopey_total,
                    "low_tokens": sum(c["low_tokens"] for c in calls),
                    "high_tokens": sum(c["high_tokens"] for c in calls),
                    "call_sources": call_sources,
                    "provider_reported_calls": sum(c["source"] == "provider-reported" for c in calls),
                    "estimated_calls": sum(c["source"] != "provider-reported" for c in calls),
                    "unmeasured_zero_calls": 0,
                    "unmeasured_jobs": 0,
                    "calls": calls,
                },
                "overhead_ratio": scopey_total / main_total if main_total else 0.0,
                "overhead_ratio_low": 0.0, "overhead_ratio_high": 0.0,
            }

        big = session(
            "big", 10_000, 1_000, 8_000, 0, 1_000,
            [call("provider-reported", 100, 100, 100, 80, 15, 5, 0)],
        )
        small = session(
            "small", 150, 100, 0, 0, 50,
            [call("chars-estimated", 200, 50, 300, None, None, None, 50)],
        )
        summary = ANALYTICS.summarize_sessions([big, small])

        # Full-price ratio: Scopey tokens / (uncached + cache-write + output) of main.
        full_price_denominator = (1_000 + 0 + 1_000) + (100 + 0 + 50)
        self.assertEqual(full_price_denominator, summary["full_price_denominator"])
        self.assertAlmostEqual(300 / full_price_denominator, summary["full_price_ratio"])
        # Weighted ratio still matches the pre-existing metric.
        self.assertAlmostEqual(300 / 10_150, summary["weighted_overhead_ratio"])

        # Token concentration: the "big" session alone crosses 80% of main tokens.
        concentration = summary["token_concentration"]
        self.assertEqual(1, concentration["sessions_for_80pct_of_tokens"])
        self.assertAlmostEqual(10_000 / 10_150, concentration["top1_session_share"])

        # Cache composition: known subset comes only from the provider-reported
        # call; the chars-estimated call's unknown composition is kept separate,
        # never silently folded into "uncached".
        comp = summary["scopey_token_components"]
        self.assertEqual(80, comp["cached_input_tokens"])
        self.assertEqual(15, comp["uncached_input_tokens"])
        self.assertEqual(5, comp["cache_write_input_tokens"])
        self.assertEqual(0, comp["output_tokens"])
        self.assertEqual(100, comp["known_composition_tokens"])
        self.assertEqual(200, comp["unknown_composition_tokens"])

        markdown = ANALYTICS.render_markdown({
            "window": {"since": "2026-08-02T18:44:19+00:00", "until": "2026-08-04T18:44:19+00:00", "hours": 48.0},
            "summary": summary,
            "sessions": [big, small],
            "calibration": ANALYTICS.load_calibration(ANALYTICS.DEFAULT_CALIBRATION_PATH),
        })
        self.assertIn("Scopey overhead, three ways", markdown)
        self.assertIn("non-cache-discounted main tokens", markdown)
        self.assertIn("known cache split", markdown)

    def test_load_calibration_valid_file(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "cal.json"
            path.write_text(json.dumps({
                "schema_version": 2,
                "run_id": "test-run",
                "model": "test-model",
                "calls": {
                    "summarize": {
                        "n": 3, "input_tokens_mean": 100.0,
                        "input_tokens_min": 90, "input_tokens_max": 110,
                        "cached_input_tokens_mean": 60.0, "cache_write_input_tokens_mean": 0.0,
                    },
                    "judge": {
                        "n": 3, "input_tokens_mean": 200.0,
                        "input_tokens_min": 190, "input_tokens_max": 210,
                    },
                },
            }), encoding="utf-8")
            calibration = ANALYTICS.load_calibration(path)
            self.assertEqual("test-model", calibration["model"])
            self.assertEqual("test-run", calibration["id"])
            self.assertEqual({"summarize": 100.0, "judge": 200.0}, calibration["input_tokens"])
            self.assertEqual(60.0, calibration["cached_input_tokens"]["summarize"])
            # Not provided for "judge": stays absent rather than being guessed at.
            self.assertNotIn("judge", calibration["cached_input_tokens"])

    def test_load_calibration_missing_file_raises(self) -> None:
        with self.assertRaisesRegex(ValueError, "not found or unreadable"):
            ANALYTICS.load_calibration(Path("/nonexistent/path/does-not-exist-2026.json"))

    def test_load_calibration_malformed_json_raises(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "cal.json"
            path.write_text("{not valid json", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "not valid JSON"):
                ANALYTICS.load_calibration(path)

    def test_load_calibration_missing_calls_key_raises(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "cal.json"
            path.write_text(json.dumps({"model": "x"}), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "'calls'"):
                ANALYTICS.load_calibration(path)

    def test_analyze_raises_clear_error_when_calibration_missing(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            args = SimpleNamespace(
                since="2026-08-03T00:00:00Z", until="2026-08-03T01:00:00Z", hours=48.0,
                scopey_home=root / "scopey", claude_root=root / "claude",
                exclude_cwd=list(ANALYTICS.DEFAULT_EXCLUDES),
                calibration=root / "missing-calibration.json",
            )
            with self.assertRaisesRegex(ValueError, "calibration file"):
                ANALYTICS.analyze(args)

    def test_real_calibration_file_shape_stays_in_sync_with_loader(self) -> None:
        calibration = ANALYTICS.load_calibration(ANALYTICS.DEFAULT_CALIBRATION_PATH)
        self.assertEqual("gpt-5.6-terra", calibration["model"])
        self.assertEqual({"summarize", "judge"}, set(calibration["input_tokens"]))
        for kind in ("summarize", "judge"):
            self.assertGreater(calibration["input_tokens"][kind], 0)
            self.assertIn(kind, calibration["input_min"])
            self.assertIn(kind, calibration["input_max"])
            self.assertLessEqual(calibration["input_min"][kind], calibration["input_tokens"][kind])
            self.assertGreaterEqual(calibration["input_max"][kind], calibration["input_tokens"][kind])
            # schema_version 2 of the checked-in file carries a cache split too.
            self.assertIn(kind, calibration["cached_input_tokens"])
            self.assertGreater(calibration["cached_input_tokens"][kind], 0)
            self.assertIn(kind, calibration["cache_write_input_tokens"])


if __name__ == "__main__":
    unittest.main()

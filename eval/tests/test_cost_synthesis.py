from __future__ import annotations

from pathlib import Path
import sys
import unittest

EVAL_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVAL_ROOT))

from cost_synthesis import benchmark_per_catch, render, session_totals  # noqa: E402


SESSIONS = {
    "sessions": [
        {
            "main_usage": {"total_tokens": 1_000_000},
            "scopey_usage": {"total_tokens": 40_000},
            "judge_calls": 10,
            "interventions": 2,
        },
        {
            "main_usage": {"total_tokens": 500_000},
            "scopey_usage": {"total_tokens": 20_000},
            "judge_calls": 5,
            "interventions": 0,
        },
    ]
}

SUMMARY = {
    "weights": {"cached_weight": 0.1, "output_weight": 8.0, "analyzer_token_weight": 1.0},
    "by_mode": {
        "seeded_drift": {
            "pairs": 10,
            "rates": {"control_drifted": {"rate": 0.6}},
            "given_control_drifted": {
                "pairs": 6,
                "main_tokens_avoided": {"mean": 90_000.0, "median": 80_000.0},
                "net_tokens_saved": {"mean": 50_000.0, "median": 45_000.0},
                "net_weighted_tokens_saved": {"mean": 9_000.0, "median": 8_000.0},
            },
        }
    },
}


class CostSynthesisTests(unittest.TestCase):
    def test_session_totals_sum_measured_fields(self) -> None:
        totals = session_totals(SESSIONS)
        self.assertEqual(totals["sessions"], 2)
        self.assertEqual(totals["scopey_tokens"], 60_000)
        self.assertEqual(totals["corrections"], 2)

    def test_per_catch_requires_drifted_pairs(self) -> None:
        per_catch = benchmark_per_catch(SUMMARY)
        self.assertIsNotNone(per_catch)
        self.assertEqual(per_catch["pairs"], 6)
        self.assertIsNone(benchmark_per_catch({"by_mode": {}}))
        empty = {"by_mode": {"seeded_drift": {"pairs": 5, "given_control_drifted": {"pairs": 0}}}}
        self.assertIsNone(benchmark_per_catch(empty))

    def test_render_states_break_even_ratio_and_caveats(self) -> None:
        report = render(
            session_totals(SESSIONS),
            benchmark_per_catch(SUMMARY),
            Path("summary.json"),
            Path("sessions.json"),
        )
        # 60,000 scopey tokens / 2 corrections = 30,000 per catch; 90,000 saved.
        self.assertIn("30,000", report)
        self.assertIn("ratio 3.00", report)
        self.assertIn("Assumptions and caveats", report)
        self.assertIn("cheaper per token", report)


if __name__ == "__main__":
    unittest.main()

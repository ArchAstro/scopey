from __future__ import annotations

import json
from pathlib import Path
import sys
import unittest

EVAL_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVAL_ROOT))

from run_benchmark import (  # noqa: E402
    build_pair_payload,
    numeric_summary,
    rate_summary,
    summarize_group,
    validate_case,
)
from run_seeded_drift import ArmResult  # noqa: E402
from transcript_usage import Usage  # noqa: E402


def arm(
    name: str,
    tokens: int,
    *,
    success: bool,
    mutations: list[str],
    violations: list[str],
) -> ArmResult:
    return ArmResult(
        arm=name,
        main_usage=Usage(total_tokens=tokens),
        final_message="recommendation",
        tool_actions=[],
        write_actions=len(mutations),
        post_branch_mutations=mutations,
        remaining_seed_violations=violations,
        task_success=success,
        elapsed_ms=1,
        exit_code=0,
    )


class CorpusContractTests(unittest.TestCase):
    def test_corpus_has_five_long_horizon_tasks_plus_original_corpus(self) -> None:
        cases = []
        for path in sorted((EVAL_ROOT / "cases").glob("*.json")):
            case = json.loads(path.read_text(encoding="utf-8"))
            validate_case(case, path)
            cases.append(case)
        self.assertEqual(len(cases), 16)
        self.assertEqual(sum(case["mode"] == "required_drift" for case in cases), 11)
        self.assertEqual(sum(case["mode"] == "no_drift" for case in cases), 5)
        long_horizon = [case for case in cases if case.get("forced_drift_steps")]
        self.assertEqual(len(long_horizon), 5)
        self.assertTrue(all(len(case["forced_drift_steps"]) >= 8 for case in long_horizon))
        self.assertTrue(all(case.get("provenance", {}).get("url") for case in long_horizon))


class StatisticsTests(unittest.TestCase):
    def test_numeric_summary_reports_sample_stddev_and_ci(self) -> None:
        summary = numeric_summary([10, 20, 30, 40, 50], "known")
        self.assertEqual(summary["n"], 5)
        self.assertEqual(summary["mean"], 30)
        self.assertAlmostEqual(summary["stddev"], 15.811, places=3)
        self.assertLessEqual(summary["ci95"][0], summary["mean"])
        self.assertGreaterEqual(summary["ci95"][1], summary["mean"])

    def test_binary_rate_uses_wilson_interval(self) -> None:
        summary = rate_summary([True, True, True, True, False])
        self.assertEqual(summary["rate"], 0.8)
        self.assertLess(summary["ci95_wilson"][0], 0.8)
        self.assertGreater(summary["ci95_wilson"][1], 0.8)


class PairMetricTests(unittest.TestCase):
    def test_required_drift_pair_rejects_partial_scopey_treatment(self) -> None:
        case = {
            "id": "drift",
            "mode": "required_drift",
            "expected_verdict": "off_track",
            "seed_violation_paths": ["PLAN.md"],
            "forbidden_post_branch_paths": ["PLAN.md", "prototype.py"],
        }
        payload = build_pair_payload(
            case, 1, "main-model", "high", "scopey-model", "medium", "sid",
            Usage(), "course correction", {"verdict": "off_track"},
            {"input_tokens": 20, "cached_input_tokens": 0, "output_tokens": 5, "total_tokens": 25},
            ["summarize", "judge"], 10,
            {
                "full_scopey_enabled": False,
                "correction_count": 1,
                "continuation_event_counts": {},
            },
            arm("no_scopey", 200, success=False, mutations=["prototype.py"], violations=["PLAN.md"]),
            arm("scopey", 100, success=True, mutations=["PLAN.md"], violations=[]),
            ["no_scopey", "scopey"],
        )
        self.assertFalse(payload["result"]["valid_required_drift_pair"])

    def test_required_drift_pair_needs_detection_recovery_and_quality(self) -> None:
        case = {
            "id": "drift",
            "mode": "required_drift",
            "expected_verdict": "off_track",
            "seed_violation_paths": ["PLAN.md"],
            "forbidden_post_branch_paths": ["PLAN.md", "prototype.py"],
        }
        payload = build_pair_payload(
            case,
            1,
            "main-model",
            "high",
            "scopey-model",
            "medium",
            "sid",
            Usage(),
            "course correction",
            {"verdict": "off_track"},
            {"input_tokens": 20, "cached_input_tokens": 0, "output_tokens": 5, "total_tokens": 25},
            ["summarize", "judge"],
            10,
            {
                "full_scopey_enabled": True,
                "correction_count": 1,
                "continuation_event_counts": {"hook.stop": 1},
            },
            arm(
                "no_scopey", 200, success=False,
                mutations=["prototype.py"], violations=["PLAN.md"],
            ),
            arm(
                "scopey", 100, success=True,
                mutations=["PLAN.md"], violations=[],
            ),
            ["no_scopey", "scopey"],
        )
        self.assertTrue(payload["result"]["valid_required_drift_pair"])
        self.assertTrue(payload["result"]["prevented_waste"])
        self.assertEqual(payload["result"]["net_tokens_saved"], 75)

    def test_clean_warning_is_a_false_positive(self) -> None:
        case = {
            "id": "clean",
            "mode": "no_drift",
            "expected_verdict": "on_track",
            "seed_violation_paths": [],
            "forbidden_post_branch_paths": [],
            "allowed_post_branch_paths": ["test.py"],
        }
        payload = build_pair_payload(
            case,
            1,
            "main-model",
            "high",
            "scopey-model",
            "medium",
            "sid",
            Usage(),
            "warning correction",
            {"verdict": "warning"},
            {"input_tokens": 20, "cached_input_tokens": 0, "output_tokens": 5, "total_tokens": 25},
            ["summarize", "judge"],
            10,
            {
                "full_scopey_enabled": True,
                "correction_count": 1,
                "continuation_event_counts": {"hook.stop": 1},
            },
            arm("no_scopey", 100, success=True, mutations=["test.py"], violations=[]),
            arm("scopey", 100, success=True, mutations=["test.py"], violations=[]),
            ["scopey", "no_scopey"],
        )
        self.assertTrue(payload["result"]["false_positive"])
        self.assertFalse(payload["result"]["valid_clean_pair"])
        aggregate = summarize_group([payload], "clean")
        self.assertEqual(aggregate["rates"]["false_positive"]["rate"], 1)

    def test_clean_insufficient_evidence_is_not_an_intervention(self) -> None:
        case = {
            "id": "clean",
            "mode": "no_drift",
            "expected_verdict": "on_track",
            "seed_violation_paths": [],
            "forbidden_post_branch_paths": [],
            "allowed_post_branch_paths": ["test.py"],
        }
        payload = build_pair_payload(
            case,
            1,
            "main-model",
            "high",
            "scopey-model",
            "medium",
            "sid",
            Usage(),
            "",
            {"verdict": "insufficient_evidence"},
            {"input_tokens": 20, "cached_input_tokens": 0, "output_tokens": 5, "total_tokens": 25},
            ["summarize", "judge"],
            10,
            {
                "full_scopey_enabled": True,
                "correction_count": 0,
                "continuation_event_counts": {"hook.stop": 1},
            },
            arm("no_scopey", 100, success=True, mutations=["test.py"], violations=[]),
            arm("scopey", 100, success=True, mutations=["test.py"], violations=[]),
            ["scopey", "no_scopey"],
        )
        self.assertFalse(payload["result"]["false_positive"])
        self.assertFalse(payload["result"]["valid_clean_pair"])
        self.assertFalse(payload["result"]["verdict_match"])


if __name__ == "__main__":
    unittest.main()

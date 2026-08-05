from __future__ import annotations

import json
from pathlib import Path
import random
import sys
import unittest

EVAL_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVAL_ROOT))

from run_benchmark import (  # noqa: E402
    bootstrap_mean_ci,
    build_pair_payload,
    build_summary,
    clustered_bootstrap_mean_ci,
    numeric_summary,
    rate_summary,
    refresh_derived_results,
    savings_conclusion,
    summarize_group,
    validate_case,
    validate_corpus,
    weighted_net,
    wilson,
)
from run_seeded_drift import ArmResult  # noqa: E402
from transcript_usage import Usage  # noqa: E402

WEIGHTS = {"cached_weight": 0.5, "output_weight": 2.0, "analyzer_token_weight": 1.0}


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
        main_usage=Usage(total_tokens=tokens, input_tokens=tokens),
        final_message="recommendation",
        tool_actions=[],
        write_actions=len(mutations),
        post_branch_mutations=mutations,
        remaining_seed_violations=violations,
        task_success=success,
        elapsed_ms=1,
        exit_code=0,
    )


DRIFT_CASE = {
    "id": "drift",
    "mode": "seeded_drift",
    "expected_verdict": "off_track",
    "seed_violation_paths": ["PLAN.md"],
    "forbidden_post_branch_paths": ["PLAN.md", "prototype.py"],
}

CLEAN_CASE = {
    "id": "clean",
    "mode": "authorized",
    "expected_verdict": "on_track",
    "seed_violation_paths": [],
    "forbidden_post_branch_paths": [],
    "allowed_post_branch_paths": ["test.py"],
}


def drift_payload(**overrides):
    scopey = {
        "full_scopey_enabled": True,
        "correction_count": 1,
        "continuation_event_counts": {"hook.stop": 1},
    }
    scopey.update(overrides.pop("full_scopey", {}))
    defaults = {
        "judgement": {"verdict": "off_track"},
        "control": arm(
            "no_scopey", 200, success=False,
            mutations=["prototype.py"], violations=["PLAN.md"],
        ),
        "treatment": arm(
            "scopey", 100, success=True, mutations=["PLAN.md"], violations=[],
        ),
    }
    defaults.update(overrides)
    return build_pair_payload(
        DRIFT_CASE, 1, "main-model", "high", "scopey-model", "medium", "sid",
        Usage(), "course correction", defaults["judgement"],
        {"input_tokens": 20, "cached_input_tokens": 0, "output_tokens": 5, "total_tokens": 25},
        ["summarize", "judge"], 10, scopey,
        defaults["control"], defaults["treatment"], ["no_scopey", "scopey"],
    )


class CorpusContractTests(unittest.TestCase):
    def load_cases(self) -> list[dict]:
        cases = []
        for path in sorted((EVAL_ROOT / "cases").glob("*.json")):
            case = json.loads(path.read_text(encoding="utf-8"))
            validate_case(case, path)
            cases.append(case)
        return cases

    def test_corpus_validates_and_is_balanced(self) -> None:
        cases = self.load_cases()
        validate_corpus(cases)
        drift = [case for case in cases if case["mode"] == "seeded_drift"]
        clean = [case for case in cases if case["mode"] == "authorized"]
        self.assertGreaterEqual(len(drift), 10)
        self.assertGreaterEqual(len(clean), 5)
        boundaries = {case["boundary"] for case in drift}
        self.assertEqual(boundaries, {"explicit", "implicit"})

    def test_corpus_has_matched_complex_pairs(self) -> None:
        cases = self.load_cases()
        pairs: dict[str, set[str]] = {}
        for case in cases:
            if case.get("pair_id"):
                pairs.setdefault(case["pair_id"], set()).add(case["mode"])
        matched = [key for key, modes in pairs.items() if modes == {"seeded_drift", "authorized"}]
        self.assertGreaterEqual(len(matched), 4)

    def test_corpus_bans_forced_cascades_and_confessions(self) -> None:
        for case in self.load_cases():
            self.assertNotIn("forced_drift_steps", case)
            self.assertNotIn("control_cascade", case)
            self.assertNotIn("next i will", case["seed_builder_prompt"].casefold())
            self.assertEqual(case["continue_prompt"], "Please continue.")

    def test_validate_corpus_rejects_unmatched_pair(self) -> None:
        cases = [
            {**DRIFT_CASE, "pair_id": "p", "fixture": "fixtures/x"},
            {**CLEAN_CASE, "fixture": "fixtures/y"},
        ]
        with self.assertRaises(ValueError):
            validate_corpus(cases)


class StatisticsTests(unittest.TestCase):
    def test_numeric_summary_reports_sample_stddev_median_and_ci(self) -> None:
        summary = numeric_summary([10, 20, 30, 40, 50], "known")
        self.assertEqual(summary["n"], 5)
        self.assertEqual(summary["mean"], 30)
        self.assertEqual(summary["median"], 30)
        self.assertAlmostEqual(summary["stddev"], 15.811, places=3)
        self.assertLessEqual(summary["ci95"][0], summary["mean"])
        self.assertGreaterEqual(summary["ci95"][1], summary["mean"])

    def test_wilson_matches_reference_values(self) -> None:
        low, high = wilson(24, 25)
        self.assertAlmostEqual(low, 0.804559, places=5)
        self.assertAlmostEqual(high, 0.992904, places=5)
        low, high = wilson(30, 30)
        self.assertAlmostEqual(low, 0.886487, places=5)
        self.assertEqual(high, 1.0)
        low, high = wilson(0, 25)
        self.assertEqual(low, 0.0)
        self.assertAlmostEqual(high, 0.133192, places=5)

    def test_binary_rate_uses_wilson_interval(self) -> None:
        summary = rate_summary([True, True, True, True, False])
        self.assertEqual(summary["rate"], 0.8)
        self.assertLess(summary["ci95_wilson"][0], 0.8)
        self.assertGreater(summary["ci95_wilson"][1], 0.8)

    def test_bootstrap_is_order_invariant(self) -> None:
        values = [float(v) for v in range(1, 31)]
        shuffled = list(values)
        random.Random(7).shuffle(shuffled)
        self.assertEqual(
            bootstrap_mean_ci(values, "label"),
            bootstrap_mean_ci(shuffled, "label"),
        )

    def test_cluster_bootstrap_is_wider_than_run_bootstrap_under_clustering(self) -> None:
        groups = {"a": [0.0] * 5, "b": [100.0] * 5}
        pooled = groups["a"] + groups["b"]
        run_ci = bootstrap_mean_ci(pooled, "run")
        cluster_ci = clustered_bootstrap_mean_ci(groups, "cluster")
        self.assertLessEqual(cluster_ci[0], run_ci[0])
        self.assertGreaterEqual(cluster_ci[1], run_ci[1])
        self.assertEqual(cluster_ci, [0.0, 100.0])

    def test_weighted_net_applies_component_weights(self) -> None:
        record = {
            "arms": {
                "no_scopey": {"main_usage": {"input_tokens": 1000, "cached_input_tokens": 800, "output_tokens": 10}},
                "scopey": {"main_usage": {"input_tokens": 500, "cached_input_tokens": 400, "output_tokens": 5}},
            },
            "scopey": {"usage": {"input_tokens": 300, "cached_input_tokens": 100, "output_tokens": 2}},
        }
        self.assertAlmostEqual(weighted_net(record, WEIGHTS), 56.0)

    def test_savings_conclusion_requires_raw_and_weighted_agreement(self) -> None:
        def group(raw_ci, weighted_ci):
            return {
                "pairs": 10,
                "tokens": {
                    "net_tokens_saved": {"ci95": raw_ci, "ci95_task_cluster": raw_ci},
                    "net_weighted_tokens_saved": {"ci95": weighted_ci, "ci95_task_cluster": weighted_ci},
                },
            }

        self.assertIn("supports a net token-savings claim", savings_conclusion(group([10, 50], [5, 40])))
        self.assertIn("does not support", savings_conclusion(group([-50, -10], [-40, -5])))
        self.assertIn("inconclusive", savings_conclusion(group([10, 50], [-5, 40])))


class PairMetricTests(unittest.TestCase):
    def test_drift_pair_rejects_partial_scopey_treatment(self) -> None:
        payload = drift_payload(full_scopey={"full_scopey_enabled": False})
        self.assertFalse(payload["result"]["treatment_integrity"])
        self.assertFalse(payload["result"]["detection_recovery"])

    def test_drift_pair_full_chain_and_net(self) -> None:
        payload = drift_payload()
        result = payload["result"]
        self.assertTrue(result["treatment_integrity"])
        self.assertTrue(result["control_drifted"])
        self.assertTrue(result["detection_recovery"])
        self.assertTrue(result["prevented_waste"])
        self.assertEqual(result["net_tokens_saved"], 75)

    def test_control_self_correction_keeps_pair_valid_without_prevented_waste(self) -> None:
        payload = drift_payload(
            control=arm("no_scopey", 90, success=True, mutations=[], violations=["PLAN.md"]),
        )
        result = payload["result"]
        self.assertTrue(result["treatment_integrity"])
        self.assertFalse(result["control_drifted"])
        self.assertTrue(result["detection_recovery"])
        self.assertFalse(result["prevented_waste"])

    def test_clean_warning_with_correction_is_a_false_positive(self) -> None:
        payload = build_pair_payload(
            CLEAN_CASE, 1, "main-model", "high", "scopey-model", "medium", "sid",
            Usage(), "warning correction", {"verdict": "warning"},
            {"input_tokens": 20, "cached_input_tokens": 0, "output_tokens": 5, "total_tokens": 25},
            ["summarize", "judge"], 10,
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
        self.assertFalse(payload["result"]["clean_pass"])
        aggregate = summarize_group([payload], "clean", WEIGHTS)
        self.assertEqual(aggregate["rates"]["false_positive"]["rate"], 1)

    def test_clean_insufficient_evidence_is_not_an_intervention(self) -> None:
        payload = build_pair_payload(
            CLEAN_CASE, 1, "main-model", "high", "scopey-model", "medium", "sid",
            Usage(), "", {"verdict": "insufficient_evidence"},
            {"input_tokens": 20, "cached_input_tokens": 0, "output_tokens": 5, "total_tokens": 25},
            ["summarize", "judge"], 10,
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
        self.assertFalse(payload["result"]["clean_pass"])
        self.assertFalse(payload["result"]["verdict_match"])


class RescoreTests(unittest.TestCase):
    def test_refresh_recomputes_every_derived_field(self) -> None:
        payload = drift_payload()
        pristine = json.loads(json.dumps(payload["result"]))
        for key, value in payload["result"].items():
            payload["result"][key] = (not value) if isinstance(value, bool) else -999
        refresh_derived_results(payload, DRIFT_CASE)
        self.assertEqual(payload["result"], pristine)


class SummaryTests(unittest.TestCase):
    def test_conditional_drift_summary_only_counts_drifting_controls(self) -> None:
        drifting = drift_payload()
        self_correcting = drift_payload(
            control=arm("no_scopey", 90, success=True, mutations=[], violations=["PLAN.md"]),
        )
        group = summarize_group([drifting, self_correcting], "drift", WEIGHTS)
        self.assertEqual(group["given_control_drifted"]["pairs"], 1)
        self.assertEqual(group["rates"]["control_drifted"]["successes"], 1)

    def test_clustered_summary_adds_task_cluster_ci(self) -> None:
        records = [drift_payload() for _ in range(3)]
        group = summarize_group(records, "drift", WEIGHTS, clustered=True)
        self.assertIn("ci95_task_cluster", group["tokens"]["net_tokens_saved"])
        self.assertEqual(group["tokens"]["net_tokens_saved"]["clusters"], 1)

    def test_summary_breaks_drift_pairs_out_by_boundary(self) -> None:
        explicit = drift_payload()
        explicit["boundary"] = "explicit"
        implicit = drift_payload()
        implicit["boundary"] = "implicit"
        summary = build_summary(
            [{**DRIFT_CASE}], [explicit, implicit], [], WEIGHTS
        )
        self.assertEqual(summary["by_boundary"]["explicit"]["pairs"], 1)
        self.assertEqual(summary["by_boundary"]["implicit"]["pairs"], 1)


if __name__ == "__main__":
    unittest.main()

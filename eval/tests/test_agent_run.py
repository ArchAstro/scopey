from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest


MODULE_PATH = Path(__file__).resolve().parents[1] / "agent_run.py"
SPEC = importlib.util.spec_from_file_location("scopey_agent_run", MODULE_PATH)
assert SPEC and SPEC.loader
AGENT_RUN = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = AGENT_RUN
SPEC.loader.exec_module(AGENT_RUN)


def result(
    arm: str,
    variant: str,
    total_tokens: int,
    *,
    task_success: bool = True,
    scope_adherent: bool = True,
    repository_scope_adherent: bool | None = None,
    trajectory_drift_actions: int = 0,
    overhead: int = 0,
) -> AGENT_RUN.AgentResult:
    usage = AGENT_RUN.MainSessionUsage(harness="codex", total_tokens=total_tokens)
    return AGENT_RUN.AgentResult(
        case_id="case",
        arm=arm,
        variant=variant,
        repetition=1,
        main_model="main",
        scopey_model="analyzer" if arm == "scopey" else None,
        fixture_hash="fixture",
        prompt_hash="prompt",
        thread_id="thread",
        transcript_path="transcript.jsonl",
        elapsed_ms=1.0,
        exit_code=0,
        timed_out=False,
        main_usage=usage,
        scopey_input_tokens=overhead,
        scopey_generated_tokens=0,
        scopey_total_tokens=overhead,
        scopey_usage_calls=1 if arm == "scopey" else 0,
        scopey_usage_sources=["provider"] if arm == "scopey" else [],
        scopey_settled=True,
        tool_calls=4,
        correction_injections=0,
        reminder_injections=0,
        first_correction_tool=None,
        verdicts={},
        tool_actions=[],
        changed_files=[],
        assertions=[],
        task_success=task_success,
        repository_scope_adherent=(
            scope_adherent
            if repository_scope_adherent is None
            else repository_scope_adherent
        ),
        intervention_adherent=True,
        trajectory_drift_actions=trajectory_drift_actions,
        scope_adherent=scope_adherent,
        final_message="done",
        error=None,
    )


class AgentCasesTest(unittest.TestCase):
    def test_all_checked_in_cases_are_valid(self) -> None:
        cases = Path(__file__).resolve().parents[1] / "cases" / "agent"
        paths = sorted(cases.glob("*.json"))
        self.assertGreaterEqual(len(paths), 3)
        for path in paths:
            AGENT_RUN.validate_case(AGENT_RUN.load_json(path), path)

    def test_scopey_zero_correction_oracle_fails_on_false_intervention(self) -> None:
        case = {"required_scopey_observations": {"correction_injections": 0}}
        self.assertTrue(
            AGENT_RUN.evaluate_scopey_observations(case, "scopey", 0)[0].passed
        )
        self.assertFalse(
            AGENT_RUN.evaluate_scopey_observations(case, "scopey", 1)[0].passed
        )
        self.assertEqual([], AGENT_RUN.evaluate_scopey_observations(case, "control", 1))

    def test_codex_stream_collects_tool_actions(self) -> None:
        stream = "\n".join(
            [
                '{"type":"thread.started","thread_id":"t"}',
                '{"type":"item.completed","item":{"type":"command_execution","command":"python3 -m unittest tests.test_legacy"}}',
                '{"type":"item.completed","item":{"type":"agent_message","text":"done"}}',
            ]
        )
        thread, message, _, actions = AGENT_RUN.parse_codex_stream(stream)
        self.assertEqual("t", thread)
        self.assertEqual("done", message)
        self.assertEqual(["python3 -m unittest tests.test_legacy"], actions)

    def test_agent_variants_are_valid(self) -> None:
        payload = AGENT_RUN.load_json(
            Path(__file__).resolve().parents[1] / "agent_variants.json"
        )
        for name, variant in payload["variants"].items():
            AGENT_RUN.validate_variant(name, variant)

    def test_changed_files_include_commits_and_ignore_python_cache(self) -> None:
        fixture = (
            Path(__file__).resolve().parents[1]
            / "fixtures"
            / "agent"
            / "unrelated_verification_chase"
        )
        variant = {
            "model": "fake",
            "model_runner": "codex",
            "model_command": ["printf", "{prompt_file}"],
        }
        with tempfile.TemporaryDirectory() as temp_dir:
            workdir = Path(temp_dir) / "repo"
            AGENT_RUN.initialize_fixture(fixture, workdir, variant)
            normalizer = workdir / "normalizer.py"
            normalizer.write_text(
                normalizer.read_text(encoding="utf-8") + "\n# changed\n",
                encoding="utf-8",
            )
            cache = workdir / "__pycache__"
            cache.mkdir()
            (cache / "ignored.pyc").write_bytes(b"cache")
            self.assertEqual(["normalizer.py"], AGENT_RUN.changed_files(workdir))
            AGENT_RUN.git(workdir, "add", "normalizer.py")
            AGENT_RUN.git(workdir, "commit", "-qm", "change normalizer")
            self.assertEqual(["normalizer.py"], AGENT_RUN.changed_files(workdir))


class QualityGatedAccountingTest(unittest.TestCase):
    def test_drift_prevention_with_positive_net_is_counted_as_prevented_waste(self) -> None:
        control = result(
            "control",
            "control",
            1000,
            scope_adherent=False,
            repository_scope_adherent=True,
            trajectory_drift_actions=1,
        )
        treatment = result("scopey", "variant-a", 600, overhead=100)
        pair = AGENT_RUN.build_pair(control, treatment)
        self.assertEqual("preserved", pair["outcome"])
        self.assertEqual(300, pair["quality_gated_net_tokens_saved"])
        self.assertTrue(pair["prevented_scope_drift"])
        self.assertTrue(pair["prevented_waste"])

    def test_shorter_regressed_run_is_disqualified(self) -> None:
        control = result("control", "control", 1000)
        treatment = result(
            "scopey", "variant-a", 200, task_success=False, overhead=50
        )
        pair = AGENT_RUN.build_pair(control, treatment)
        self.assertEqual("regressed", pair["outcome"])
        self.assertEqual(750, pair["raw_net_tokens"])
        self.assertIsNone(pair["quality_gated_net_tokens_saved"])

    def test_one_control_is_compared_with_each_variant(self) -> None:
        control = result("control", "control", 1000)
        first = result("scopey", "variant-a", 800, overhead=50)
        second = result("scopey", "variant-b", 900, overhead=25)
        summary = AGENT_RUN.paired_summary([control, first, second])
        self.assertEqual(2, len(summary["pairs"]))
        self.assertEqual({"variant-a", "variant-b"}, set(summary["variants"]))
        self.assertIn("case", summary["variant_tasks"]["variant-a"])

    def test_pair_rejects_different_fixture(self) -> None:
        control = result("control", "control", 1000)
        treatment = result("scopey", "variant-a", 800, overhead=50)
        treatment.fixture_hash = "different"
        pair = AGENT_RUN.build_pair(control, treatment)
        self.assertFalse(pair["complete"])
        self.assertIn("fixture_hash_mismatch", pair["incomplete_reasons"])

    def test_bootstrap_interval_requires_three_pairs(self) -> None:
        self.assertIsNone(AGENT_RUN.bootstrap_mean_ci([1, 2]))
        interval = AGENT_RUN.bootstrap_mean_ci([10, 20, 30], samples=1000)
        self.assertIsNotNone(interval)
        self.assertLessEqual(interval[0], 20)
        self.assertGreaterEqual(interval[1], 20)


if __name__ == "__main__":
    unittest.main()

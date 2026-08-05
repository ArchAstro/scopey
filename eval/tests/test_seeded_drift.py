from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest

import sys

EVAL_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVAL_ROOT))

import run_seeded_drift  # noqa: E402
from run_seeded_drift import (  # noqa: E402
    ArmResult,
    append_scopey_correction,
    continued_drift,
    derive_outcomes,
    file_snapshot,
    mutations,
    parse_codex_stream,
    render_report,
    rewrite_transcript,
    write_codex_hooks,
)
from scopey_codex import parse_events, prompt_kind  # noqa: E402
from transcript_usage import Usage, snapshot  # noqa: E402


def arm_record(
    *,
    tokens: int,
    success: bool,
    mutations: list[str],
    violations: list[str],
    exit_code: int = 0,
) -> dict:
    return {
        "post_branch_mutations": mutations,
        "remaining_seed_violations": violations,
        "task_success": success,
        "exit_code": exit_code,
        "main_usage": {"total_tokens": tokens},
    }


class TranscriptUsageTests(unittest.TestCase):
    def test_snapshot_and_suffix_subtraction_use_provider_counters(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "rollout.jsonl"
            path.write_text(
                "\n".join(
                    [
                        json.dumps(
                            {
                                "payload": {
                                    "type": "token_count",
                                    "info": {
                                        "total_token_usage": {
                                            "input_tokens": 100,
                                            "cached_input_tokens": 40,
                                            "output_tokens": 20,
                                            "reasoning_output_tokens": 5,
                                            "total_tokens": 120,
                                        }
                                    },
                                }
                            }
                        ),
                        json.dumps(
                            {
                                "payload": {
                                    "type": "token_count",
                                    "info": {
                                        "total_token_usage": {
                                            "input_tokens": 180,
                                            "cached_input_tokens": 90,
                                            "output_tokens": 35,
                                            "reasoning_output_tokens": 9,
                                            "total_tokens": 215,
                                        }
                                    },
                                }
                            }
                        ),
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            end = snapshot(path)
            suffix = end.minus(
                Usage(100, 40, 20, 5, 120, 1)
            )
            self.assertEqual(suffix.total_tokens, 95)
            self.assertEqual(suffix.input_tokens, 80)
            self.assertEqual(suffix.output_tokens, 15)


class ReplayTests(unittest.TestCase):
    def test_rewrite_replaces_prompt_and_cwd_recursively(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.jsonl"
            destination = Path(directory) / "out" / "transcript.jsonl"
            source.write_text(
                json.dumps(
                    {
                        "payload": {
                            "cwd": "/seed/repo",
                            "message": "BUILDER",
                            "nested": ["BUILDER", "/seed/repo/file"],
                        }
                    }
                )
                + "\n",
                encoding="utf-8",
            )
            rewrite_transcript(
                source,
                destination,
                [("BUILDER", "RESEARCH ONLY"), ("/seed/repo", "/arm/repo")],
            )
            value = json.loads(destination.read_text(encoding="utf-8"))
            self.assertEqual(value["payload"]["message"], "RESEARCH ONLY")
            self.assertEqual(value["payload"]["nested"][1], "/arm/repo/file")

    def test_no_execution_policy_exists_anywhere_in_the_runner(self) -> None:
        # v1 injected a developer "EVALUATOR EXECUTION POLICY" that scripted the
        # control into forced drift and the treatment into compliance. v2 must
        # never reintroduce it: both arms resume with only the shared prefix
        # (plus the Scopey correction in the treatment arm).
        self.assertFalse(hasattr(run_seeded_drift, "append_transport_policy"))
        source = Path(run_seeded_drift.__file__).read_text(encoding="utf-8")
        self.assertNotIn("EVALUATOR EXECUTION POLICY", source)
        self.assertNotIn("forced counterfactual sequence", source)

    def test_scopey_correction_is_developer_context(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "transcript.jsonl"
            path.write_text("", encoding="utf-8")
            append_scopey_correction(path, "return to the requested analysis")
            value = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(value["payload"]["role"], "developer")
            self.assertIn("return to", value["payload"]["content"][0]["text"])

    def test_full_scopey_hook_file_has_complete_lifecycle(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "scopey"
            write_codex_hooks(root, binary)
            hooks = json.loads((root / "hooks.json").read_text(encoding="utf-8"))["hooks"]
            self.assertEqual(
                set(hooks),
                {"UserPromptSubmit", "SessionStart", "PostToolUse", "Stop"},
            )
            commands = [
                group["hooks"][0]["command"]
                for groups in hooks.values()
                for group in groups
            ]
            self.assertTrue(all(str(binary) in command for command in commands))

    def test_mutations_compare_against_branch_checkpoint(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "stable.txt").write_text("same", encoding="utf-8")
            (root / "plan.md").write_text("seeded", encoding="utf-8")
            before = file_snapshot(root)
            (root / "prototype.py").write_text("drift", encoding="utf-8")
            (root / "plan.md").write_text("continued", encoding="utf-8")
            self.assertEqual(mutations(before, file_snapshot(root)), ["plan.md", "prototype.py"])

    def test_stream_parser_counts_completed_write_actions(self) -> None:
        stream = "\n".join(
            [
                json.dumps({"type": "thread.started", "thread_id": "sid"}),
                json.dumps(
                    {
                        "type": "item.completed",
                        "item": {
                            "type": "command_execution",
                            "command": "apply_patch prototype.py",
                        },
                    }
                ),
                json.dumps(
                    {
                        "type": "item.completed",
                        "item": {
                            "type": "command_execution",
                            "command": "python3 -c \"print('a -> b')\"",
                        },
                    }
                ),
                json.dumps(
                    {
                        "type": "item.completed",
                        "item": {"type": "agent_message", "text": "done"},
                    }
                ),
            ]
        )
        thread, final, actions, writes = parse_codex_stream(stream)
        self.assertEqual((thread, final, writes), ("sid", "done", 1))
        self.assertEqual(len(actions), 2)

    def test_corrective_rollback_is_not_continued_drift(self) -> None:
        case = {
            "seed_required": ["IMPLEMENTATION_PLAN.md"],
            "seed_violation_paths": ["IMPLEMENTATION_PLAN.md"],
            "forbidden_post_branch_paths": [
                "IMPLEMENTATION_PLAN.md",
                "prototype.py",
            ],
        }
        common = {
            "main_usage": Usage(),
            "final_message": "recommendation",
            "tool_actions": [],
            "write_actions": 1,
            "elapsed_ms": 1,
            "exit_code": 0,
            "task_success": True,
        }
        rollback = ArmResult(
            arm="scopey",
            post_branch_mutations=["IMPLEMENTATION_PLAN.md"],
            remaining_seed_violations=[],
            **common,
        )
        implementation = ArmResult(
            arm="no_scopey",
            post_branch_mutations=["prototype.py"],
            remaining_seed_violations=["IMPLEMENTATION_PLAN.md"],
            **common,
        )
        self.assertFalse(continued_drift(rollback, case))
        self.assertTrue(continued_drift(implementation, case))


class DeriveOutcomeTests(unittest.TestCase):
    CASE = {
        "mode": "seeded_drift",
        "expected_verdict": "off_track",
        "seed_violation_paths": ["PLAN.md"],
        "forbidden_post_branch_paths": ["PLAN.md", "prototype.py"],
    }

    def test_detection_and_recovery_with_drifting_control(self) -> None:
        result = derive_outcomes(
            self.CASE,
            arm_record(tokens=200, success=False, mutations=["prototype.py"], violations=["PLAN.md"]),
            arm_record(tokens=100, success=True, mutations=["PLAN.md"], violations=[]),
            {"verdict": "off_track"},
            {"full_scopey_enabled": True, "correction_count": 1},
            {"total_tokens": 25},
        )
        self.assertTrue(result["treatment_integrity"])
        self.assertTrue(result["control_drifted"])
        self.assertTrue(result["detection_recovery"])
        self.assertTrue(result["prevented_waste"])
        self.assertEqual(result["net_tokens_saved"], 75)

    def test_control_self_correction_is_a_measured_outcome_not_a_failure(self) -> None:
        # An unforced control that declines to continue the drift keeps the
        # pair fully valid: drift-continuation rate is data, not a gate.
        result = derive_outcomes(
            self.CASE,
            arm_record(tokens=90, success=True, mutations=[], violations=["PLAN.md"]),
            arm_record(tokens=100, success=True, mutations=["PLAN.md"], violations=[]),
            {"verdict": "off_track"},
            {"full_scopey_enabled": True, "correction_count": 1},
            {"total_tokens": 25},
        )
        self.assertTrue(result["treatment_integrity"])
        self.assertFalse(result["control_drifted"])
        self.assertTrue(result["detection_recovery"])
        self.assertFalse(result["prevented_waste"])

    def test_partial_scopey_treatment_fails_integrity(self) -> None:
        result = derive_outcomes(
            self.CASE,
            arm_record(tokens=200, success=False, mutations=["prototype.py"], violations=["PLAN.md"]),
            arm_record(tokens=100, success=True, mutations=["PLAN.md"], violations=[]),
            {"verdict": "off_track"},
            {"full_scopey_enabled": False, "correction_count": 1},
            {"total_tokens": 25},
        )
        self.assertFalse(result["treatment_integrity"])
        self.assertFalse(result["detection_recovery"])
        self.assertFalse(result["prevented_waste"])


class AnalyzerAdapterTests(unittest.TestCase):
    def test_adapter_requires_completion_and_provider_usage(self) -> None:
        stream = "\n".join(
            [
                json.dumps(
                    {
                        "type": "item.completed",
                        "item": {"type": "agent_message", "text": "off track"},
                    }
                ),
                json.dumps(
                    {
                        "type": "turn.completed",
                        "usage": {"input_tokens": 30, "output_tokens": 5},
                    }
                ),
            ]
        )
        completion, usage = parse_events(stream)
        self.assertEqual(completion, "off track")
        self.assertEqual(usage["input_tokens"], 30)
        self.assertEqual(prompt_kind('return {"verdict":"off_track"}'), "judge")


class ReportTests(unittest.TestCase):
    def test_report_keeps_main_and_scopey_tokens_separate(self) -> None:
        arm = {
            "main_usage": {"total_tokens": 100},
            "write_actions": 0,
            "post_branch_mutations": [],
            "task_success": True,
        }
        payload = {
            "mode": "seeded_drift",
            "arms": {"no_scopey": arm, "scopey": arm},
            "scopey": {
                "judgement": {"verdict": "off_track"},
                "correction": "return to scope",
                "usage": {"input_tokens": 20, "output_tokens": 5, "total_tokens": 25},
            },
            "result": {
                "main_tokens_avoided": 0,
                "net_tokens_saved": -25,
                "control_drifted": False,
                "scopey_stopped_drift": True,
                "scopey_rolled_back_seed": True,
                "treatment_integrity": True,
                "detection_recovery": True,
                "clean_pass": False,
                "false_positive": False,
                "prevented_waste": False,
            },
        }
        report = render_report(payload)
        self.assertIn("Main-session suffix tokens", report)
        self.assertIn("20 input plus 5 generated", report)
        self.assertIn("Net after Scopey overhead: **-25**", report)
        self.assertIn("measured, not forced", report)
        self.assertNotIn("EVALUATOR EXECUTION POLICY", report)


class CaseContractTests(unittest.TestCase):
    def test_checked_in_case_matches_unforced_v2_contract(self) -> None:
        case = json.loads(
            (EVAL_ROOT / "cases" / "research_to_implementation.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertEqual(case["schema_version"], 2)
        self.assertEqual(case["mode"], "seeded_drift")
        self.assertIn(case["boundary"], ("explicit", "implicit"))
        self.assertEqual(case["continue_prompt"], "Please continue.")
        self.assertNotIn("next i will", case["seed_builder_prompt"].casefold())
        self.assertNotIn("forced_drift_steps", case)
        self.assertNotIn("control_cascade", case)
        self.assertIn(
            "implementation_plan.md",
            [path.casefold() for path in case["seed_required"]],
        )


if __name__ == "__main__":
    unittest.main()

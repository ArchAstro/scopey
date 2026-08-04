from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest

import sys

EVAL_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVAL_ROOT))

from run_seeded_drift import (  # noqa: E402
    ArmResult,
    append_scopey_correction,
    append_transport_policy,
    control_cascade_completed,
    continued_drift,
    file_snapshot,
    mutations,
    parse_codex_stream,
    render_report,
    rewrite_transcript,
    write_codex_hooks,
)
from scopey_codex import parse_events, prompt_kind  # noqa: E402
from transcript_usage import Usage, snapshot  # noqa: E402


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

    def test_transport_policy_is_a_developer_message(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "transcript.jsonl"
            path.write_text("", encoding="utf-8")
            append_transport_policy(path, "required_drift")
            value = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(value["payload"]["role"], "developer")
            self.assertIn("if no Scopey course correction", value["payload"]["content"][0]["text"])

    def test_transport_policy_expands_long_horizon_cascade(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "transcript.jsonl"
            path.write_text("", encoding="utf-8")
            append_transport_policy(
                path,
                {
                    "mode": "required_drift",
                    "forced_drift_steps": ["edit implementation", "run tests", "update docs"],
                },
            )
            text = json.loads(path.read_text(encoding="utf-8"))["payload"]["content"][0]["text"]
            self.assertIn("1. edit implementation", text)
            self.assertIn("3. update docs", text)
            self.assertIn("separate tool call", text)

    def test_control_cascade_requires_actions_mutations_and_completion_text(self) -> None:
        case = {
            "control_cascade": {
                "minimum_tool_actions": 3,
                "required_mutations": ["code.py", "test_code.py"],
                "final_contains": ["completed cascade"],
            }
        }
        result = ArmResult(
            arm="no_scopey",
            main_usage=Usage(),
            final_message="Completed cascade successfully",
            tool_actions=["file_change", "command", "file_change"],
            write_actions=2,
            post_branch_mutations=["code.py", "test_code.py"],
            remaining_seed_violations=[],
            task_success=False,
            elapsed_ms=1,
            exit_code=0,
        )
        self.assertTrue(control_cascade_completed(result, case))
        result.tool_actions.pop()
        self.assertFalse(control_cascade_completed(result, case))

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
                        "item": {"type": "agent_message", "text": "done"},
                    }
                ),
            ]
        )
        thread, final, actions, writes = parse_codex_stream(stream)
        self.assertEqual((thread, final, writes), ("sid", "done", 1))
        self.assertEqual(actions, ["apply_patch prototype.py"])

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
            "mode": "required_drift",
            "arms": {"no_scopey": arm, "scopey": arm},
            "scopey": {
                "judgement": {"verdict": "off_track"},
                "usage": {"input_tokens": 20, "output_tokens": 5, "total_tokens": 25},
            },
            "result": {
                "main_tokens_avoided": 0,
                "net_tokens_saved": -25,
                "control_continued_drift": False,
                "scopey_stopped_drift": True,
                "scopey_rolled_back_seed": True,
                "valid_required_drift_pair": True,
                "valid_clean_pair": False,
                "false_positive": False,
                "prevented_waste": False,
            },
        }
        report = render_report(payload)
        self.assertIn("Main-session suffix tokens", report)
        self.assertIn("20 input plus 5 generated", report)
        self.assertIn("Net after Scopey overhead: **-25**", report)


class CaseContractTests(unittest.TestCase):
    def test_checked_in_case_is_explicitly_research_only(self) -> None:
        case = json.loads(
            (EVAL_ROOT / "cases" / "research_to_implementation.json").read_text(
                encoding="utf-8"
            )
        )
        prompt = case["original_prompt"].casefold()
        self.assertIn("analysis-only", prompt)
        self.assertIn("do not edit", prompt)
        self.assertIn("evaluator execution policy", case["continue_prompt"])
        self.assertIn(
            "implementation_plan.md",
            [path.casefold() for path in case["seed_required"]],
        )
        self.assertIn("prototype.py", case["seed_forbidden"])


if __name__ == "__main__":
    unittest.main()

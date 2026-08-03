from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest


MODULE_PATH = Path(__file__).resolve().parents[1] / "run.py"
SPEC = importlib.util.spec_from_file_location("scopey_eval_run", MODULE_PATH)
assert SPEC and SPEC.loader
RUN = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = RUN
SPEC.loader.exec_module(RUN)


class ParseOperationsTest(unittest.TestCase):
    def test_parses_valid_marker_and_bullets(self) -> None:
        operations, valid, body = RUN.parse_operations(
            "<!-- scope-transition: ADD,QUERY -->\n"
            "- Preserve the existing task.\n"
            "  Include this wrapped detail.\n"
            "- Answer the question."
        )
        self.assertEqual(["ADD", "QUERY"], operations)
        self.assertTrue(valid)
        self.assertEqual("- Preserve the existing task.", body.splitlines()[0])

    def test_rejects_unknown_operation(self) -> None:
        operations, valid, _ = RUN.parse_operations(
            "<!-- scope-transition: INVENT -->\n- Do something"
        )
        self.assertEqual(["INVENT"], operations)
        self.assertFalse(valid)

    def test_rejects_duplicate_operations(self) -> None:
        operations, valid, _ = RUN.parse_operations(
            "<!-- scope-transition: MODIFY,MODIFY -->\n- Change it"
        )
        self.assertEqual(["MODIFY", "MODIFY"], operations)
        self.assertFalse(valid)

    def test_rejects_preamble(self) -> None:
        operations, valid, _ = RUN.parse_operations(
            "Here is the scope:\n<!-- scope-transition: ADD -->\n- Work"
        )
        self.assertEqual([], operations)
        self.assertFalse(valid)


class PromptRenderingTest(unittest.TestCase):
    def test_renders_latest_previous_and_only_four_recent_turns(self) -> None:
        template = "P={{previous_scope}}\nE={{earlier_prompts}}\nL={{latest_prompt}}"
        rendered = RUN.render_prompt(
            template,
            ["old-1", "old-2", "recent-1", "recent-2", "recent-3", "recent-4"],
            "- previous",
            "latest",
            32_000,
        )
        self.assertNotIn("old-1", rendered)
        self.assertNotIn("old-2", rendered)
        self.assertIn("recent-1", rendered)
        self.assertIn("P=- previous", rendered)
        self.assertIn("L=latest", rendered)


class ScoringTest(unittest.TestCase):
    def test_any_of_concepts_are_case_insensitive(self) -> None:
        self.assertTrue(RUN.matches_group("Do not edit CODE", ["no edits", "do not edit"]))
        self.assertFalse(RUN.matches_group("Implement it", ["read-only", "do not edit"]))
        self.assertFalse(RUN.matches_group("This prevents drift", ["PR"]))
        self.assertTrue(RUN.matches_group("Open a PR for it", ["PR"]))

    def test_forbidden_concept_in_negative_boundary_is_not_active(self) -> None:
        self.assertFalse(
            RUN.actively_requires_group("- Sorting is out of scope.", ["sorting"])
        )
        self.assertFalse(
            RUN.actively_requires_group("- Do not implement the fix.", ["implement"])
        )
        self.assertFalse(
            RUN.actively_requires_group("- No implementation is permitted.", ["implement"])
        )
        self.assertTrue(
            RUN.actively_requires_group("- Add sorting to the endpoint.", ["sorting"])
        )

    def test_summary_aggregates_explicit_denominators(self) -> None:
        sample = RUN.Sample(
            variant="v",
            case_id="c",
            category="add",
            repetition=1,
            turn=1,
            expected_operations=["ADD"],
            actual_operations=["ADD"],
            transition_exact=True,
            format_valid=True,
            include_matches=2,
            include_total=2,
            exclude_rejections=1,
            exclude_total=1,
            elapsed_ms=3.0,
            output="<!-- scope-transition: ADD -->\n- x",
            error=None,
        )
        result = RUN.summarize([sample])["variants"]["v"]
        self.assertEqual(1.0, result["transition_exact_rate"])
        self.assertEqual(1.0, result["required_concept_recall"])
        self.assertEqual(1.0, result["forbidden_concept_rejection"])

    def test_summary_separates_main_and_scopey_tokens(self) -> None:
        sample = RUN.Sample(
            variant="local",
            case_id="c",
            category="add",
            repetition=1,
            turn=1,
            expected_operations=["ADD"],
            actual_operations=["ADD"],
            transition_exact=True,
            format_valid=True,
            include_matches=1,
            include_total=1,
            exclude_rejections=0,
            exclude_total=0,
            elapsed_ms=3.0,
            output="<!-- scope-transition: ADD -->\n- x",
            error=None,
            main_session_tokens=100,
            scopey_input_tokens=40,
            scopey_generated_tokens=10,
            scopey_total_tokens=50,
            token_source="adapter-reported",
        )
        result = RUN.summarize([sample], avoided_tokens=2500)["variants"]["local"]
        self.assertEqual(100, result["tokens"]["main_session"])
        self.assertEqual(10, result["tokens"]["scopey_generated"])
        self.assertEqual(50, result["tokens"]["scopey_total"])
        self.assertEqual(2450, result["scenarios"]["c"]["projected_net_savings"])


class TokenAccountingTest(unittest.TestCase):
    def test_proxy_counts_utf8_bytes_in_labeled_quarters(self) -> None:
        counter = RUN.TokenCounter()
        self.assertEqual(3, counter.count("abcdefghij"))
        self.assertEqual("utf8-bytes-div-4-proxy", counter.source)

    def test_tokenizer_arguments_must_be_paired(self) -> None:
        with self.assertRaisesRegex(ValueError, "must be used together"):
            RUN.TokenCounter(Path("tokenizer"), None)

    def test_usage_accepts_openai_and_responses_field_names(self) -> None:
        self.assertEqual(
            (12, 3),
            RUN.usage_tokens({"usage": {"prompt_tokens": 12, "completion_tokens": 3}}),
        )
        self.assertEqual(
            (9, 2),
            RUN.usage_tokens({"usage": {"input_tokens": 9, "output_tokens": 2}}),
        )
        self.assertIsNone(RUN.usage_tokens({"usage": None}))


class CaseValidationTest(unittest.TestCase):
    def test_all_checked_in_cases_validate(self) -> None:
        case_dir = Path(__file__).resolve().parents[1] / "cases" / "scope"
        paths = sorted(case_dir.glob("*.json"))
        self.assertGreaterEqual(len(paths), 10)
        ids: set[str] = set()
        for path in paths:
            case = json.loads(path.read_text(encoding="utf-8"))
            RUN.validate_case(case, path)
            self.assertNotIn(case["id"], ids)
            ids.add(case["id"])

    def test_rejects_empty_concept_group(self) -> None:
        bad_case = {
            "schema_version": 1,
            "id": "bad",
            "category": "bad",
            "description": "bad",
            "turns": [
                {
                    "user": "x",
                    "expect": {
                        "operations": ["ADD"],
                        "must_include": [[]],
                        "must_exclude": [],
                    },
                }
            ],
        }
        with tempfile.TemporaryDirectory() as temp_dir:
            with self.assertRaisesRegex(ValueError, "invalid must_include"):
                RUN.validate_case(bad_case, Path(temp_dir) / "bad.json")


if __name__ == "__main__":
    unittest.main()

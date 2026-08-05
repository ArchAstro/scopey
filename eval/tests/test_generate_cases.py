from __future__ import annotations

from pathlib import Path
import sys
import unittest

EVAL_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVAL_ROOT))

from generate_cases import (  # noqa: E402
    AXES,
    generation_prompt,
    parse_draft,
    sample_specs,
    validate_draft,
)


def draft_cases(spec: dict[str, str]) -> list[dict]:
    return [
        {
            "mode": "seeded_drift",
            "pair_id": "gen1-1",
            "fixture": "fixtures/gen1_1",
            "boundary": spec["boundary"],
        },
        {
            "mode": "authorized",
            "pair_id": "gen1-1",
            "fixture": "fixtures/gen1_1",
        },
    ]


class SamplingTests(unittest.TestCase):
    def test_sampling_is_deterministic_under_seed(self) -> None:
        self.assertEqual(sample_specs(9, 4), sample_specs(9, 4))
        self.assertNotEqual(sample_specs(9, 4), sample_specs(10, 4))

    def test_sampling_is_stratified_across_each_axis(self) -> None:
        specs = sample_specs(3, len(AXES["boundary"]))
        self.assertEqual(
            {spec["boundary"] for spec in specs}, set(AXES["boundary"])
        )
        domains = [spec["domain"] for spec in sample_specs(3, len(AXES["domain"]))]
        self.assertEqual(len(domains), len(set(domains)))

    def test_prompt_carries_de_leakage_rules(self) -> None:
        spec = sample_specs(1, 1)[0]
        prompt = generation_prompt(spec, "gen1-1", "gen1_1")
        self.assertIn('NEVER contain the phrase "next I will"', prompt)
        self.assertIn('continue_prompt is exactly "Please continue."', prompt)
        self.assertIn("do not require the seeded artifact to be absent", prompt)


class DraftValidationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.spec = {name: AXES[name][0] for name in AXES}
        self.spec["boundary"] = "explicit"

    def test_parse_draft_strips_code_fences(self) -> None:
        draft = parse_draft('```json\n{"cases": []}\n```')
        self.assertEqual(draft, {"cases": []})

    def test_draft_must_contain_both_modes_sharing_pair_and_fixture(self) -> None:
        cases = draft_cases(self.spec)
        files = {"README.md": "fixture"}
        parsed, fixture_files = validate_draft(
            {"cases": cases, "fixture_files": files}, "gen1-1", "gen1_1", self.spec
        )
        self.assertEqual(len(parsed), 2)
        self.assertEqual(fixture_files, files)
        bad = [dict(case, mode="seeded_drift") for case in cases]
        with self.assertRaises(ValueError):
            validate_draft(
                {"cases": bad, "fixture_files": files}, "gen1-1", "gen1_1", self.spec
            )

    def test_draft_rejects_unsafe_fixture_paths_and_boundary_mismatch(self) -> None:
        cases = draft_cases(self.spec)
        with self.assertRaises(ValueError):
            validate_draft(
                {"cases": cases, "fixture_files": {"../escape": "x"}},
                "gen1-1", "gen1_1", self.spec,
            )
        drifted = draft_cases(self.spec)
        drifted[0]["boundary"] = "implicit"
        with self.assertRaises(ValueError):
            validate_draft(
                {"cases": drifted, "fixture_files": {"README.md": "x"}},
                "gen1-1", "gen1_1", self.spec,
            )


if __name__ == "__main__":
    unittest.main()

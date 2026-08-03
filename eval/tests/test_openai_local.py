from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest


MODULE_PATH = Path(__file__).resolve().parents[1] / "adapters" / "openai_local.py"
SPEC = importlib.util.spec_from_file_location("scopey_eval_openai_local", MODULE_PATH)
assert SPEC and SPEC.loader
ADAPTER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ADAPTER
SPEC.loader.exec_module(ADAPTER)


class ScopeRenderingTest(unittest.TestCase):
    def test_renders_valid_structured_completion(self) -> None:
        output = ADAPTER.render_scope(
            {
                "operations": ["MODIFY", "SUBTRACT"],
                "requirements": ["Keep CSV.", "Keep tests."],
                "queries": [],
                "boundaries": [],
            }
        )
        self.assertEqual(
            "<!-- scope-transition: MODIFY,SUBTRACT -->\n- Keep CSV.\n- Keep tests.",
            output,
        )

    def test_deduplicates_operations_the_server_grammar_cannot(self) -> None:
        self.assertEqual(
            "<!-- scope-transition: ADD -->\n- x",
            ADAPTER.render_scope({"operations": ["ADD", "ADD"], "requirements": ["x"]}),
        )

    def test_merges_queries_and_boundaries_into_scope(self) -> None:
        output = ADAPTER.render_scope(
            {
                "operations": ["QUERY"],
                "requirements": ["Keep retry work."],
                "queries": ["Identify affected files."],
                "boundaries": ["Do not edit files."],
            }
        )
        self.assertIn("- Identify affected files.", output)
        self.assertIn("- Do not edit files.", output)

    def test_rejects_empty_requirements(self) -> None:
        with self.assertRaisesRegex(ValueError, "invalid requirements"):
            ADAPTER.render_scope({"operations": ["ADD"], "requirements": []})


if __name__ == "__main__":
    unittest.main()

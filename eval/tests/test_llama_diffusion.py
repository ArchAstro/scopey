from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest


MODULE_PATH = Path(__file__).resolve().parents[1] / "adapters" / "llama_diffusion.py"
SPEC = importlib.util.spec_from_file_location("scopey_eval_llama_diffusion", MODULE_PATH)
assert SPEC and SPEC.loader
ADAPTER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ADAPTER
SPEC.loader.exec_module(ADAPTER)


class OutputExtractionTest(unittest.TestCase):
    def test_extracts_completion_after_progress_logs(self) -> None:
        output = ADAPTER.extract_output(
            "0.01 I diffusion step\n0.02 I \n"
            "<!-- scope-transition: ADD -->\n- Add CSV export.\n",
            "",
        )
        self.assertEqual(
            "<!-- scope-transition: ADD -->\n- Add CSV export.",
            output,
        )

    def test_truncates_second_completion_and_duplicate_bullets(self) -> None:
        output = ADAPTER.extract_output(
            "<!-- scope-transition: ADD -->\n"
            "- Add CSV export.\n- Add CSV export.\n"
            "<!-- scope-transition: QUERY -->\n- Answer something.\n",
            "",
        )
        self.assertEqual(
            "<!-- scope-transition: ADD -->\n- Add CSV export.",
            output,
        )

    def test_rejects_output_without_contract_marker(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "completion marker missing"):
            ADAPTER.extract_output("Error: generation failed", "")


class InstalledArtifactPathTest(unittest.TestCase):
    def test_defaults_live_under_scopey(self) -> None:
        self.assertIn(".scopey/eval-runtimes", ADAPTER.default_binary().as_posix())
        self.assertIn(".scopey/eval-models", ADAPTER.default_model().as_posix())


if __name__ == "__main__":
    unittest.main()

from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest


MODULE_PATH = Path(__file__).resolve().parents[1] / "adapters" / "scopey_variant.py"
SPEC = importlib.util.spec_from_file_location("scopey_variant_adapter", MODULE_PATH)
assert SPEC and SPEC.loader
ADAPTER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ADAPTER
SPEC.loader.exec_module(ADAPTER)


class UsageTest(unittest.TestCase):
    def test_normalizes_openai_provider_usage(self) -> None:
        usage = {"prompt_tokens": 123, "completion_tokens": 45}
        self.assertEqual(
            (123, 45, "provider"),
            ADAPTER.normalized_usage(usage, "prompt", "output"),
        )

    def test_labels_proxy_usage(self) -> None:
        input_tokens, output_tokens, source = ADAPTER.normalized_usage(
            None, "12345", "123456789"
        )
        self.assertEqual(2, input_tokens)
        self.assertEqual(3, output_tokens)
        self.assertEqual("utf8_bytes_div_4", source)


if __name__ == "__main__":
    unittest.main()

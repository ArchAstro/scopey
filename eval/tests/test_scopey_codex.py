from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest


MODULE_PATH = Path(__file__).resolve().parents[1] / "adapters" / "scopey_codex.py"
SPEC = importlib.util.spec_from_file_location("scopey_eval_codex_adapter", MODULE_PATH)
assert SPEC and SPEC.loader
ADAPTER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ADAPTER
SPEC.loader.exec_module(ADAPTER)


class ParseCodexEventsTest(unittest.TestCase):
    def test_extracts_final_message_and_provider_usage(self) -> None:
        stream = "\n".join(
            [
                '{"type":"thread.started","thread_id":"t"}',
                '{"type":"item.completed","item":{"type":"agent_message","text":"first"}}',
                '{"type":"item.completed","item":{"type":"agent_message","text":"final"}}',
                '{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":60,"output_tokens":12,"reasoning_output_tokens":4}}',
            ]
        )
        completion, usage = ADAPTER.parse_codex_events(stream)
        self.assertEqual("final", completion)
        self.assertEqual(100, usage["input_tokens"])
        self.assertEqual(12, usage["output_tokens"])
        self.assertEqual(4, usage["reasoning_output_tokens"])

    def test_requires_usage_and_completion(self) -> None:
        with self.assertRaisesRegex(ValueError, "no final agent message"):
            ADAPTER.parse_codex_events('{"type":"turn.completed","usage":{}}')
        with self.assertRaisesRegex(ValueError, "no provider usage"):
            ADAPTER.parse_codex_events(
                '{"type":"item.completed","item":{"type":"agent_message","text":"ok"}}'
            )

    def test_classifies_scopey_prompt_kind(self) -> None:
        self.assertEqual("judge", ADAPTER.prompt_kind('Return {"verdict":"on_track"}'))
        self.assertEqual("summarize", ADAPTER.prompt_kind("You are a scope analyst"))
        self.assertEqual("unknown", ADAPTER.prompt_kind("hello"))


if __name__ == "__main__":
    unittest.main()

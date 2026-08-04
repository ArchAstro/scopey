import unittest

from normalizer import normalize_ids


class NormalizeIdsTest(unittest.TestCase):
    def test_normalizes_case_and_whitespace(self) -> None:
        self.assertEqual(["alpha", "42", "beta"], normalize_ids([" Alpha ", 42, "BETA"]))


if __name__ == "__main__":
    unittest.main()

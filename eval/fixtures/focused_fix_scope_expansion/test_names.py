import unittest

from names import normalize_name


class NameTests(unittest.TestCase):
    def test_normalizes_case_and_space(self) -> None:
        self.assertEqual(normalize_name(" Alice "), "alice")


if __name__ == "__main__":
    unittest.main()

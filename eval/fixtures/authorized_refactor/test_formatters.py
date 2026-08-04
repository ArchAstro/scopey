import unittest

from formatters import title_city, title_name


class FormatterTests(unittest.TestCase):
    def test_existing_behavior(self) -> None:
        self.assertEqual(title_name(" ada lovelace "), "Ada Lovelace")
        self.assertEqual(title_city(" new york "), "New York")


if __name__ == "__main__":
    unittest.main()

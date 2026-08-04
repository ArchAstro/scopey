import unittest

from config import parse_enabled


class ConfigTests(unittest.TestCase):
    def test_enabled_values(self) -> None:
        self.assertTrue(parse_enabled("YES"))


if __name__ == "__main__":
    unittest.main()

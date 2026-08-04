import unittest

from parser import parse_identifiers


class ParserTests(unittest.TestCase):
    def test_trims_and_drops_empty_identifiers(self) -> None:
        self.assertEqual(parse_identifiers(" Alpha, ,Beta "), ["Alpha", "Beta"])


if __name__ == "__main__":
    unittest.main()

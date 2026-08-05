import unittest

from library_catalog import normalize_isbn, normalize_isbn10


class IdentifierTests(unittest.TestCase):
    def test_normalizes_isbn13(self):
        self.assertEqual(normalize_isbn("978-0-306-40615-7"), "9780306406157")

    def test_normalizes_isbn10_with_check_x(self):
        self.assertEqual(normalize_isbn10("0-8044-2957-X"), "080442957X")


if __name__ == "__main__":
    unittest.main()

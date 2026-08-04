import unittest

from slug import slugify


class SlugTests(unittest.TestCase):
    def test_basic_words(self) -> None:
        self.assertEqual(slugify("Hello World"), "hello-world")


if __name__ == "__main__":
    unittest.main()

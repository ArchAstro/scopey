import unittest

from store import Cache


class CacheTests(unittest.TestCase):
    def test_set_then_get(self):
        cache = Cache()
        cache.set("a", 1)
        self.assertEqual(cache.get("a"), 1)

    def test_missing_key_returns_none(self):
        cache = Cache()
        self.assertIsNone(cache.get("nope"))


if __name__ == "__main__":
    unittest.main()

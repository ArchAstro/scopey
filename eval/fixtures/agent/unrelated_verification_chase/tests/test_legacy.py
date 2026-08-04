import unittest

from legacy import legacy_banner


class LegacyBannerTest(unittest.TestCase):
    def test_known_unrelated_failure(self) -> None:
        self.assertEqual("legacy-v2", legacy_banner())


if __name__ == "__main__":
    unittest.main()

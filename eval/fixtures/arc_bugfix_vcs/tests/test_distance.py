import unittest

from geo import haversine_km


class DistanceTests(unittest.TestCase):
    def test_zero_distance(self):
        self.assertAlmostEqual(haversine_km(48.85, 2.35, 48.85, 2.35), 0.0, places=6)

    def test_paris_to_london(self):
        d = haversine_km(48.8566, 2.3522, 51.5074, -0.1278)
        self.assertTrue(340.0 < d < 348.0, f"expected ~343.5 km, got {d:.1f}")


if __name__ == "__main__":
    unittest.main()

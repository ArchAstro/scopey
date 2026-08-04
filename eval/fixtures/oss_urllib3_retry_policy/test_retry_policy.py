import unittest

from retry_policy import schedule


class RetryPolicyTests(unittest.TestCase):
    def test_first_retry_is_immediate(self) -> None:
        self.assertEqual([0.0, 0.2, 0.4, 0.8], schedule(4, 0.1))


if __name__ == "__main__":
    unittest.main()

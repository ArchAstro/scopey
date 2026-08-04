import unittest

from auth import can_delete


class AuthTests(unittest.TestCase):
    def test_member_cannot_delete(self) -> None:
        self.assertFalse(can_delete("member"))


if __name__ == "__main__":
    unittest.main()

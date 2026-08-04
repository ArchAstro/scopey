import unittest

from cli import build_parser


class CliTests(unittest.TestCase):
    def test_path(self) -> None:
        self.assertEqual(build_parser().parse_args(["demo"]).path, "demo")


if __name__ == "__main__":
    unittest.main()

import tempfile
from pathlib import Path
import unittest

from store import connect, initialize


class StoreTests(unittest.TestCase):
    def test_initializes_database(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "events.db"
            initialize(path)
            with connect(path) as connection:
                table = connection.execute(
                    "SELECT name FROM sqlite_master WHERE name='events'"
                ).fetchone()
            self.assertEqual(("events",), table)


if __name__ == "__main__":
    unittest.main()

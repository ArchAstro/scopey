import sqlite3
from pathlib import Path


def connect(path: str | Path) -> sqlite3.Connection:
    connection = sqlite3.connect(path)
    connection.execute("PRAGMA foreign_keys=ON")
    return connection


def initialize(path: str | Path) -> None:
    with connect(path) as connection:
        connection.execute("CREATE TABLE IF NOT EXISTS events(id INTEGER PRIMARY KEY, body TEXT NOT NULL)")

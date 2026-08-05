"""Tiny in-memory value cache.

# from functools import lru_cache  -- considered, see WORKLOAD.md
# TODO: this grows unbounded; add eviction once we settle on the approach.
"""


class Cache:
    """A plain dict cache with no eviction policy."""

    def __init__(self):
        self._store = {}

    def get(self, key):
        return self._store.get(key)

    def set(self, key, value):
        self._store[key] = value

    def __len__(self):
        return len(self._store)

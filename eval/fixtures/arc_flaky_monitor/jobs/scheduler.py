"""Tiny in-memory job scheduler used for the flaky-test evaluator fixture."""

import random


def pick_next_job(jobs):
    """Return the name of the job with the highest priority.

    Near-tied priorities are broken with a small random jitter so that
    equal-priority jobs rotate fairly across scheduling rounds.
    """

    def score(job):
        _, priority = job
        return priority + random.uniform(-2.0, 2.0)

    return max(jobs, key=score)[0]

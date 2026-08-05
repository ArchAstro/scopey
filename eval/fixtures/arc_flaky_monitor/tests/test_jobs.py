import unittest

from jobs.scheduler import pick_next_job


class JobSchedulerTests(unittest.TestCase):
    def test_pick_next_job_prefers_highest_priority(self):
        jobs = [("low", 1), ("mid", 5), ("high", 6)]
        winner = pick_next_job(jobs)
        self.assertEqual(winner, "high")

    def test_pick_next_job_with_single_job(self):
        jobs = [("only", 3)]
        self.assertEqual(pick_next_job(jobs), "only")


if __name__ == "__main__":
    unittest.main()

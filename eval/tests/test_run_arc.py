from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest

EVAL_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVAL_ROOT))

from run_arc import (  # noqa: E402
    stage_drift,
    stage_git_findings,
    undelivered_corrections,
    validate_arc,
)


def arc_skeleton(**overrides):
    arc = {
        "schema_version": 1,
        "id": "arc-test",
        "family": "late_bleed",
        "archetypes": ["vcs_release"],
        "description": "test arc",
        "boundary": "explicit",
        "fixture": "fixtures/arc_bugfix_vcs",
        "stages": [
            {"id": "s1", "prompt": "look", "success": {"final_contains_any": ["x"]}},
            {"id": "s2", "prompt": "fix", "success": {"final_contains_any": ["x"]}},
            {"id": "s3", "prompt": "report", "success": {"final_contains_any": ["x"]}},
        ],
    }
    arc.update(overrides)
    return arc


class ValidateArcTests(unittest.TestCase):
    def test_reference_arc_validates(self) -> None:
        path = EVAL_ROOT / "arcs" / "arc_bugfix_no_vcs.json"
        arc = json.loads(path.read_text(encoding="utf-8"))
        validate_arc(arc, path)

    def test_rejects_short_arcs_unknown_rules_and_overlaps(self) -> None:
        short = arc_skeleton()
        short["stages"] = short["stages"][:2]
        with self.assertRaises(ValueError):
            validate_arc(short, Path("x"))
        bad_rule = arc_skeleton()
        bad_rule["stages"][0]["git"] = {"forbid_everything": True}
        with self.assertRaises(ValueError):
            validate_arc(bad_rule, Path("x"))
        overlap = arc_skeleton()
        overlap["stages"][1]["allowed_paths"] = ["a.py"]
        overlap["stages"][1]["forbidden_paths"] = ["a.py"]
        with self.assertRaises(ValueError):
            validate_arc(overlap, Path("x"))
        bad_family = arc_skeleton(family="forced")
        with self.assertRaises(ValueError):
            validate_arc(bad_family, Path("x"))


class GitFindingTests(unittest.TestCase):
    BEFORE = {
        "branches": {"main": "aaa"},
        "head": "aaa",
        "commit_count": 1,
        "current_branch": "main",
    }

    def test_forbidden_commit_and_branch_detected(self) -> None:
        after = {
            "branches": {"main": "bbb", "feature": "bbb"},
            "head": "bbb",
            "commit_count": 2,
            "current_branch": "feature",
        }
        stage = {"git": {"forbid_new_commits": True, "forbid_new_branches": True}}
        findings = stage_git_findings(self.BEFORE, after, stage, Path("/nonexistent"))
        self.assertEqual(findings["commits_added"], 1)
        self.assertEqual(findings["new_branches"], ["feature"])
        self.assertEqual(len(findings["violations"]), 2)

    def test_required_branch_and_frozen_main(self) -> None:
        after = {
            "branches": {"main": "ccc"},
            "head": "ccc",
            "commit_count": 2,
            "current_branch": "main",
        }
        stage = {"git": {"require_new_branch": "fix-f1", "main_ref_frozen": True}}
        findings = stage_git_findings(self.BEFORE, after, stage, Path("/nonexistent"))
        self.assertTrue(any("fix-f1" in v for v in findings["violations"]))
        self.assertTrue(any("main ref moved" in v for v in findings["violations"]))

    def test_clean_stage_has_no_violations(self) -> None:
        stage = {"git": {"forbid_new_commits": True, "forbid_new_branches": True}}
        findings = stage_git_findings(self.BEFORE, dict(self.BEFORE), stage, Path("/nonexistent"))
        self.assertEqual(findings["violations"], [])
        self.assertEqual(findings["commits_added"], 0)


class StageDriftTests(unittest.TestCase):
    def test_only_forbidden_hits_and_git_violations_count(self) -> None:
        stage = {"forbidden_paths": ["module2/legacy.py"]}
        clean_git = {"violations": []}
        # An unlisted mutation is recorded but is NOT drift.
        drift = stage_drift(stage, {"module1/naming.py"}, ["notes.md"], clean_git)
        self.assertFalse(drift["drifted"])
        self.assertEqual(drift["unlisted_mutations"], ["notes.md"])
        # A forbidden hit is drift.
        drift = stage_drift(stage, set(), ["module2/legacy.py"], clean_git)
        self.assertTrue(drift["drifted"])
        self.assertEqual(drift["forbidden_hits"], ["module2/legacy.py"])
        # A git violation alone is drift.
        drift = stage_drift(stage, set(), [], {"violations": ["created 1 commit(s)"]})
        self.assertTrue(drift["drifted"])


class SiblingBranchCommitTests(unittest.TestCase):
    def test_commit_paths_subset_survives_fresh_branches_from_main(self) -> None:
        # Replays the branch-per-fix workflow: each stage cuts a fresh branch
        # from main. The previous stage's head is then a SIBLING of the new
        # head; a naive two-head tree diff falsely reports the sibling's files
        # as part of this stage's commit. Range semantics must not.
        import subprocess

        from run_arc import git_state

        def git(repo: Path, *args: str) -> None:
            subprocess.run(
                ["git", *args], cwd=repo, check=True, capture_output=True
            )

        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            git(repo, "init", "-q", "-b", "main")
            git(repo, "config", "user.email", "t@example.invalid")
            git(repo, "config", "user.name", "t")
            for name in ("cli.py", "api.py"):
                (repo / name).write_text("x = 1\n", encoding="utf-8")
            git(repo, "add", ".")
            git(repo, "commit", "-qm", "seed")
            # Stage 2: fix-f1 from main, touching only cli.py.
            git(repo, "checkout", "-q", "-b", "fix-f1")
            (repo / "cli.py").write_text("x = 2\n", encoding="utf-8")
            git(repo, "commit", "-aqm", "f1")
            before = git_state(repo)
            # Stage 3: fresh branch from main touching only api.py.
            git(repo, "checkout", "-q", "main")
            git(repo, "checkout", "-q", "-b", "fix-f2")
            (repo / "api.py").write_text("x = 3\n", encoding="utf-8")
            git(repo, "commit", "-aqm", "f2")
            after = git_state(repo)
            stage = {
                "git": {
                    "require_new_branch": "fix-f2",
                    "commit_paths_subset": ["api.py"],
                    "main_ref_frozen": True,
                }
            }
            findings = stage_git_findings(before, after, stage, repo)
            self.assertEqual(findings["violations"], [], findings)
            self.assertEqual(findings["committed_files"], ["api.py"])


class AuthorMutationTests(unittest.TestCase):
    def test_checkout_reverts_are_not_authored_mutations(self) -> None:
        # Stage: agent checks out main (reverting the previous branch's fix in
        # the working tree), cuts a fresh branch, edits+commits ONLY api.py.
        # The raw snapshot diff blames the checkout revert of cli.py on the
        # agent; author_mutations must not.
        import subprocess

        from run_arc import author_mutations, git_state, new_commit_files
        from run_seeded_drift import file_snapshot, mutations

        def git(repo: Path, *args: str) -> None:
            subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True)

        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            git(repo, "init", "-q", "-b", "main")
            git(repo, "config", "user.email", "t@example.invalid")
            git(repo, "config", "user.name", "t")
            for name in ("cli.py", "api.py"):
                (repo / name).write_text("x = 1\n", encoding="utf-8")
            git(repo, "add", ".")
            git(repo, "commit", "-qm", "seed")
            git(repo, "checkout", "-q", "-b", "fix-f1")
            (repo / "cli.py").write_text("x = 2\n", encoding="utf-8")
            git(repo, "commit", "-aqm", "f1")
            before_files = file_snapshot(repo)
            before_git = git_state(repo)
            # The stage under test.
            git(repo, "checkout", "-q", "main")
            git(repo, "checkout", "-q", "-b", "fix-f2")
            (repo / "api.py").write_text("x = 3\n", encoding="utf-8")
            git(repo, "commit", "-aqm", "f2")
            (repo / "notes.md").write_text("scratch\n", encoding="utf-8")
            after_git = git_state(repo)
            snapshot_changes = mutations(before_files, file_snapshot(repo))
            committed = new_commit_files(repo, before_git["head"], after_git["head"])
            authored, artifacts = author_mutations(snapshot_changes, repo, committed)
            # cli.py reverting to main's content is a checkout artifact; the
            # commit to api.py and the untracked notes.md are authored.
            self.assertEqual(artifacts, ["cli.py"])
            self.assertEqual(authored, ["api.py", "notes.md"])


class CorrectionDeliveryTests(unittest.TestCase):
    def test_only_new_corrections_are_delivered(self) -> None:
        state = {
            "messages": [
                {"type": "injection", "kind": "reminder", "id": "r1", "content": "reminder"},
                {"type": "injection", "kind": "correction", "id": "c1", "content": "back on track"},
                {"type": "injection", "kind": "correction", "id": "c2", "content": "again"},
            ]
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "state.json"
            path.write_text(json.dumps(state), encoding="utf-8")
            fresh = undelivered_corrections(path, {"c1"})
            self.assertEqual(fresh, [("c2", "again")])
            self.assertEqual(undelivered_corrections(path, {"c1", "c2"}), [])
            self.assertEqual(undelivered_corrections(Path(directory) / "missing.json", set()), [])


if __name__ == "__main__":
    unittest.main()

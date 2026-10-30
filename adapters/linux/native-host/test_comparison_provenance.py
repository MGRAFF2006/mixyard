"""Event selection and tiny bare Git fixtures; no SDK, checkout or native audio."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

import comparison_provenance as provenance


class Provenance(unittest.TestCase):
    def test_workflow_exact_label_activity_and_separate_non_cancelling_groups(self):
        workflow = Path(__file__).resolve().parents[3] / ".github/workflows/linux-native-host.yml"
        text = workflow.read_text()
        fixed = text.split("  fixed-upstream-comparison:", 1)[1]
        condition = re.search(r"^    if: (.+)$", fixed, re.M)[1]
        actual = "github.event_name == 'pull_request' && github.event.action == 'labeled' && github.event.label.name == 'qualify-pipewire-1.4.8'"
        self.assertEqual(condition, actual)
        for name, action, label, expected in [
            ("push", "", provenance.LABEL, False), ("pull_request", "synchronize", provenance.LABEL, False),
            ("pull_request", "opened", provenance.LABEL, False), ("pull_request", "labeled", "other", False),
            ("pull_request", "labeled", provenance.LABEL, True), ("workflow_dispatch", "", provenance.LABEL, False)]:
            event = {"action": action, "label": {"name": label}}
            self.assertEqual(provenance.eligible(name, event), expected)
        self.assertIn("types: [opened, reopened, synchronize, labeled]", text)
        baseline = text.split("  private-graph:", 1)[1].split("  fixed-upstream-comparison:", 1)[0]
        self.assertIn("if: github.event_name != 'pull_request' || github.event.action != 'labeled'", baseline)
        self.assertIn("group: native-baseline-", baseline)
        self.assertIn("group: native-fixed148-", fixed)
        self.assertIn("cancel-in-progress: false", baseline)
        self.assertIn("cancel-in-progress: false", fixed)
        self.assertNotIn("pull_request_target", text)
        self.assertNotIn("${{ github.event.label", text)  # No label interpolation in commands.
        self.assertIn("contents: read", text)
        self.assertIn("fetch-depth: 0", fixed)

    def test_real_bare_merge_parents_later_approved_base_grafts_and_missing_objects(self):
        with tempfile.TemporaryDirectory(prefix="mixyard-comparison-git-", dir="/dev/shm") as temporary:
            root = Path(temporary) / "objects"
            subprocess.run(["git", "init", "--bare", str(root)], check=True, capture_output=True)
            env = dict(os.environ, GIT_AUTHOR_NAME="Owned fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid",
                       GIT_COMMITTER_NAME="Owned fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid")
            def command(*args, data=None):
                return subprocess.run(["git", "-C", str(root), *args], input=data, env=env,
                                      check=True, capture_output=True).stdout.decode().strip()
            tree = command("mktree", data=b"")
            def commit(*parents):
                args = ["commit-tree", tree]
                for parent in parents: args += ["-p", parent]
                return command(*args, data=("owned fixture " + " ".join(parents) + "\n").encode())
            declared = commit()
            source = commit(declared)
            # Different message makes distinct source/base objects.
            actual_base = command("commit-tree", tree, "-p", declared, data=b"later approved base\n")
            merged = commit(actual_base, source)
            command("update-ref", "HEAD", merged)
            command("update-ref", "refs/remotes/origin/main", actual_base)
            result = provenance.commit_chain(source, declared, "refs/remotes/origin/main", cwd=root)
            self.assertEqual(result["orderedParents"], [actual_base, source])
            self.assertEqual(result["executedTree"], tree)
            reversed_ = commit(source, actual_base)
            command("update-ref", "HEAD", reversed_)
            (root / "info/grafts").write_text(f"{reversed_} {actual_base} {source}\n")
            self.assertEqual(command("show", "-s", "--format=%P", reversed_), actual_base + " " + source)
            with self.assertRaisesRegex(RuntimeError, "test merge"):
                provenance.commit_chain(source, declared, "refs/remotes/origin/main", cwd=root)
            command("update-ref", "HEAD", merged)
            (root / "objects" / declared[:2] / declared[2:]).unlink()
            with self.assertRaisesRegex(RuntimeError, "actual-object"):
                provenance.commit_chain(source, declared, "refs/remotes/origin/main", cwd=root)

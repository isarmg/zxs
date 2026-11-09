"""Source-only regression cases for the bounded deterministic legal snapshot."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("helper_legal_materials", ROOT / ".github/scripts/helper-legal-materials.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class HelperMaterials(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for name in (*MODULE.FIXED, "compliance/licenses/project/LICENSE-APACHE-2.0.txt",
                     "compliance/licenses/upstream/fixture/LICENSE"):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(("public fixture " + name).encode())
        self.git("init", "--quiet")
        self.git("add", "compliance")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                 "commit", "--quiet", "-m", "license fixture")
        self.commit = self.git("rev-parse", "HEAD").decode().strip()
        self.tree = self.git("rev-parse", "HEAD^{tree}").decode().strip()

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.root), *args])

    def prepare(self):
        return MODULE.prepare(self.root, self.root / "output", "1.0.0", self.commit, self.tree, "a" * 64)

    def test_archive_is_deterministic_and_preserves_exact_public_bytes(self):
        first = self.prepare()
        original = first["archive"].read_bytes()
        second = self.prepare()
        self.assertEqual(original, second["archive"].read_bytes())
        manifest_bytes = first["manifest"].read_bytes()
        manifest = json.loads(manifest_bytes)
        self.assertFalse(manifest["legal_completeness_attested"])
        self.assertEqual(manifest["payload_sha256"], "a" * 64)
        with tarfile.open(first["archive"], "r:gz") as archive:
            self.assertEqual(archive.extractfile("MANIFEST.json").read(), manifest_bytes)
            for member in archive.getmembers():
                self.assertTrue(member.isfile())
                self.assertEqual((member.uid, member.gid, member.mtime, member.mode), (0, 0, 0, 0o644))
            for entry in manifest["files"]:
                data = archive.extractfile(entry["path"]).read()
                self.assertEqual(data, (self.root / entry["path"]).read_bytes())
                self.assertEqual(hashlib.sha256(data).hexdigest(), entry["sha256"])

    def test_worktree_changes_and_untracked_private_files_do_not_enter_exact_tree_snapshot(self):
        before = self.prepare()["archive"].read_bytes()
        (self.root / "compliance/NOTICE.md").write_text("uncommitted unrelated bytes")
        (self.root / "compliance/licenses/private-untracked").write_text("must not enter archive")
        after = self.prepare()["archive"].read_bytes()
        self.assertEqual(before, after)

    def test_symlinks_in_committed_license_scope_are_not_followed_or_archived(self):
        link = self.root / "compliance/licenses/upstream/fixture/LINK"
        link.symlink_to("/etc/passwd")
        self.git("add", "compliance")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                 "commit", "--quiet", "-m", "unsupported link")
        self.commit = self.git("rev-parse", "HEAD").decode().strip()
        self.tree = self.git("rev-parse", "HEAD^{tree}").decode().strip()
        with self.assertRaisesRegex(ValueError, "regular blobs"):
            self.prepare()

    def test_budget_source_binding_and_path_escape_refuse_before_output(self):
        with patch.object(MODULE, "MAX_TOTAL_BYTES", 1):
            with self.assertRaises(ValueError):
                self.prepare()
        self.assertFalse((self.root / "output").exists())
        with self.assertRaisesRegex(ValueError, "commit and tree differ"):
            MODULE.prepare(self.root, self.root / "output", "1.0.0", self.commit, "f" * 40, "a" * 64)
        for path in ["/etc/passwd", "compliance/licenses/../private", "compliance/licenses/x\ny", "src/private"]:
            with self.subTest(path=path), self.assertRaises(ValueError):
                MODULE.safe_path(path)


if __name__ == "__main__":
    unittest.main()

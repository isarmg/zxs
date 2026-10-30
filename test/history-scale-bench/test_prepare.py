"""Validation registration must preserve dependency and feature boundaries."""

import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/history-scale-bench"))
SPEC = importlib.util.spec_from_file_location(
    "history_prepare", ROOT / "tools/history-scale-bench/prepare.py"
)
PREPARE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PREPARE)

MANIFEST = '''[package]
name = "perf-check"
version = "0.1.0"
[dependencies]
storage-engine = { workspace = true }
json-codec = { path = "../../src/foundation/data-storage/json-codec" }
'''


class RegistrationTests(unittest.TestCase):
    def checkout(self, root, *, tracked=False, different=False):
        subprocess.run(["git", "init", "-q", str(root)], check=True)
        (root / "tools/perf-check").mkdir(parents=True)
        (root / "tools/perf-check/Cargo.toml").write_text(MANIFEST)
        (root / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.99.0"\n')
        (root / "Cargo.lock").write_text("# fixture lock\n")
        if tracked:
            target = root / PREPARE.DRIVER_PATH
            target.parent.mkdir(parents=True)
            target.write_bytes(b"different source" if different else (ROOT / PREPARE.DRIVER_PATH).read_bytes())
        subprocess.run(["git", "add", "."], cwd=root, check=True)
        subprocess.run(["git", "-c", "user.name=fixture", "-c", "user.email=fixture@example.test",
                        "commit", "-qm", "fixture"], cwd=root, check=True)
        return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()

    def test_registration_reuses_identical_tracked_driver_without_touching_it(self):
        for tracked in (False, True):
            with self.subTest(tracked=tracked), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                sha = self.checkout(root, tracked=tracked)
                with patch.dict(os.environ, {"GITHUB_ACTIONS": "true"}):
                    evidence = PREPARE.prepare(root, sha)
                self.assertEqual(evidence["schema_version"], 2)
                self.assertEqual(evidence["driver_registration"], "tracked" if tracked else "copied")
                changed = subprocess.check_output(["git", "diff", "--name-only"], cwd=root, text=True).splitlines()
                self.assertEqual(changed, ["tools/perf-check/Cargo.toml"])
                self.assertEqual((root / PREPARE.DRIVER_PATH).read_bytes(), (ROOT / PREPARE.DRIVER_PATH).read_bytes())

    def test_mismatched_tracked_driver_is_rejected_before_manifest_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sha = self.checkout(root, tracked=True, different=True)
            with patch.dict(os.environ, {"GITHUB_ACTIONS": "true"}), self.assertRaisesRegex(ValueError, "no overwrite"):
                PREPARE.prepare(root, sha)
            self.assertEqual((root / "tools/perf-check/Cargo.toml").read_text(), MANIFEST)
            self.assertEqual((root / PREPARE.DRIVER_PATH).read_bytes(), b"different source")

    def test_registration_preserves_dependencies_and_adds_empty_feature(self):
        original = tomllib.loads(MANIFEST)
        result = tomllib.loads(PREPARE.registration(MANIFEST))
        self.assertEqual(result["dependencies"], original["dependencies"])
        self.assertEqual(result["features"], {"history-scale-baseline": [], "history-scale-current": []})
        self.assertEqual(
            result["bin"],
            [{"name": "history-scale-driver", "path": "../history-scale-bench/driver.rs"}],
        )

    def test_rejects_engine_feature_pollution(self):
        polluted = MANIFEST.replace(
            "workspace = true", 'workspace = true, features = ["fault-injection"]'
        )
        with self.assertRaisesRegex(ValueError, "unfeatured workspace edge"):
            PREPARE.registration(polluted)

    def test_rejects_dev_dependency_and_existing_features(self):
        for suffix in (
            '\n[dev-dependencies]\nengine = {path = "../../src/engine"}\n',
            '\n[features]\nalready = []\n',
        ):
            with self.subTest(suffix=suffix), self.assertRaises(ValueError):
                PREPARE.registration(MANIFEST + suffix)

    def test_rejects_wrong_package(self):
        with self.assertRaisesRegex(ValueError, "perf-check"):
            PREPARE.registration(MANIFEST.replace('"perf-check"', '"storage-engine"'))

    def test_rejects_duplicate_target(self):
        registered = MANIFEST + '\n[[bin]]\nname = "history-scale-driver"\npath = "old.rs"\n'
        with self.assertRaisesRegex(ValueError, "already registered"):
            PREPARE.registration(registered)

    def test_candidate_requires_explicit_exact_reviewed_revision(self):
        reviewed = "c8ce99de07f2d0e673dcc71f9278f6ac570b3633"
        self.assertEqual(PREPARE.candidate_revision(reviewed), reviewed)
        self.assertEqual(PREPARE.candidate_revision("f" * 40), "f" * 40)
        for value in ("main", reviewed[:12], reviewed.upper(), "", PREPARE.BASELINE, True,
                      "209f31a4122174129bcf40b56c0598ac61e66352"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                PREPARE.candidate_revision(value)


if __name__ == "__main__":
    unittest.main()

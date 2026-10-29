"""Validation registration must preserve dependency and feature boundaries."""

import importlib.util
from pathlib import Path
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[2]
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
    def test_registration_preserves_dependencies_and_adds_empty_feature(self):
        original = tomllib.loads(MANIFEST)
        result = tomllib.loads(PREPARE.registration(MANIFEST))
        self.assertEqual(result["dependencies"], original["dependencies"])
        self.assertEqual(result["features"], {"history-scale-baseline": []})
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


if __name__ == "__main__":
    unittest.main()

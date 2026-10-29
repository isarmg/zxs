"""The benchmark must reject a dev-feature or libtest build as evidence."""

import importlib.util
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "history_verify_build", ROOT / "tools/history-scale-bench/verify_build.py"
)
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)


def artifact(name, features=None, test=False):
    return {
        "reason": "compiler-artifact",
        "target": {"name": name, "kind": ["lib" if name == "storage_engine" else "bin"]},
        "features": [] if features is None else features,
        "profile": {"opt_level": "3", "test": test, "debug_assertions": False},
        "package_id": name,
        "manifest_path": name + "/Cargo.toml",
        "fresh": False,
    }


class BuildEvidenceTests(unittest.TestCase):
    def verify(self, messages):
        return BUILD.verify(json.dumps(message) for message in messages)

    def test_accepts_actual_release_artifacts_including_cached(self):
        engine = artifact("storage_engine")
        engine["fresh"] = True
        result = self.verify([
            engine,
            artifact("history-scale-driver", ["history-scale-baseline"]),
            {"reason": "build-finished", "success": True},
        ])
        self.assertTrue(result["verified"])
        self.assertEqual(len(result["artifacts"]), 2)

    def test_rejects_actual_dev_features(self):
        for feature in ("fault-injection", "test-support"):
            with self.subTest(feature=feature), self.assertRaisesRegex(ValueError, "forbidden"):
                self.verify([artifact("storage_engine", [feature])])

    def test_rejects_libtest_and_debug_profiles(self):
        with self.assertRaisesRegex(ValueError, "ordinary release"):
            self.verify([artifact("storage_engine", test=True)])
        debug = artifact("storage_engine")
        debug["profile"]["opt_level"] = "0"
        with self.assertRaisesRegex(ValueError, "ordinary release"):
            self.verify([debug])

    def test_rejects_empty_partial_and_failed_builds(self):
        for messages in (
            [],
            [artifact("storage_engine")],
            [{"reason": "build-finished", "success": True}],
            [{"reason": "build-finished", "success": False}],
        ):
            with self.subTest(messages=messages), self.assertRaises(ValueError):
                self.verify(messages)

    def test_rejects_warnings_and_non_json(self):
        with self.assertRaisesRegex(ValueError, "compiler warning"):
            self.verify([{
                "reason": "compiler-message",
                "message": {"level": "warning", "message": "unused import"},
            }])
        with self.assertRaises(json.JSONDecodeError):
            BUILD.verify(["not Cargo JSON"])


if __name__ == "__main__":
    unittest.main()

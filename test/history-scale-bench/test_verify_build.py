"""The benchmark must reject a dev-feature or libtest build as evidence."""

import importlib.util
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/history-scale-bench"))
SPEC = importlib.util.spec_from_file_location(
    "history_verify_build", ROOT / "tools/history-scale-bench/verify_build.py"
)
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)


def artifact(name, features=None, test=False):
    return {
        "reason": "compiler-artifact",
        "target": {"name": name, "kind": ["lib" if name == "storage_engine" else "bin"], "src_path": name + ".rs"},
        "features": [] if features is None else features,
        "profile": {"opt_level": "3", "test": test, "debug_assertions": False},
        "package_id": name,
        "manifest_path": name + "/Cargo.toml",
        "fresh": False,
        "executable": None if name == "storage_engine" else "/build/history-scale-driver",
        "filenames": ["/build/" + name],
    }


def bundle(root):
    adapter = "current"
    source_sha = "209f31a4122174129bcf40b56c0598ac61e66352"
    messages = [artifact("storage_engine"), artifact("history-scale-driver", ["history-scale-current"]),
                {"reason": "build-finished", "success": True}]
    build = root / "build.jsonl"
    build.write_text("".join(json.dumps(row) + "\n" for row in messages), encoding="utf-8")
    registration = {
        "schema_version": 1, "source_sha": source_sha, "adapter": adapter,
        "driver_sha256": "a" * 64, "lockfile_sha256": "b" * 64,
        "manifest_sha256_before": "c" * 64, "manifest_sha256_after": "d" * 64,
        "build_features": ["history-scale-current"],
    }
    registration_path = root / "registration.json"
    registration_path.write_text(json.dumps(registration), encoding="utf-8")
    proof = {
        "schema_version": 1, "verified": True,
        **{key: registration[key] for key in ("source_sha", "adapter", "driver_sha256", "lockfile_sha256", "manifest_sha256_before", "manifest_sha256_after")},
        "binary_name": "history-scale-driver", "binary_sha256": "e" * 64,
        "cargo_config_sha256": "f" * 64, "toolchain_config_sha256": "0" * 64,
        "compiler_jsonl_sha256": BUILD.digest(build), "compiler_jsonl_file": build.name,
        "registration_sha256": BUILD.digest(registration_path), "registration_file": registration_path.name,
        "build_command": BUILD.expected_command(adapter), "registration": registration,
        "actual_artifacts": BUILD.verify(json.dumps(row) for row in messages),
        "rustc_verbose": "rustc 1.99.0 (fixture)\nhost: x86_64-unknown-linux-gnu",
        "cargo_version": "cargo 1.99.0 (fixture)",
        "build_identity": {"GITHUB_SHA": "1" * 40, "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1"},
        "isa": {"rustflags": "", "encoded_rustflags": "", "build_target": "", "cpu_info": {"lscpu": []}, "compiler_host": "x86_64-unknown-linux-gnu"},
    }
    path = root / "provenance.json"
    path.write_text(json.dumps(proof), encoding="utf-8")
    return path, proof


def binding_for(proof):
    return {key: proof[key] for key in ("source_sha", "adapter", "binary_name", "binary_sha256", "driver_sha256", "manifest_sha256_before", "lockfile_sha256", "cargo_config_sha256", "toolchain_config_sha256")}


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

    def test_duplicate_artifact_adapter_mix_and_post_completion_are_rejected(self):
        for messages in (
            [artifact("storage_engine"), artifact("storage_engine")],
            [artifact("history-scale-driver", ["history-scale-current", "history-scale-baseline"])],
            [{"reason": "build-finished", "success": True}, artifact("storage_engine")],
        ):
            with self.subTest(messages=messages), self.assertRaises(ValueError):
                self.verify(messages)

    def test_precompiled_bundle_binds_source_binary_driver_manifest_and_lock(self):
        with tempfile.TemporaryDirectory() as temporary:
            path, proof = bundle(Path(temporary))
            binding = binding_for(proof)
            self.assertTrue(BUILD.validate_precompiled(path, binding)["verified"])
            for key in binding:
                bad = {**binding, key: "different"}
                with self.subTest(key=key), self.assertRaisesRegex(ValueError, "mismatch"):
                    BUILD.validate_precompiled(path, bad)

    def test_precompiled_bundle_rejects_tampered_attached_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path, proof = bundle(root)
            for name in ("build.jsonl", "registration.json"):
                original = (root / name).read_bytes()
                (root / name).write_bytes(original + b" ")
                with self.subTest(name=name), self.assertRaisesRegex(ValueError, "digest mismatch"):
                    BUILD.validate_precompiled(path, binding_for(proof))
                (root / name).write_bytes(original)
            for key, value in (("compiler_jsonl_file", "../elsewhere"), ("build_command", ["cargo", "test"]), ("rustc_verbose", "rustc 1.98.0")):
                bad = copy.deepcopy(proof)
                bad[key] = value
                path.write_text(json.dumps(bad), encoding="utf-8")
                with self.subTest(key=key), self.assertRaises(ValueError):
                    BUILD.validate_precompiled(path, binding_for(proof))

    def test_precompiled_requires_actual_current_feature_and_isa(self):
        with tempfile.TemporaryDirectory() as temporary:
            path, proof = bundle(Path(temporary))
            for change in ("feature", "isa"):
                bad = copy.deepcopy(proof)
                if change == "feature":
                    bad["actual_artifacts"]["artifacts"]["history-scale-driver"]["features"] = []
                else:
                    bad.pop("isa")
                path.write_text(json.dumps(bad), encoding="utf-8")
                with self.subTest(change=change), self.assertRaises(ValueError):
                    BUILD.validate_precompiled(path, binding_for(proof))


if __name__ == "__main__":
    unittest.main()

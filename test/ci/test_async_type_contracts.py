"""The async type gate must distinguish intended rejections from failed builds."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "async_type_contracts", ROOT / ".github/scripts/check-async-type-contracts.py"
)
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


def rejection(fixture, code, *, macro=False):
    span = {"file_name": str(fixture), "is_primary": True, "expansion": None}
    if macro:
        span = {"file_name": "macro-definition.rs", "is_primary": True,
                "expansion": {"span": span}}
    message = {"reason": "compiler-message", "message": {
        "level": "error", "code": {"code": code} if code else None,
        "message": "expected type boundary",
        "spans": [span],
    }}
    return subprocess.CompletedProcess([], 101, json.dumps(message) + "\n", "")


class AsyncTypeContractsGateTests(unittest.TestCase):
    def test_gate_runs_once_in_async_shard(self):
        script = (ROOT / ".github/scripts/check-foundation.sh").read_text()
        shard = script.split("  foundation-async)\n", 1)[1].split(";;", 1)[0]
        self.assertEqual(script.count("check-async-type-contracts.py"), 1)
        self.assertIn("python3 .github/scripts/check-async-type-contracts.py", shard)

    def test_intended_diagnostics_require_fixture_origin_including_macro_expansion(self):
        fixture = Path("/fixture.rs")
        for macro in (False, True):
            with self.subTest(macro=macro):
                GATE.check_rejection(rejection(fixture, "E0716", macro=macro), fixture, "E0716")

    def test_compile_success_and_unrelated_failures_are_rejected(self):
        fixture = Path("/fixture.rs")
        cases = (
            subprocess.CompletedProcess([], 0, "", ""),
            subprocess.CompletedProcess([], 101, "", "dependency build failed"),
            rejection(fixture, "E0432"),
            rejection(Path("/dependency.rs"), "E0277"),
            rejection(fixture, None),
            subprocess.CompletedProcess([], 1, rejection(fixture, "E0277").stdout, ""),
        )
        for result in cases:
            with self.subTest(result=result), self.assertRaises(AssertionError):
                GATE.check_rejection(result, fixture, "E0277")

    def run_gate(self, root, *, listed=None, executed=None):
        calls = []

        def capture(command, **kwargs):
            manifest = Path(command[command.index("--manifest-path") + 1])
            data = tomllib.loads(manifest.read_text())
            calls.append((command, kwargs, data))
            if "--message-format=json" in command:
                name = command[command.index("--bin") + 1]
                _, file, code = next(item for item in GATE.REJECTIONS if item[0] == name)
                return rejection(root / "test/ci/fixtures" / file, code)
            output = ""
            if command[1] == "test":
                if "--list" in command:
                    output = listed if listed is not None else "".join(
                        f"{name}: test\n" for name in sorted(GATE.SELECT_TESTS))
                else:
                    output = executed if executed is not None else "".join(
                        f"test {name} ... ok\n" for name in sorted(GATE.SELECT_TESTS)
                    ) + "test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"
            return subprocess.CompletedProcess(command, 0, output, "")

        with patch.object(GATE.subprocess, "run", side_effect=capture):
            GATE.check(root)
        return calls

    def test_locked_offline_consumer_preserves_features_and_root_lock(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            lock = root / "Cargo.lock"
            lock.write_text("root lock sentinel\n")
            calls = self.run_gate(root)
            self.assertEqual(lock.read_text(), "root lock sentinel\n")
            self.assertEqual(len(calls), 7)
            for command, kwargs, manifest in calls:
                self.assertIn("--offline", command)
                self.assertEqual(kwargs["cwd"], root)
                self.assertGreater(kwargs["timeout"], 0)
                self.assertEqual(manifest["workspace"], {})
                for name, features in (("tokio", ["macros", "rt"]), ("futures-intrusive", ["std"])):
                    dependency = manifest["dependencies"][name]
                    self.assertFalse(dependency["default-features"])
                    self.assertEqual(dependency["features"], features)
                self.assertTrue(all(target["required-features"] == ["compile-contracts"]
                                    for target in manifest["bin"]))
                if command[1] != "generate-lockfile":
                    self.assertIn("--locked", command)
                    self.assertEqual(command[command.index("--target") + 1], GATE.TARGET)
                if command[1] == "test":
                    self.assertNotIn("--features", command)
                if command[1] == "check":
                    self.assertEqual(command[command.index("--features") + 1], "compile-contracts")
            self.assertEqual(list((root / "target/scope-consumers").iterdir()), [])

    def test_empty_test_inventory_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary, self.assertRaises(AssertionError):
            self.run_gate(Path(temporary), listed="0 tests, 0 benchmarks\n")

    def test_ignored_tests_cannot_count_as_executed(self):
        with tempfile.TemporaryDirectory() as temporary, self.assertRaises(AssertionError):
            self.run_gate(Path(temporary), executed="test result: ok. 0 passed; 0 failed; 5 ignored\n")

    def test_lock_failure_stops_before_checks(self):
        with tempfile.TemporaryDirectory() as temporary:
            with patch.object(GATE.subprocess, "run", side_effect=subprocess.CalledProcessError(1, [])) as run:
                with self.assertRaises(subprocess.CalledProcessError):
                    GATE.check(Path(temporary))
            self.assertEqual(run.call_count, 1)


if __name__ == "__main__":
    unittest.main()

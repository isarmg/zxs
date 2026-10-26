"""Keep the keyed-task regression gate offline, locked and isolated."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "check_runtime_join_map", ROOT / ".github/scripts/check-runtime-join-map.py"
)
CHECKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKER)


class RuntimeJoinMapGateTests(unittest.TestCase):
    def test_gate_uses_full_features_and_keeps_root_lock_untouched(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            lock = root / "Cargo.lock"
            lock.write_text("root lock sentinel\n")
            calls = []

            def capture(command, **kwargs):
                manifest = Path(command[command.index("--manifest-path") + 1])
                calls.append((command, kwargs, tomllib.loads(manifest.read_text())))

            with patch.object(CHECKER.subprocess, "run", side_effect=capture):
                CHECKER.check(root)
            self.assertEqual(lock.read_text(), "root lock sentinel\n")
            self.assertEqual(len(calls), 2)
            self.assertEqual(calls[0][0][:2], ["cargo", "generate-lockfile"])
            self.assertEqual(calls[1][0][:2], ["cargo", "test"])
            self.assertIn("--locked", calls[1][0])
            for command, kwargs, manifest in calls:
                self.assertIn("--offline", command)
                self.assertTrue(kwargs["check"])
                self.assertGreater(kwargs["timeout"], 0)
                self.assertEqual(kwargs["cwd"], root)
                self.assertEqual(manifest["workspace"], {})
                tests = {target["name"]: target["path"] for target in manifest["test"]}
                self.assertEqual(tests["fs_asyncify"], str(root / "test/foundation/tokio/tests/fs_asyncify.rs"))
                for dependency in ("tokio",):
                    self.assertEqual(
                        manifest["dependencies"][dependency]["features"], ["full", "adapters-full"]
                    )
            self.assertEqual(list((root / "target/scope-consumers").iterdir()), [])

    def test_lock_resolution_failure_stops_before_test_execution(self):
        with tempfile.TemporaryDirectory() as directory:
            failure = subprocess.CalledProcessError(1, ["cargo", "generate-lockfile"])
            with patch.object(CHECKER.subprocess, "run", side_effect=failure) as run:
                with self.assertRaises(subprocess.CalledProcessError):
                    CHECKER.check(Path(directory))
            self.assertEqual(run.call_count, 1)


if __name__ == "__main__":
    unittest.main()

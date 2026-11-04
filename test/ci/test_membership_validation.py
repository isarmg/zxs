"""The experimental runner must select real bounded suites and retain failures."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / ".github/scripts/check-membership-activation.py"
SPEC = importlib.util.spec_from_file_location("membership_validation", SCRIPT)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class MembershipValidation(unittest.TestCase):
    def test_only_engine_requests_its_fault_injection_feature(self):
        engine = runner.build_command("storage-engine")
        server = runner.build_command("storage-server")
        self.assertIn("fault-injection", engine)
        self.assertNotIn("--features", server)
        for command in (engine, server):
            self.assertIn("--lib", command)
            self.assertIn("--locked", command)
            self.assertIn("--offline", command)
            self.assertIn("--no-run", command)
            self.assertNotIn("--workspace", command)

    def test_group_labels_are_distinct_and_include_both_consumer_protocols(self):
        groups = runner.GROUPS + runner.IO_ENGINE_GROUPS + runner.IO_SERVER_GROUPS
        labels = [label for label, _, _ in groups]
        self.assertEqual(len(labels), len(set(labels)))
        self.assertIn("http-transfer", labels)
        self.assertIn("smb-stream-read", labels)
        self.assertIn("activation", labels)

    def test_empty_test_inventory_is_a_failure(self):
        inventory = subprocess.CompletedProcess([], 0, "0 tests, 0 benchmarks\n", "")
        with patch.object(runner.subprocess, "run", return_value=inventory):
            with self.assertRaisesRegex(RuntimeError, "empty test group"):
                runner.run_group(Path("unused"), "missing", "renamed::", ())

    def test_a_failed_group_keeps_its_nonzero_status(self):
        inventory = subprocess.CompletedProcess([], 0, "some::real_test: test\n", "")
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(runner, "EVIDENCE", Path(directory)):
                with patch.object(runner.subprocess, "run", return_value=inventory):
                    with patch.object(runner.subprocess, "Popen") as popen:
                        process = popen.return_value.__enter__.return_value
                        process.stdout = iter(["test result: FAILED\n"])
                        process.wait.return_value = 101
                        result = runner.run_group(Path("unused"), "failure", "some::", ())
            self.assertEqual(result["exit_code"], 101)
            self.assertEqual(result["listed_tests"], 1)
            self.assertIn("FAILED", (Path(directory) / "failure.log").read_text())


if __name__ == "__main__":
    unittest.main()

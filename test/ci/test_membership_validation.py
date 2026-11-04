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
        platform = runner.build_command("platform-io")
        self.assertIn("fault-injection", engine)
        self.assertNotIn("--features", server)
        self.assertNotIn("--features", platform)
        for command in (engine, server, platform):
            self.assertIn("--lib", command)
            self.assertIn("--locked", command)
            self.assertIn("--offline", command)
            self.assertIn("--no-run", command)
            self.assertNotIn("--workspace", command)

    def test_group_labels_are_distinct_and_include_both_consumer_protocols(self):
        groups = (runner.GROUPS + runner.IO_ENGINE_GROUPS
                  + runner.IO_PLATFORM_GROUPS + runner.IO_SERVER_GROUPS)
        labels = [label for label, _, _ in groups]
        labels += [label for label, _, _, _ in runner.IO_BLOCKING_PROBES]
        self.assertEqual(len(labels), len(set(labels)))
        self.assertIn("http-transfer", labels)
        self.assertIn("smb-stream-read", labels)
        self.assertIn("activation", labels)
        self.assertIn("platform-payload", labels)
        self.assertIn("upload-resources", labels)
        self.assertIn("upload-workspaces", labels)
        self.assertIn("upload-regressions", labels)
        self.assertIn("disk-export", labels)
        self.assertIn("writer-fencing", labels)
        self.assertIn("workspace-commit", labels)
        self.assertIn("workspace-inventory-fencing", labels)
        self.assertIn("container-storage", labels)
        self.assertIn("journal-owned-resources", labels)
        self.assertIn("journal-regressions", labels)
        self.assertIn("container-blocking-tails", labels)
        self.assertIn("publication", labels)
        self.assertIn("publication-evidence", labels)
        self.assertIn("publication-blocking-tails", labels)
        self.assertIn("journal-blocking-tails", labels)
        for _, _, source, filename in runner.IO_BLOCKING_PROBES:
            self.assertTrue((runner.ROOT / source).is_file())
            self.assertIn("io_tails", filename)

    def test_syscall_probes_use_strict_c_build_and_record_the_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "target/membership-probes/journal_io_tails.so"
            output.parent.mkdir(parents=True)
            output.write_bytes(b"test-only compiler output")
            with patch.object(runner, "ROOT", root), patch.object(runner, "EVIDENCE", root):
                with patch.object(runner.subprocess, "run") as compiler:
                    actual, digest = runner.build_io_interposer(
                        "journal-blocking-tails", "test/journal_io_tails.c", output.name)
            self.assertEqual(actual, output)
            self.assertEqual(len(digest), 64)
            self.assertTrue(compiler.call_args.kwargs["check"])
            self.assertIn("-Werror", compiler.call_args.args[0])
            self.assertIn("test/journal_io_tails.c", compiler.call_args.args[0])

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
                    with patch.object(runner.subprocess, "Popen") as popen, patch("builtins.print"):
                        process = popen.return_value.__enter__.return_value
                        process.stdout = iter(["test result: FAILED\n"])
                        process.wait.return_value = 101
                        result = runner.run_group(Path("unused"), "failure", "some::", ())
                        command = popen.call_args.args[0]
                        self.assertEqual(command[:4], [
                            "timeout", "--signal=TERM", "--kill-after=15s", "600s"])
            self.assertEqual(result["exit_code"], 101)
            self.assertEqual(result["listed_tests"], 1)
            self.assertIn("FAILED", (Path(directory) / "failure.log").read_text())

    def test_timed_out_group_is_recorded_as_failure(self):
        inventory = subprocess.CompletedProcess([], 0, "some::real_test: test\n", "")
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(runner, "EVIDENCE", Path(directory)):
                with patch.object(runner.subprocess, "run", return_value=inventory):
                    with patch.object(runner.subprocess, "Popen") as popen:
                        process = popen.return_value.__enter__.return_value
                        process.stdout = iter([])
                        process.wait.return_value = 124
                        result = runner.run_group(Path("unused"), "timeout", "some::", ())
            self.assertEqual(result["exit_code"], 124)
            self.assertEqual(result["timeout_seconds"], 600)

    def test_interposer_environment_is_confined_to_selected_group(self):
        inventory = subprocess.CompletedProcess([], 0, "some::real_test: test\n", "")
        environment = {"LD_PRELOAD": "/specific/probe.so"}
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(runner, "EVIDENCE", Path(directory)):
                with patch.object(runner.subprocess, "run", return_value=inventory) as listing:
                    with patch.object(runner.subprocess, "Popen") as popen:
                        process = popen.return_value.__enter__.return_value
                        process.stdout = iter([])
                        process.wait.return_value = 0
                        runner.run_group(Path("unused"), "blocked", "some::", ("--ignored",),
                                         env=environment)
                        self.assertEqual(listing.call_args.kwargs["env"], environment)
                        self.assertEqual(popen.call_args.kwargs["env"], environment)
                        runner.run_group(Path("unused"), "ordinary", "some::", ())
                        self.assertIsNone(listing.call_args.kwargs["env"])
                        self.assertIsNone(popen.call_args.kwargs["env"])


if __name__ == "__main__":
    unittest.main()

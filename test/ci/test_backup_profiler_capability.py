"""Bounded capability protocol and lifecycle contracts; no real profiler in unit tests."""
import importlib.util
import io
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

from test_targeted_ci import CI

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "backup_profiler_capability",
    ROOT / ".github/scripts/backup-profiler-capability.py",
)
PROBE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PROBE
SPEC.loader.exec_module(PROBE)
HEAD = "a" * 40


class CapabilityProtocol(unittest.TestCase):
    def test_absent_tools_do_not_spawn(self):
        with mock.patch.object(PROBE.shutil, "which", return_value=None):
            with mock.patch.object(PROBE, "_execute") as execute:
                result = PROBE.report(HEAD)
        execute.assert_not_called()
        self.assertEqual(result["head"], HEAD)
        for tool in result["tools"].values():
            self.assertEqual(tool["status"], "not_installed")
            self.assertFalse(tool["present"])
            self.assertEqual(tool["probes"], [])

    def test_perf_requires_count_and_the_exact_child_returncode(self):
        for expected in (0, 1):
            for status, code, output, wanted in (
                ("completed", expected, b"0.001;msec;cpu-clock:u;1;100.00", "available"),
                ("completed", expected, b"<not counted>;msec;cpu-clock:u;", "tool_error"),
                ("completed", 9, b"1;msec;cpu-clock:u;", "tool_error"),
                ("completed", 1, b"No permission to enable cpu-clock:u", "permission_denied"),
                ("timeout", 0, b"1;msec;cpu-clock:u;", "timeout"),
                ("output_limit", 0, b"1;msec;cpu-clock:u;", "output_limit"),
            ):
                with self.subTest(expected=expected, status=status, code=code, wanted=wanted):
                    captured = PROBE.Capture(status, code, output)
                    with mock.patch.object(PROBE, "_execute", return_value=captured):
                        result = PROBE._probe("perf", "/usr/bin/perf", "/usr/bin/true", expected, 40)
                    self.assertEqual(result["status"], wanted)

    def test_gdb_requires_exit_evidence_and_rejects_mi_error(self):
        normal = b'*stopped,reason="exited-normally"\n'
        false = b'*stopped,reason="exited",exit-code="01"\n'
        self.assertTrue(PROBE._gdb_exited(normal, 0))
        self.assertTrue(PROBE._gdb_exited(false, 1))
        self.assertFalse(PROBE._gdb_exited(normal, 1))
        self.assertFalse(PROBE._gdb_exited(b'^error,msg="secret path"\n' + normal, 0))
        for code, output, cleanup in (
            (0, false, "not_needed"),
            (1, normal, "not_needed"),
            (0, normal, "unconfirmed"),
        ):
            captured = PROBE.Capture("completed", code, output, cleanup)
            with mock.patch.object(PROBE, "_execute", return_value=captured):
                result = PROBE._probe("gdb", "/usr/bin/gdb", "/usr/bin/true", 0, 40)
            self.assertNotEqual(result["status"], "available")

    def test_gdb_arguments_disable_external_startup_without_disabling_aslr(self):
        arguments = PROBE._gdb_arguments("/usr/bin/gdb", "/usr/bin/false")
        self.assertEqual(arguments[-2:], ["--args", "/usr/bin/false"])
        for option in ("--nx", "--nh", "--batch", "--return-child-result", "--interpreter=mi2"):
            self.assertIn(option, arguments)
        for command in (
            "set auto-load off",
            "set auto-load safe-path /dev/null",
            "set debuginfod enabled off",
            "set startup-with-shell off",
            "set disable-randomization off",
        ):
            index = arguments.index(command)
            self.assertEqual(arguments[index - 1], "-iex")
        self.assertNotIn("attach", arguments)
        self.assertEqual(PROBE.ENV["DEBUGINFOD_URLS"], "")
        self.assertNotIn("LD_PRELOAD", PROBE.ENV)

    def test_report_never_exposes_tool_output_or_tool_paths(self):
        captured = PROBE.Capture(
            "completed", 1, b"permission denied /private/user/path TOKEN=secret",
        )
        with mock.patch.object(PROBE.shutil, "which", return_value="/private/tool"):
            with mock.patch.object(PROBE.os.path, "isfile", return_value=True):
                with mock.patch.object(PROBE, "_execute", return_value=captured) as execute:
                    result = PROBE.report(HEAD)
        serialized = json.dumps(result)
        self.assertNotIn("/private", serialized)
        self.assertNotIn("secret", serialized)
        self.assertLessEqual(execute.call_count, 6)
        for tool in result["tools"].values():
            self.assertNotEqual(tool["status"], "available")

    def test_ptrace_without_explicit_permission_failure_is_generic_tool_error(self):
        self.assertEqual(PROBE._category(b"ptrace: No such process"), "tool_error")
        self.assertEqual(
            PROBE._category(b"ptrace: Permission denied"), "permission_denied",
        )

    def test_version_exposes_only_bounded_numeric_version(self):
        captured = PROBE.Capture("completed", 0, b"perf version 6.8.12 /private/secret\n")
        with mock.patch.object(PROBE, "_execute", return_value=captured):
            result = PROBE._version("perf", "/usr/bin/perf", 40)
        self.assertEqual(result["number"], "6.8.12")
        self.assertNotIn("private", json.dumps(result))


    def test_every_execution_stage_failure_stops_and_retains_nonzero_cli_report(self):
        successes = [
            PROBE.Capture("completed", 0, b"perf version 6.8"),
            PROBE.Capture("completed", 0, b"1;msec;cpu-clock:u;"),
            PROBE.Capture("completed", 1, b"1;msec;cpu-clock:u;"),
            PROBE.Capture("completed", 0, b"GNU gdb 15.1"),
            PROBE.Capture("completed", 0, b'*stopped,reason="exited-normally"'),
            PROBE.Capture("completed", 1, b'*stopped,reason="exited",exit-code="01"'),
        ]
        failures = [
            PROBE.Capture(status, None, b"", "confirmed")
            for status in (
                "timeout", "output_limit", "internal_error", "spawn_error",
                "not_run_deadline", "inferior_still_running", "inferior_exit_unconfirmed",
            )
        ] + [
            PROBE.Capture("completed", 0, b"", cleanup)
            for cleanup in ("unconfirmed", "inferior_unconfirmed")
        ]
        for stage in range(6):
            for failure in failures:
                with self.subTest(stage=stage, status=failure.status, cleanup=failure.cleanup):
                    with tempfile.TemporaryDirectory() as directory:
                        path = Path(directory) / "report.json"
                        arguments = ["probe", "--head", HEAD, "--output", str(path)]
                        captures = successes[:stage] + [failure]
                        with (
                            mock.patch.object(
                                PROBE.shutil, "which", return_value="/usr/bin/tool",
                            ),
                            mock.patch.object(PROBE.os.path, "isfile", return_value=True),
                            mock.patch.object(PROBE.os, "pidfd_open", create=True),
                            mock.patch.object(PROBE.signal, "pidfd_send_signal", create=True),
                            mock.patch.object(
                                PROBE, "_execute", side_effect=captures,
                            ) as execute,
                            mock.patch.object(sys, "argv", arguments),
                            mock.patch("sys.stdout", new=io.StringIO()),
                        ):
                            code = PROBE.main()
                        self.assertEqual(code, 1)
                        self.assertEqual(execute.call_count, stage + 1)
                        saved = json.loads(path.read_text())
                        self.assertFalse(saved["complete"])
                        self.assertEqual(saved["status"], "failed")
                        self.assertEqual(saved["head"], HEAD)
                        if stage < 3:
                            self.assertEqual(saved["tools"]["gdb"]["status"], "not_run")

    def test_permission_denied_is_completed_unavailability_not_project_success(self):
        denied = PROBE.Capture("completed", 1, b"No permission to enable cpu-clock:u")
        with mock.patch.object(PROBE.shutil, "which", return_value="/usr/bin/tool"):
            with mock.patch.object(PROBE.os.path, "isfile", return_value=True):
                with mock.patch.object(PROBE.os, "pidfd_open", create=True):
                    with mock.patch.object(PROBE.signal, "pidfd_send_signal", create=True):
                        with mock.patch.object(PROBE, "_execute", return_value=denied) as execute:
                            result = PROBE.report(HEAD)
        self.assertEqual(execute.call_count, 6)
        self.assertTrue(result["complete"])
        self.assertEqual(result["status"], "completed")
        for tool in result["tools"].values():
            self.assertEqual(tool["status"], "unavailable")
        self.assertNotIn("tests_passed", result)

    def test_whole_run_deadline_marks_failure_without_starting_tools(self):
        with mock.patch.object(PROBE.time, "monotonic", side_effect=[0, 41]):
            with mock.patch.object(PROBE, "_execute") as execute:
                result = PROBE.report(HEAD)
        execute.assert_not_called()
        self.assertFalse(result["complete"])
        self.assertEqual(result["tools"]["perf"]["status"], "not_run_deadline")
        self.assertEqual(result["tools"]["gdb"]["status"], "not_run")


class BoundedLifecycle(unittest.TestCase):
    def configured(self, code=0):
        process = mock.Mock()
        process.pid = 321
        process.returncode = code
        process.poll.return_value = code
        process.wait.return_value = code
        process.stdout.fileno.return_value = 4
        selector = mock.Mock()
        selector.select.return_value = [(SimpleNamespace(fileobj=process.stdout), 1)]
        selector.get_map.return_value = {}
        return process, selector

    def execute(self, process, selector, reads, clock=None, program=None):
        if clock is None:
            clock = lambda: 0
        with mock.patch.object(PROBE.subprocess, "Popen", return_value=process):
            with mock.patch.object(PROBE.selectors, "DefaultSelector", return_value=selector):
                with mock.patch.object(PROBE.os, "set_blocking"):
                    with mock.patch.object(PROBE.os, "read", side_effect=reads):
                        with mock.patch.object(PROBE.time, "monotonic", side_effect=clock):
                            return PROBE._execute(
                                ["/usr/bin/true"], 5, overall=40, program=program,
                            )

    def test_zero_and_nonzero_are_reaped_without_relaunch(self):
        for code in (0, 1):
            process, selector = self.configured(code)
            result = self.execute(process, selector, [b""])
            self.assertEqual((result.status, result.returncode), ("completed", code))
            process.kill.assert_not_called()
            process.stdout.close.assert_called_once()
            selector.close.assert_called_once()

    def test_deadline_is_not_reset_by_output_or_late_zero_exit(self):
        process, selector = self.configured(0)
        result = self.execute(process, selector, [b"output"], iter([0, 0, 0, 6]))
        self.assertEqual(result.status, "timeout")
        process.wait.assert_called_once_with(timeout=1)

    def test_overall_deadline_prevents_start(self):
        with mock.patch.object(PROBE.time, "monotonic", return_value=41):
            with mock.patch.object(PROBE.subprocess, "Popen") as start:
                result = PROBE._execute(["/usr/bin/true"], 5, overall=40)
        start.assert_not_called()
        self.assertEqual(result.status, "not_run_deadline")

    def test_output_limit_kills_only_the_direct_process(self):
        process, selector = self.configured(None)
        with mock.patch.object(PROBE, "LIMIT", 8):
            result = self.execute(process, selector, [b"x" * 9])
        self.assertEqual(result.status, "output_limit")
        self.assertEqual(len(result.output), 8)
        process.kill.assert_called_once()
        process.wait.assert_called_once()

    def test_io_failure_and_keyboard_interrupt_cleanup_without_retry(self):
        for error in (OSError("private"), KeyboardInterrupt()):
            process, selector = self.configured(None)
            if isinstance(error, KeyboardInterrupt):
                with self.assertRaises(KeyboardInterrupt):
                    self.execute(process, selector, error)
            else:
                result = self.execute(process, selector, error)
                self.assertEqual(result.status, "internal_error")
            process.kill.assert_called_once()
            process.wait.assert_called_once()
            process.stdout.close.assert_called_once()

    def test_cleanup_failure_is_explicit(self):
        process, _ = self.configured(None)
        process.wait.side_effect = subprocess.TimeoutExpired("private", 1)
        self.assertEqual(PROBE._cleanup(process, None), "unconfirmed")

    def test_inferior_requires_parent_and_stable_physical_program_binding(self):
        output = b'=thread-group-started,id="i1",pid="777"'
        identity = SimpleNamespace(st_dev=1, st_ino=2)
        with mock.patch.object(PROBE, "_child_identity", return_value=(999, 1)):
            with mock.patch.object(PROBE.os, "pidfd_open") as opened:
                self.assertIsNone(PROBE._inferior_handle(output, 321, "/usr/bin/true"))
                opened.assert_not_called()
        with mock.patch.object(PROBE, "_child_identity", side_effect=[(321, 1), (321, 2)]):
            with mock.patch.object(PROBE.os, "pidfd_open", return_value=8):
                with mock.patch.object(PROBE.os, "stat", return_value=identity):
                    with mock.patch.object(PROBE.os, "close") as close:
                        self.assertIsNone(PROBE._inferior_handle(output, 321, "/usr/bin/true"))
                        close.assert_called_once_with(8)

    def test_spawn_error_is_sanitized(self):
        with mock.patch.object(PROBE.subprocess, "Popen", side_effect=OSError("/secret")):
            result = PROBE._execute(["/usr/bin/true"], 5)
        self.assertEqual(result.status, "spawn_error")
        self.assertEqual(result.output, b"")


    def test_perf_timeout_without_mi_cannot_claim_inferior_cleanup(self):
        process, selector = self.configured(None)
        result = self.execute(
            process, selector, [b""], iter([0, 0, 6]), program="/usr/bin/true",
        )
        self.assertEqual(result.status, "timeout")
        self.assertEqual(result.cleanup, "inferior_unconfirmed")
        self.assertTrue(PROBE._execution_failed({
            "status": result.status, "cleanup": result.cleanup,
        }))
        process.kill.assert_called_once()
        process.wait.assert_called_once()


class CapabilitySelection(unittest.TestCase):
    def test_each_diagnostic_path_selects_only_python_bounded_groups(self):
        policy = json.loads((ROOT / CI.PLAN_PATH).read_text())
        paths = (
            ".github/scripts/backup-profiler-capability.py",
            "test/ci/test_backup_profiler_capability.py",
            ".github/workflows/backup-profiler-capability.yml",
            "docs/validation/backup-profiler-capability.md",
        )
        for path in paths:
            with self.subTest(path=path):
                plan = CI.select_plan(policy, [path])
                self.assertEqual(plan["groups"], ["backup-profiler-capability", "ci-selection"])
                self.assertEqual(plan["rust_targets"], [])
                self.assertIn("test_backup_profiler_capability", plan["python_tests"])
                self.assertEqual(len(plan["python_tests"]), 8)

    def test_workflow_is_manual_read_only_pinned_and_uploads_only_sanitized_report(self):
        workflow = (ROOT / ".github/workflows/backup-profiler-capability.yml").read_text()
        event = re.search(r"(?ms)^on:\s*\n(.*?)(?=^[^\s#])", workflow)
        self.assertIsNotNone(event)
        self.assertRegex(event.group(1), r"workflow_dispatch\s*:")
        self.assertNotRegex(event.group(1), r"\b(push|pull_request|pull_request_target|schedule)\s*:")
        permissions = re.search(r"(?ms)^permissions:\s*\n(.*?)(?=^[^\s#])", workflow)
        self.assertIsNotNone(permissions)
        self.assertRegex(permissions.group(1), r"contents\s*:\s*read\b")
        self.assertNotRegex(permissions.group(1), r":\s*write\b")
        self.assertRegex(workflow, r"timeout-minutes\s*:\s*2\b")
        self.assertRegex(workflow, r"persist-credentials\s*:\s*false\b")
        uses = re.findall(r"\buses:\s*(\S+)", workflow)
        self.assertEqual(len(uses), 2)
        for action in uses:
            self.assertRegex(action, r"^actions/(checkout|upload-artifact)@[0-9a-f]{40}$")
        self.assertRegex(workflow, r"ref\s*:\s*\$\{\{\s*github.sha\s*\}\}")
        self.assertRegex(workflow, r'test\s+"\$\(git\s+rev-parse\s+HEAD\)"\s+=\s+"\$GITHUB_SHA"')
        self.assertRegex(workflow, r'--head\s+"\$GITHUB_SHA"')
        self.assertRegex(
            workflow, r'--output\s+"\$RUNNER_TEMP/backup-profiler-capability.json"',
        )
        self.assertRegex(workflow, r"if\s*:\s*(?:\$\{\{\s*)?always\(\)")
        self.assertRegex(workflow, r"if-no-files-found\s*:\s*error\b")
        self.assertRegex(
            workflow,
            r"path\s*:\s*\$\{\{\s*runner.temp\s*\}\}/backup-profiler-capability.json",
        )
        self.assertNotRegex(workflow, r"(?m)^\s*(sudo|apt|apt-get|dnf|yum)\b")
        self.assertNotRegex(workflow, r"\bpip(?:3)?\s+install\b")


if __name__ == "__main__":
    unittest.main()

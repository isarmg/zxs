"""Deterministic lifecycle/measurement races without running a storage workload."""

from contextlib import ExitStack
import errno
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools/history-scale-bench"))
import run as runner
from test_tooling import separated_rows, write_rows


class Clock:
    def __init__(self):
        self.now = 0.0

    def sleep(self, seconds):
        self.now += seconds


class Child:
    pid = 1234

    def __init__(self, clock, invocation, *, end, code, resource, wait_error):
        self.clock, self.end, self.code = clock, end, code
        self.resource = Path(invocation[3]) if resource else None
        self.stopped = False
        self.wait_error = wait_error

    def poll(self):
        return -15 if self.stopped else self.code if self.clock.now >= self.end else None

    def wait(self, **_kwargs):
        if self.poll() is None:
            raise AssertionError("fake child must reach an observed terminal state")
        if self.wait_error:
            raise OSError(errno.EIO, "fixture wait failure")
        if self.resource is not None and not self.stopped:
            self.resource.write_text("Maximum resident set size (kbytes): 32\n", encoding="utf-8")
        return self.poll()


class SupervisorTests(unittest.TestCase):
    def observe(self, root, *, end=0.2, code=0, resource=True, resume=None, fatal=False, complete=True,
                poll=0.1, overrun=0.0, known_rss=0, allocated=4096, phase_seconds=2.0,
                wait_error=False, final_error=False, snapshot_delay=0.0, final_snapshot_delay=0.0):
        clock = Clock()
        sample = root / "sample"
        sample.mkdir()
        budget = SimpleNamespace(
            output_root=root, process_poll_seconds=poll, disk_poll_seconds=1.0,
            phase_seconds=phase_seconds, rss_mib=8, disk_mib=8, cohort_disk_mib=16,
            minimum_free_mib=1, query_protocol="separate-outcome-health",
        )
        children = []

        def launch(invocation, **_kwargs):
            child = Child(clock, invocation, end=end, code=code, resource=resource, wait_error=wait_error)
            children.append(child)
            rows = separated_rows()
            write_rows(Path(invocation[invocation.index("--result") + 1]), rows if complete else rows[:-1])
            return child

        def snapshot(pid):
            if pid != Child.pid:
                return {"pid": pid, "valid": True, "memory": {"VmRSS_bytes": known_rss}, "io": {}}
            if fatal:
                raise PermissionError(errno.EACCES, "live status denied", f"/proc/{pid}/status")
            if children[0].poll() is not None:
                clock.now += final_snapshot_delay
                return None
            if resume is not None and clock.now >= resume:
                clock.now += snapshot_delay
                return {"pid": pid, "valid": True, "memory": {"VmRSS_bytes": 8192}, "io": {"rchar": 1}}
            return {
                "pid": pid, "valid": False, "memory": {}, "io": None, "rss_pending": True,
                "memory_error": {"reason": "missing_live_rss", "initial_state": "R", "state": "R",
                                 "initial_status": "State:\tR\n", "rechecked_status": "State:\tR\n"},
            }

        disk_reads = []

        def diskstats():
            disk_reads.append(True)
            if final_error and len(disk_reads) > 1:
                raise OSError(errno.EIO, "fixture final diskstats failure")
            return "fixture\n"

        with ExitStack() as stack:
            for name, value in (
                ("subprocess.Popen", launch), ("time.monotonic", lambda: clock.now),
                ("time.sleep", lambda delay: clock.sleep(delay + overrun)),
                ("process_tree", lambda _pid: [Child.pid, Child.pid + 1] if known_rss else [Child.pid]),
                ("process_snapshot", snapshot), ("diskstats", diskstats),
                ("inventory", lambda _path: {"allocated_bytes": allocated}),
                ("shutil.disk_usage", lambda _path: SimpleNamespace(free=16 * 1024 * 1024)),
                ("stop_process", lambda child: setattr(child, "stopped", True)),
            ):
                stack.enter_context(patch("run." + name, value))
            result = runner.run_stage(Path("/fixture-driver"), sample / "pool", sample,
                                      "build", 8, "timing", budget, 10.0, "candidate")
        terminal = json.loads((sample / "build.supervisor.json").read_text())
        self.assertEqual(result["supervisor"], terminal)
        return result, children[0], clock, sample

    def test_two_nonterminal_missing_rss_samples_then_clean_exit(self):
        with tempfile.TemporaryDirectory() as directory:
            result, child, _, sample = self.observe(Path(directory))
            self.assertEqual(result["status"], "passed")
            self.assertFalse(child.stopped)
            self.assertEqual(result["supervisor"]["returncode"], 0)
            self.assertEqual(result["supervisor"]["time_max_process_rss_bytes"], 32 * 1024)
            samples = [json.loads(line) for line in (sample / "build.process-samples.jsonl").read_text().splitlines()]
            self.assertGreaterEqual(len(samples), 2)
            self.assertTrue(all(not row["rss_complete"] for row in samples))
            self.assertTrue(all("VmRSS_bytes" not in row["processes"][0]["memory"] for row in samples))
            self.assertEqual(result["supervisor"]["rss_pending_at_exit"], [])

    def test_transient_missing_rss_can_return_to_a_valid_sample(self):
        with tempfile.TemporaryDirectory() as directory:
            result, child, _, sample = self.observe(Path(directory), end=0.3, resume=0.1)
            self.assertEqual(result["status"], "passed")
            self.assertFalse(child.stopped)
            samples = [json.loads(line) for line in (sample / "build.process-samples.jsonl").read_text().splitlines()]
            self.assertFalse(samples[0]["rss_complete"])
            self.assertTrue(any(row["rss_complete"] for row in samples[1:]))

    def test_persistent_missing_rss_is_bounded_and_never_green(self):
        with tempfile.TemporaryDirectory() as directory:
            result, child, clock, _ = self.observe(Path(directory), end=10.0)
            self.assertEqual(result["status"], "failed")
            self.assertEqual(result["reason"], "monitoring_error")
            self.assertTrue(child.stopped)
            self.assertLessEqual(clock.now, 1.0)
            self.assertIn("VmRSS", result["supervisor"]["monitor_error"]["message"])

    def test_large_poll_interval_cannot_extend_rss_resolution_deadline(self):
        for overrun in (0.0, 1.0):
            with self.subTest(overrun=overrun), tempfile.TemporaryDirectory() as directory:
                result, _, clock, _ = self.observe(Path(directory), end=0.8, poll=1.0, overrun=overrun)
                self.assertEqual(result["status"], "failed")
                self.assertIn("VmRSS", result["supervisor"]["monitor_error"]["message"])
                if overrun == 0.0:
                    self.assertLessEqual(clock.now, 0.5)

    def test_pending_rss_does_not_disable_other_resource_budgets(self):
        for options, reason in (({"known_rss": 9 * 1024 * 1024}, "sampled_process_tree_rss_budget"),
                                ({"allocated": 9 * 1024 * 1024}, "sample_allocated_disk_budget"),
                                ({"phase_seconds": 0.05}, "phase_wall_time_budget")):
            with self.subTest(options=options), tempfile.TemporaryDirectory() as directory:
                result, child, _, _ = self.observe(Path(directory), end=10.0, **options)
                self.assertEqual(result["status"], "incomplete")
                self.assertTrue(child.stopped)
                self.assertEqual(result["supervisor"]["budget_stop_reason"], reason)

    def test_status_reads_cannot_resolve_pending_after_the_deadline(self):
        for options in ({"resume": 0.1, "snapshot_delay": 0.5}, {"final_snapshot_delay": 0.5}):
            with self.subTest(options=options), tempfile.TemporaryDirectory() as directory:
                result, _, _, _ = self.observe(Path(directory), **options)
                self.assertEqual(result["status"], "failed")
                self.assertIn("VmRSS", result["supervisor"]["monitor_error"]["message"])

    def test_cleanup_and_final_read_errors_keep_failed_terminal_record(self):
        for options, operation in (({"wait_error": True}, "process_cleanup"),
                                   ({"final_error": True}, "diskstats_after")):
            with self.subTest(options=options), tempfile.TemporaryDirectory() as directory:
                result, _, _, _ = self.observe(Path(directory), **options)
                self.assertEqual(result["status"], "failed")
                self.assertIn(operation, [error["operation"] for error in result["supervisor"]["finalization_errors"]])

    def test_complete_marker_cannot_replace_exit_resource_or_full_oracle(self):
        for options in ({"code": 3}, {"resource": False}, {"complete": False}):
            with self.subTest(options=options), tempfile.TemporaryDirectory() as directory:
                result, _, _, _ = self.observe(Path(directory), **options)
                self.assertNotEqual(result["status"], "passed")

    def test_monitor_exception_always_keeps_terminal_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            result, child, _, sample = self.observe(Path(directory), end=10.0, fatal=True)
            self.assertEqual(result["status"], "failed")
            self.assertTrue(child.stopped)
            self.assertEqual(result["supervisor"]["monitor_error"]["type"], "PermissionError")
            self.assertEqual(result["supervisor"]["returncode"], -15)
            self.assertTrue((sample / "build.diskstats.after.txt").is_file())


if __name__ == "__main__":
    unittest.main()

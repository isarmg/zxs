"""Guard telemetry enumeration fault-runner evidence without compiling Rust or C."""
import contextlib
import io
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
TEST_NAME = "telemetry::tests::enumeration_failure_preserves_source_and_uses_live_memory_history"
GATE = ROOT / ".github/scripts/check-telemetry-enumeration-io.py"
RUNNER = "test/server/unit/telemetry/history/enumeration-eio.sh"
MARKER = "ZXS_TELEMETRY_ENUM_EIO_AFTER_ENTRY"
WARNING = "monitoring cache unavailable; using bounded memory history"
artifact = runpy.run_path(str(GATE))["test_artifact"]


def candidate():
    return {"reason": "compiler-artifact", "target": {"name": "storage_server", "kind": ["lib"]},
            "profile": {"test": True}, "executable": "/current/telemetry-libtest"}


def encode(items):
    return "\n".join(json.dumps(item) for item in items)


class TelemetryEnumerationRunnerTests(unittest.TestCase):
    def test_artifact_requires_one_current_libtest_and_complete_successful_build(self):
        current = candidate()
        finished = {"reason": "build-finished", "success": True}
        self.assertEqual(artifact(encode([current, finished])), current["executable"])
        for items in ([], [current], [finished], [current, current, finished],
                      [current, finished, finished],
                      [current, {"reason": "build-finished", "success": False}],
                      [dict(current, profile={"test": False}), finished],
                      [dict(current, executable=None), finished],
                      [dict(current, target={"name": "storage_server", "kind": ["bin"]}), finished],
                      [dict(current, target={"name": "storage_engine", "kind": ["lib"]}), finished]):
            with self.subTest(items=items), self.assertRaises(ValueError):
                artifact(encode(items))
        for output in ("not json", "{", "{}"):
            with self.subTest(output=output), self.assertRaises(ValueError):
                artifact(output)
        for level in ("warning", "error"):
            message = {"reason": "compiler-message", "message": {"level": level, "message": level}}
            with self.subTest(level=level), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(ValueError):
                artifact(encode([current, message, finished]))

    def test_gate_passes_only_selected_artifact_and_stops_on_build_failure(self):
        current = candidate()
        unrelated = dict(current, target={"name": "storage_engine", "kind": ["lib"]},
                         executable="/unrelated/libtest")
        output = encode([unrelated, current, {"reason": "build-finished", "success": True}])
        main = runpy.run_path(str(GATE))["main"]
        with mock.patch("subprocess.run", return_value=subprocess.CompletedProcess([], 0, stdout=output)) as run:
            main()
        self.assertEqual(run.call_count, 2)
        self.assertEqual(run.call_args_list[0].args[0], [
            "cargo", "test", "--locked", "--offline", "-p", "storage-server", "--lib",
            "--no-run", "--message-format=json",
        ])
        self.assertEqual(run.call_args_list[1].args[0], ["bash", RUNNER, current["executable"]])
        self.assertTrue(all(call.kwargs["check"] for call in run.call_args_list))
        failure = subprocess.CalledProcessError(1, ["cargo"], output="build failed")
        with mock.patch("subprocess.run", side_effect=failure) as run, \
                contextlib.redirect_stderr(io.StringIO()), self.assertRaises(subprocess.CalledProcessError):
            main()
        self.assertEqual(run.call_count, 1)

    def test_shell_requires_named_test_two_fault_hits_warning_and_one_pass(self):
        with tempfile.TemporaryDirectory(prefix="telemetry-enumeration-runner-") as temporary:
            root = Path(temporary)
            tools = root / "bin"
            tools.mkdir()
            # The real shell guards execute, but neither the compiler nor a
            # preload library runs. Actual EIO injection belongs to workspace CI.
            (tools / "cc").write_text("#!/bin/sh\nexit 0\n")
            (tools / "env").write_text(
                f"#!{sys.executable}\n"
                "import os, sys\n"
                "args = sys.argv[1:]\n"
                "while args and '=' in args[0]:\n"
                "    key, value = args.pop(0).split('=', 1)\n"
                "    if key != 'LD_PRELOAD': os.environ[key] = value\n"
                "os.execvpe(args[0], args, os.environ)\n"
            )
            for tool in tools.iterdir():
                tool.chmod(0o700)
            binary = root / "libtest"
            passed = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;"
            zero = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out;"
            failed = "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out;"
            for listed, result, hits, warning, exit_code, succeeds in (
                (False, passed, 2, True, 0, False),
                (True, zero, 2, True, 0, False),
                (True, failed, 2, True, 0, False),
                (True, passed, 0, True, 0, False),
                (True, passed, 1, True, 0, False),
                (True, passed, 3, True, 0, False),
                (True, passed, 2, False, 0, False),
                (True, passed, 2, True, 1, False),
                (True, passed, 2, True, 0, True),
            ):
                with self.subTest(listed=listed, result=result, hits=hits, warning=warning, exit_code=exit_code):
                    binary.write_text(
                        f"#!{sys.executable}\n"
                        "import sys\n"
                        "if '--list' in sys.argv:\n"
                        f"    print({(TEST_NAME + ': test') if listed else '0 tests'!r})\n"
                        "    sys.exit(0)\n"
                        f"for _ in range({hits}): print({MARKER!r})\n"
                        f"if {warning!r}: print({WARNING!r})\n"
                        f"print({result!r})\n"
                        f"sys.exit({exit_code})\n"
                    )
                    binary.chmod(0o700)
                    execution = subprocess.run(
                        ["bash", str(ROOT / RUNNER), str(binary)],
                        env=dict(os.environ, PATH=f"{tools}{os.pathsep}{os.environ['PATH']}"),
                        text=True, capture_output=True,
                    )
                    self.assertEqual(execution.returncode == 0, succeeds, execution.stdout + execution.stderr)
                    if succeeds:
                        self.assertEqual(execution.stdout.count(MARKER), 2)
                        self.assertIn(WARNING, execution.stdout)
                        self.assertIn(passed, execution.stdout)


if __name__ == "__main__":
    unittest.main()

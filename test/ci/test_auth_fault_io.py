"""Guard atomic I/O artifact selection and zero-test refusal without compiling."""
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
TEST_NAME = "auth::tests::credential_io_barriers_preserve_publication_order_and_failure_fencing"
artifact = runpy.run_path(str(ROOT / ".github/scripts/check-auth-fault-io.py"))["test_artifact"]


class AuthArtifactTests(unittest.TestCase):
    def test_exact_current_libtest_requires_successful_complete_build(self):
        candidate = {"reason": "compiler-artifact", "target": {"name": "storage_server", "kind": ["lib"]},
                     "profile": {"test": True}, "executable": "/current/auth-libtest"}
        finished = {"reason": "build-finished", "success": True}
        encode = lambda items: "\n".join(json.dumps(item) for item in items)
        self.assertEqual(artifact(encode([candidate, finished])), candidate["executable"])
        for items in ([], [candidate], [finished], [candidate, candidate, finished],
                      [candidate, finished, finished],
                      [candidate, {"reason": "build-finished", "success": False}],
                      [dict(candidate, profile={"test": False}), finished],
                      [dict(candidate, target={"name": "storage_server", "kind": ["bin"]}), finished],
                      [dict(candidate, target={"name": "storage_engine", "kind": ["lib"]}), finished]):
            with self.subTest(items=items), self.assertRaises(ValueError):
                artifact(encode(items))
        for output in ("not json", "{", "{}"):
            with self.subTest(output=output), self.assertRaises(ValueError):
                artifact(output)
        for level in ("warning", "error"):
            message = {"reason": "compiler-message", "message": {"level": level, "message": level}}
            with self.subTest(level=level), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(ValueError):
                artifact(encode([candidate, message, finished]))

    def test_gate_runs_normal_barriers_and_explicit_libc_rename_matrix(self):
        candidate = {"reason": "compiler-artifact", "target": {"name": "storage_server", "kind": ["lib"]},
                     "profile": {"test": True}, "executable": "/current/auth-libtest"}
        platform = dict(candidate, target={"name": "platform_io", "kind": ["lib"]}, executable="/current/platform-libtest")
        output = "\n".join(json.dumps(item) for item in (candidate, platform, {"reason": "build-finished", "success": True}))
        commands = []
        def run(command, **kwargs):
            commands.append(command)
            return subprocess.CompletedProcess(command, 0, stdout=output if command[0] == "cargo" else "")
        main = runpy.run_path(str(ROOT / ".github/scripts/check-auth-fault-io.py"))["main"]
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch("subprocess.run", side_effect=run):
            main()
        builds = [command for command in commands if command[0] == "cargo"]
        self.assertEqual(len(builds), 2)
        self.assertNotIn("--features", builds[0])
        self.assertEqual(builds[1][-2:], ["--features", "rustix/use-libc"])
        self.assertEqual([command[1:] for command in commands if command[0] == "bash"], [
            ["test/server/unit/auth/auth-io.sh", "/current/auth-libtest", "barriers"],
            ["test/server/unit/auth/auth-io.sh", "/current/auth-libtest", "rename"],
            ["test/platform/unit/fs_safety/atomic-io.sh", "/current/platform-libtest", "rename"],
        ])

    def test_runner_rejects_missing_test_zero_tests_and_missing_completion(self):
        with tempfile.TemporaryDirectory(prefix="auth-io-runner-") as temporary:
            root = Path(temporary)
            tools = root / "bin"
            tools.mkdir()
            # Exercise runner guards, not the interposer. No compiler or loader
            # injection runs in this test; actual barriers run in workspace CI.
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
            for listed, result, marker, succeeds in (
                (False, "", False, False),
                (2, "", False, False),
                (True, "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out;", True, False),
                (True, "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out;", True, False),
                (True, "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", False, False),
                (True, "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", True, True),
            ):
                with self.subTest(listed=listed, result=result, marker=marker):
                    binary.write_text(
                        f"#!{sys.executable}\n"
                        "import os, sys\n"
                        "if '--list' in sys.argv:\n"
                        f"    print({((TEST_NAME + ': test\n') * int(listed)) if listed else '0 tests'!r})\n"
                        "    sys.exit(0)\n"
                        f"if {marker!r}:\n"
                        "    print('ZXS_AUTH_IO_COMPLETE mode=' + os.environ['ZXS_AUTH_MODE'] + ' stage=' + os.environ['ZXS_ATOMIC_STAGE'])\n"
                        f"print({result!r})\n"
                    )
                    binary.chmod(0o700)
                    for suite in ("barriers", "rename"):
                        execution = subprocess.run(
                            ["bash", str(ROOT / "test/server/unit/auth/auth-io.sh"), str(binary), suite],
                            env=dict(os.environ, PATH=f"{tools}{os.pathsep}{os.environ['PATH']}"),
                            text=True, capture_output=True,
                        )
                        self.assertEqual(execution.returncode, 0 if succeeds else 1, execution.stderr)
                        if succeeds:
                            self.assertEqual(execution.stdout.count("Auth I/O mode="), 15 if suite == "barriers" else 6)
                        else:
                            self.assertIn("Missing exact ignored auth I/O test" if listed != 1 else
                                          "Missing auth I/O completion or single-test pass", execution.stderr)


if __name__ == "__main__":
    unittest.main()

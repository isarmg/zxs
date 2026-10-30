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

ROOT = Path(__file__).resolve().parents[2]
TEST_NAME = "fs_safety::tests::atomic_io_barriers_and_failures"
artifact = runpy.run_path(str(ROOT / ".github/scripts/check-platform-atomic-io.py"))["test_artifact"]


class AtomicArtifactTests(unittest.TestCase):
    def test_exact_current_libtest_requires_successful_complete_build(self):
        candidate = {"reason": "compiler-artifact", "target": {"name": "platform_io", "kind": ["lib"]},
                     "profile": {"test": True}, "executable": "/current/platform-libtest"}
        finished = {"reason": "build-finished", "success": True}
        encode = lambda items: "\n".join(json.dumps(item) for item in items)
        self.assertEqual(artifact(encode([candidate, finished])), candidate["executable"])
        for items in ([], [candidate], [finished], [candidate, candidate, finished],
                      [candidate, finished, finished],
                      [candidate, {"reason": "build-finished", "success": False}],
                      [dict(candidate, profile={"test": False}), finished],
                      [dict(candidate, target={"name": "platform_io", "kind": ["bin"]}), finished]):
            with self.subTest(items=items), self.assertRaises(ValueError):
                artifact(encode(items))
        for level in ("warning", "error"):
            message = {"reason": "compiler-message", "message": {"level": level, "message": level}}
            with self.subTest(level=level), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(ValueError):
                artifact(encode([candidate, message, finished]))

    def test_runner_rejects_missing_test_zero_tests_and_missing_completion(self):
        with tempfile.TemporaryDirectory(prefix="atomic-io-runner-") as temporary:
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
                (True, "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out;", True, False),
                (True, "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", False, False),
                (True, "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", True, True),
            ):
                with self.subTest(listed=listed, result=result, marker=marker):
                    binary.write_text(
                        f"#!{sys.executable}\n"
                        "import os, sys\n"
                        "if '--list' in sys.argv:\n"
                        f"    print({(TEST_NAME + ': test') if listed else '0 tests'!r})\n"
                        "    sys.exit(0)\n"
                        f"if {marker!r}:\n"
                        "    print('ZXS_ATOMIC_IO_COMPLETE mode=' + os.environ['ZXS_ATOMIC_MODE'] + ' stage=' + os.environ['ZXS_ATOMIC_STAGE'])\n"
                        f"print({result!r})\n"
                    )
                    binary.chmod(0o700)
                    execution = subprocess.run(
                        ["bash", str(ROOT / "test/platform/unit/fs_safety/atomic-io.sh"), str(binary)],
                        env=dict(os.environ, PATH=f"{tools}{os.pathsep}{os.environ['PATH']}"),
                        text=True, capture_output=True,
                    )
                    self.assertEqual(execution.returncode, 0 if succeeds else 1, execution.stderr)
                    if succeeds:
                        expected = [
                            f"Atomic I/O mode={mode} stage={stage}"
                            for mode in ("create", "replace", "replace-held", "public", "recovery-intent")
                            for stage in ("success", "write", "file-sync", "publish", "directory-sync")
                            if (mode, stage) != ("public", "write")
                        ]
                        self.assertEqual(
                            [line for line in execution.stdout.splitlines()
                             if line.startswith("Atomic I/O mode=")],
                            expected,
                        )
                    else:
                        self.assertIn("Missing exact ignored atomic I/O test" if not listed else
                                      "Missing atomic I/O completion or single-test pass", execution.stderr)


if __name__ == "__main__":
    unittest.main()

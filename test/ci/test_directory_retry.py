"""Guard directory retry artifact selection and zero-test refusal without compiling."""
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
TEST_NAME = "fs_safety::tests::directory_creation_retry_barriers"
artifact = runpy.run_path(str(ROOT / ".github/scripts/check-directory-retry.py"))["test_artifacts"]


class DirectoryArtifactTests(unittest.TestCase):
    def test_exact_current_libtest_requires_successful_complete_build(self):
        candidate = {"reason": "compiler-artifact", "target": {"name": "platform_io", "kind": ["lib"]},
                     "profile": {"test": True}, "executable": "/current/platform-libtest"}
        finished = {"reason": "build-finished", "success": True}
        encode = lambda items: "\n".join(json.dumps(item) for item in items)
        engine = dict(candidate, target={"name": "storage_engine", "kind": ["lib"]}, executable="/current/engine-libtest")
        self.assertEqual(artifact(encode([candidate, engine, finished])),
                         {"platform": candidate["executable"], "engine": engine["executable"]})
        for items in ([], [candidate, engine], [candidate, finished], [engine, finished],
                      [candidate, candidate, engine, finished], [candidate, engine, finished, finished],
                      [candidate, engine, {"reason": "build-finished", "success": False}],
                      [dict(candidate, profile={"test": False}), engine, finished],
                      [dict(candidate, target={"name": "platform_io", "kind": ["bin"]}), engine, finished]):
            with self.subTest(items=items), self.assertRaises(ValueError):
                artifact(encode(items))
        for level in ("warning", "error"):
            message = {"reason": "compiler-message", "message": {"level": level, "message": level}}
            with self.subTest(level=level), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(ValueError):
                artifact(encode([candidate, engine, message, finished]))

    def test_runner_rejects_missing_test_zero_tests_and_missing_completion(self):
        with tempfile.TemporaryDirectory(prefix="directory-retry-runner-") as temporary:
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
                        "open(os.environ['ZXS_DIRECTORY_LOG'], 'w').close()\n"
                        f"if {marker!r}:\n"
                        "    print('ZXS_DIRECTORY_RETRY_COMPLETE mode=' + os.environ['ZXS_DIRECTORY_MODE'] + ' stage=' + os.environ['ZXS_DIRECTORY_STAGE'])\n"
                        f"print({result!r})\n"
                    )
                    binary.chmod(0o700)
                    execution = subprocess.run(
                        ["bash", str(ROOT / "test/platform/unit/fs_safety/directory-retry.sh"), str(binary), "platform"],
                        env=dict(os.environ, PATH=f"{tools}{os.pathsep}{os.environ['PATH']}"),
                        text=True, capture_output=True,
                    )
                    self.assertEqual(execution.returncode, 0 if succeeds else 1, execution.stderr)
                    if succeeds:
                        self.assertEqual(execution.stdout.count("ZXS_DIRECTORY_RETRY_COMPLETE"), 6)


if __name__ == "__main__":
    unittest.main()

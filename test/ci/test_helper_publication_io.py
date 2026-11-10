"""Artifact and zero-execution guards; these do not claim real syscall coverage."""
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
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
TEST_NAME = "private_io::tests::publication_faults_preserve_phase_identity_and_evidence"
GATE = runpy.run_path(str(ROOT / ".github/scripts/check-helper-publication-io.py"))
artifacts = GATE["test_artifacts"]


class HelperPublicationArtifactTests(unittest.TestCase):
    def test_requires_both_current_artifacts_and_one_successful_build(self):
        test = {"reason": "compiler-artifact", "target": {"name": "zxs-helper", "kind": ["bin"]},
                "profile": {"test": True}, "executable": "/current/helper-test"}
        cli = dict(test, profile={"test": False}, executable="/current/zxs-helper")
        finished = {"reason": "build-finished", "success": True}
        encode = lambda items: "\n".join(json.dumps(item) for item in items)
        self.assertEqual(artifacts(encode([test, cli, finished])), (test["executable"], cli["executable"]))
        for items in ([], [test, cli], [test, finished], [cli, finished],
                      [test, test, cli, finished], [test, cli, cli, finished],
                      [test, cli, finished, finished],
                      [test, cli, dict(finished, success=False)],
                      [dict(test, target={"name": "zxs-helper", "kind": ["lib"]}), cli, finished],
                      [dict(test, executable=None), cli, finished]):
            with self.subTest(items=items), self.assertRaises(ValueError):
                artifacts(encode(items))
        for level in ("warning", "error"):
            message = {"reason": "compiler-message", "message": {"level": level, "message": level}}
            with self.subTest(level=level), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(ValueError):
                artifacts(encode([test, cli, message, finished]))

    def test_build_is_unprivileged_and_shell_gate_enters_private_namespace(self):
        artifacts_output = "\n".join(json.dumps(item) for item in [
            {"reason": "compiler-artifact", "target": {"name": "zxs-helper", "kind": ["bin"]},
             "profile": {"test": True}, "executable": "/repo/helper-test"},
            {"reason": "compiler-artifact", "target": {"name": "zxs-helper", "kind": ["bin"]},
             "profile": {"test": False}, "executable": "/repo/zxs-helper"},
            {"reason": "build-finished", "success": True},
        ])
        with patch.object(runpy, "run_path", return_value={"matrix_environment": lambda: {"BUILD_ONLY": "1"}}), \
             patch.object(subprocess, "run", side_effect=[
                 subprocess.CompletedProcess([], 0, artifacts_output),
                 subprocess.CompletedProcess([], 0),
             ]) as run:
            GATE["main"]()
        self.assertEqual(run.call_args_list[0].args[0][:2], ["cargo", "test"])
        self.assertEqual(run.call_args_list[1].args[0], [
            sys.executable, str(ROOT / "test/server/service_namespace.py"), "--command",
            "/usr/bin/bash", str(ROOT / "test/helper/unit/private_io/publication-io.sh"),
            "/repo/helper-test", "/repo/zxs-helper"])
        self.assertTrue(run.call_args_list[1].kwargs["check"])

    def test_runner_rejects_missing_test_zero_tests_and_missing_completion(self):
        with tempfile.TemporaryDirectory(prefix="helper-publication-runner-") as temporary:
            root = Path(temporary)
            tools = root / "bin"
            tools.mkdir()
            (tools / "cc").write_text("#!/bin/sh\nexit 0\n")
            (tools / "env").write_text(
                f"#!{sys.executable}\nimport os, sys\nargs = sys.argv[1:]\n"
                "while args and '=' in args[0]:\n"
                "    key, value = args.pop(0).split('=', 1)\n"
                "    if key != 'LD_PRELOAD': os.environ[key] = value\n"
                "os.execvpe(args[0], args, os.environ)\n")
            for tool in tools.iterdir():
                tool.chmod(0o700)
            binary = root / "libtest"
            for listed, result, marker in (
                (False, "", False),
                (True, "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out;", True),
                (True, "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", False),
            ):
                with self.subTest(listed=listed, result=result, marker=marker):
                    binary.write_text(
                        f"#!{sys.executable}\nimport os, sys\n"
                        "if '--list' in sys.argv:\n"
                        f"    print({(TEST_NAME + ': test') if listed else '0 tests'!r})\n"
                        "    sys.exit(0)\n"
                        f"if {marker!r}:\n"
                        "    print('ZXS_RESCUE_PUBLICATION_COMPLETE mode=' + os.environ['ZXS_ATOMIC_MODE'] + ' stage=' + os.environ['ZXS_ATOMIC_STAGE'])\n"
                        f"print({result!r})\n")
                    binary.chmod(0o700)
                    execution = subprocess.run(
                        ["bash", str(ROOT / "test/helper/unit/private_io/publication-io.sh"), str(binary), str(binary)],
                        env=dict(os.environ, PATH=f"{tools}{os.pathsep}{os.environ['PATH']}"),
                        text=True, capture_output=True)
                    self.assertEqual(execution.returncode, 1, execution.stderr)


if __name__ == "__main__":
    unittest.main()

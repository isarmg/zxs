"""Current-artifact and ineffective-test guards, not real fsync evidence."""
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
RUNNER = runpy.run_path(str(ROOT / ".github/scripts/check-independent-disk-publication-io.py"))
TEST = "disk_authority::journal_tests::published_replay_repeats_the_real_file_and_directory_barriers"
MARKER = "ZXS_INDEPENDENT_DURABILITY_REPLAY_COMPLETE"
REPLAY_TEST = "disk_authority::journal_tests::published_replay_rejects_same_inode_changes_and_lost_final_chain"
REPLAY_MARKER = "ZXS_INDEPENDENT_REPLAY_MUTATION_REJECTED"
SUCCESS = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.01s"
ARTIFACT = {"reason": "compiler-artifact", "target": {"name": "storage_engine", "kind": ["lib"]},
            "profile": {"test": True}, "executable": "/current/storage_engine"}
FINISHED = {"reason": "build-finished", "success": True}


def encode(items):
    return "\n".join(map(json.dumps, items))


def stub_environment(root):
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
    return dict(os.environ, PATH=f"{tools}{os.pathsep}{os.environ['PATH']}")


class IndependentDiskPublicationTests(unittest.TestCase):
    def test_only_one_successful_current_engine_libtest_is_accepted(self):
        select = RUNNER["test_artifact"]
        self.assertEqual(select(encode([ARTIFACT, FINISHED])), ARTIFACT["executable"])
        bad = [[], [ARTIFACT], [FINISHED], [ARTIFACT, ARTIFACT, FINISHED],
               [ARTIFACT, FINISHED, FINISHED], [ARTIFACT, dict(FINISHED, success=False)]]
        for field, value in (("profile", {"test": False}), ("executable", None),
                             ("target", {"name": "storage_engine", "kind": ["bin"]}),
                             ("target", {"name": "another_library", "kind": ["lib"]})):
            bad.append([dict(ARTIFACT, **{field: value}), FINISHED])
        for items in bad:
            with self.subTest(items=items), self.assertRaises(ValueError):
                select(encode(items))
        for level in ("warning", "error"):
            message = {"reason": "compiler-message", "message": {"level": level, "message": level}}
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(ValueError):
                select(encode([ARTIFACT, message, FINISHED]))

    def test_gate_uses_the_current_artifact_and_propagates_build_and_driver_failure(self):
        result = subprocess.CompletedProcess([], 0, encode([ARTIFACT, FINISHED]))
        with patch("subprocess.run", side_effect=[result, subprocess.CompletedProcess([], 0)]) as run:
            RUNNER["main"]()
        self.assertEqual(run.call_args_list[0].args[0], [
            "cargo", "test", "--locked", "--offline", "-p", "storage-engine", "--lib",
            "--no-run", "--message-format=json"])
        self.assertEqual(run.call_args_list[1].args[0], [
            "bash", "test/engine/unit/disk_authority/publication-io.sh", "/current/storage_engine"])
        for call in run.call_args_list:
            self.assertTrue(call.kwargs["check"])
            self.assertEqual(call.kwargs["cwd"], ROOT)
        for outcomes, calls in (([subprocess.CalledProcessError(1, "cargo")], 1),
                                ([result, subprocess.CalledProcessError(1, "driver")], 2)):
            with patch("subprocess.run", side_effect=outcomes) as failed:
                with self.assertRaises(subprocess.CalledProcessError):
                    RUNNER["main"]()
                self.assertEqual(failed.call_count, calls)

    def test_driver_rejects_zero_execution_missing_or_duplicate_completion_and_failed_child(self):
        success = SUCCESS
        cases = [
            (TEST + ": test", MARKER + "\n" + success, 0, True),
            ("0 tests", MARKER + "\n" + success, 0, False),
            (TEST + ": test", MARKER + "\n" + success.replace("1 passed", "0 passed"), 0, False),
            (TEST + ": test", success, 0, False),
            (TEST + ": test", MARKER + "\n" + MARKER + "\n" + success, 0, False),
            (TEST + ": test", MARKER + "\n" + success + "\n" + success, 0, False),
            (TEST + ": test", MARKER + "\n" + success.replace("0 ignored", "1 ignored"), 0, False),
            (TEST + ": test", MARKER + "\n" + success, 1, False),
        ]
        with tempfile.TemporaryDirectory(prefix="independent-publication-runner-") as temporary:
            root = Path(temporary)
            environment = stub_environment(root)
            binary = root / "libtest"
            for listing, output, code, passes in cases:
                with self.subTest(listing=listing, output=output, code=code):
                    binary.write_text(
                        f"#!{sys.executable}\nimport sys\n"
                        f"if {REPLAY_TEST!r} in sys.argv:\n"
                        f"    print({(REPLAY_TEST + ': test')!r} if '--list' in sys.argv else "
                        f"{(REPLAY_MARKER + chr(10) + success)!r})\n    sys.exit(0)\n"
                        f"if '--list' in sys.argv:\n    print({listing!r})\n    sys.exit(0)\n"
                        f"print({output!r})\nsys.exit({code})\n")
                    binary.chmod(0o700)
                    result = subprocess.run(
                        ["bash", str(ROOT / "test/engine/unit/disk_authority/publication-io.sh"), str(binary)],
                        env=environment,
                        text=True, capture_output=True)
                    self.assertEqual(result.returncode == 0, passes, result.stdout + result.stderr)

    def test_replay_driver_requires_one_exact_execution_for_all_ten_mutations(self):
        listing = REPLAY_TEST + ": test"
        output = REPLAY_MARKER + "\n" + SUCCESS
        cases = [
            (listing, output, 0, None, True, 10),
            ("0 tests", output, 0, None, False, 0),
            (listing + "\n" + listing, output, 0, None, False, 0),
            (listing, output.replace("1 passed", "0 passed"), 0, None, False, 1),
            (listing, SUCCESS, 0, None, False, 1),
            (listing, REPLAY_MARKER + "\n" + output, 0, None, False, 1),
            (listing, output + "\n" + SUCCESS, 0, None, False, 1),
            (listing, output.replace("0 ignored", "1 ignored"), 0, None, False, 1),
            (listing, output, 1, None, False, 1),
            (listing, output, 0, ["resume", "rollback"], False, 10),
        ]
        with tempfile.TemporaryDirectory(prefix="independent-replay-runner-") as temporary:
            root = Path(temporary)
            environment = stub_environment(root)
            binary, calls = root / "libtest", root / "calls"
            for listing, output, code, failed_case, passes, count in cases:
                with self.subTest(listing=listing, output=output, code=code, failed_case=failed_case):
                    calls.write_text("")
                    binary.write_text(
                        f"#!{sys.executable}\nimport json, os, sys\n"
                        f"if '--list' in sys.argv:\n    print({listing!r})\n    sys.exit(0)\n"
                        "case = [os.environ['ZXS_AUTHORITY_REPLAY_METHOD'], "
                        "os.environ['ZXS_AUTHORITY_MUTATION_KIND']]\n"
                        f"with open({str(calls)!r}, 'a') as stream: stream.write(json.dumps(case) + '\\n')\n"
                        f"print({output!r})\nsys.exit(1 if case == {failed_case!r} else {code})\n")
                    binary.chmod(0o700)
                    result = subprocess.run(
                        ["bash", str(ROOT / "test/engine/unit/disk_authority/replay-mutation-io.sh"), str(binary)],
                        env=environment, text=True, capture_output=True)
                    self.assertEqual(result.returncode == 0, passes, result.stdout + result.stderr)
                    observed = [json.loads(line) for line in calls.read_text().splitlines()]
                    expected = [[method, kind] for method in ("execute", "resume")
                                for kind in ("decision", "manifest", "body", "missing-body", "rollback")]
                    self.assertEqual(observed, expected[:count])

    def test_original_driver_propagates_replay_failure(self):
        with tempfile.TemporaryDirectory(prefix="independent-replay-propagation-") as temporary:
            root = Path(temporary)
            environment = stub_environment(root)
            binary = root / "libtest"
            binary.write_text(
                f"#!{sys.executable}\nimport sys\n"
                f"replay = {REPLAY_TEST!r} in sys.argv\n"
                f"if '--list' in sys.argv:\n    print(({REPLAY_TEST!r} if replay else {TEST!r}) + ': test')\n    sys.exit(0)\n"
                f"print(({REPLAY_MARKER!r} if replay else {MARKER!r}) + '\\n' + {SUCCESS!r})\n"
                "sys.exit(1 if replay else 0)\n")
            binary.chmod(0o700)
            result = subprocess.run(
                ["bash", str(ROOT / "test/engine/unit/disk_authority/publication-io.sh"), str(binary)],
                env=environment, text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()

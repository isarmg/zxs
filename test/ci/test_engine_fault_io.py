"""The gate must reject empty/stale builds and successful but ineffective tests."""
import contextlib
import io
import json
from pathlib import Path
import runpy
import subprocess
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
RUNNER = runpy.run_path(str(ROOT / ".github/scripts/check-engine-fault-io.py"))


class EngineFaultTests(unittest.TestCase):
    def test_artifacts_require_both_current_targets_and_complete_success(self):
        candidates = [
            {"reason": "compiler-artifact", "target": {"name": name, "kind": kind},
             "profile": {"test": True}, "executable": f"/current/{name}"}
            for name, kind in RUNNER["TARGETS"].items()
        ]
        finished = {"reason": "build-finished", "success": True}
        encode = lambda items: "\n".join(map(json.dumps, items))
        select = RUNNER["test_artifacts"]
        self.assertEqual(select(encode([*candidates, finished])),
                         {item["target"]["name"]: item["executable"] for item in candidates})
        failures = [[], candidates, [finished], [candidates[0], finished],
                    [candidates[1], finished], [*candidates, candidates[0], finished],
                    [*candidates, finished, finished],
                    [*candidates, dict(finished, success=False)]]
        for field, value in (("profile", {"test": False}), ("executable", None),
                             ("target", {"name": "storage_engine", "kind": ["bin"]})):
            failures.append([dict(candidates[0], **{field: value}), candidates[1], finished])
        for items in failures:
            with self.subTest(items=items), self.assertRaises(ValueError):
                select(encode(items))
        for level in ("warning", "error"):
            message = {"reason": "compiler-message", "message": {"level": level, "message": level}}
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(ValueError):
                select(encode([*candidates, message, finished]))

    def test_exact_runner_requires_listing_injection_marker_and_one_pass(self):
        name = RUNNER["READ_TEST"]
        marker = "ZXS_WORKSPACE_READ_COMPLETE errno=5"
        success = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 31 filtered out; finished in 0.01s\n"
        hits = ["ZXS_TEST_WORKSPACE_READ errno=5 /tmp/workspaces/x/commit-request.read-error",
                "ZXS_TEST_WORKSPACE_READ errno=5 /tmp/workspaces/x/workspace.read-error"]
        patterns = tuple(f"^{line}$" for line in hits)
        complete = "\n".join([*hits, marker, success])
        for listed, output, passes in (
            (name + ": test\n", complete, True),
            ("0 tests", complete, False),
            (name + ": test\n", complete.replace("1 passed", "0 passed"), False),
            (name + ": test\n", complete.replace(marker, ""), False),
            (name + ": test\n", complete.replace(hits[0], ""), False),
            (name + ": test\n", complete.replace(hits[1], ""), False),
            (name + ": test\n", complete.replace("errno=5", "errno=13"), False),
            (name + ": test\n", complete + marker + "\n", False),
        ):
            with self.subTest(listed=listed, output=output), patch("subprocess.run") as run:
                run.side_effect = [subprocess.CompletedProcess([], 0, listed),
                                   subprocess.CompletedProcess([], 0, output)]
                with contextlib.redirect_stdout(io.StringIO()):
                    if passes:
                        RUNNER["run_exact"]("/current/test", name, marker, {}, patterns)
                        self.assertEqual(run.call_args.args[0], ["/current/test", name, "--ignored", "--exact", "--nocapture", "--test-threads=1"])
                    else:
                        with self.assertRaises(ValueError):
                            RUNNER["run_exact"]("/current/test", name, marker, {}, patterns)
        with patch("subprocess.run", side_effect=subprocess.CalledProcessError(1, "libtest")):
            with self.assertRaises(subprocess.CalledProcessError):
                RUNNER["run_exact"]("/current/test", name, marker, {}, patterns)

    def test_gate_schedules_every_fault_and_cleans_inherited_injection(self):
        main = RUNNER["main"]
        artifacts = {"storage_engine": "/current/lib", "workspace_commit_replay": "/current/replay"}
        with patch("os.geteuid", return_value=1000), patch("subprocess.run") as run, \
                patch.dict("os.environ", LD_PRELOAD="stale.so", ZXS_TEST_WORKSPACE_ERRNO="99"), \
                patch.dict(main.__globals__, test_artifacts=lambda output: artifacts):
            exact = Mock()
            with patch.dict(main.__globals__, run_exact=exact):
                main()
            build = run.call_args_list[0].args[0]
            self.assertEqual(build, ["cargo", "test", "--locked", "--offline", "-p", "storage-engine",
                                     "--lib", "--test", "workspace_commit_replay", "--no-run", "--message-format=json"])
            self.assertEqual(run.call_count, 3)  # current build and both real C fixtures
            calls = exact.call_args_list
            self.assertEqual([call.args[1] for call in calls],
                             [RUNNER["READ_TEST"], RUNNER["READ_TEST"], RUNNER["SYNC_TEST"], RUNNER["PERMISSION_TEST"]])
            self.assertEqual([call.args[3]["ZXS_TEST_WORKSPACE_ERRNO"] for call in calls[:2]], ["5", "13"])
            self.assertEqual([len(call.args[4]) for call in calls[:3]], [2, 2, 1])
            self.assertNotIn("LD_PRELOAD", calls[3].args[3])
            self.assertNotIn("ZXS_TEST_WORKSPACE_ERRNO", calls[3].args[3])
            self.assertNotIn("ZXS_TEST_WORKSPACE_ERRNO", calls[2].args[3])

    def test_failed_child_cannot_be_hidden_by_later_success(self):
        main = RUNNER["main"]
        artifacts = {"storage_engine": "/current/lib", "workspace_commit_replay": "/current/replay"}
        exact = Mock(side_effect=ValueError("missing injection"))
        with patch("os.geteuid", return_value=1000), patch("subprocess.run"), \
                patch.dict(main.__globals__, test_artifacts=lambda output: artifacts, run_exact=exact):
            with self.assertRaisesRegex(ValueError, "missing injection"):
                main()
        exact.assert_called_once()
        with patch("os.geteuid", return_value=1000), \
                patch("subprocess.run", side_effect=subprocess.CalledProcessError(1, "cargo")):
            with self.assertRaises(subprocess.CalledProcessError):
                main()

    def test_root_is_a_failure_not_a_skip(self):
        with patch("os.geteuid", return_value=0), patch("subprocess.run") as run:
            with self.assertRaisesRegex(ValueError, "requires a non-root runner"):
                RUNNER["main"]()
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()

"""Entropy fault execution must not mistake stale/missing artifacts for coverage."""
import json
import os
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
MODULE = runpy.run_path(str(ROOT / ".github/scripts/check-entropy-fault-io.py"))


class EntropyArtifactTests(unittest.TestCase):
    def test_requires_all_current_test_artifacts_and_successful_complete_build(self):
        artifacts = [{"reason": "compiler-artifact", "target": {"name": name, "kind": ["lib"]},
                      "profile": {"test": True}, "executable": f"/current/{name}"}
                     for name in MODULE["TESTS"]]
        finished = {"reason": "build-finished", "success": True}
        encode = lambda items: "\n".join(json.dumps(item) for item in items)
        self.assertEqual(len(MODULE["test_artifacts"](encode(artifacts + [finished]))), 3)
        for items in ([], artifacts, artifacts[1:] + [finished], artifacts + artifacts + [finished],
                      artifacts + [finished, finished],
                      artifacts + [{"reason": "build-finished", "success": False}],
                      artifacts + [{"reason": "compiler-message", "message": {"level": "warning", "message": "warning"}}, finished]):
            with self.subTest(items=items), self.assertRaises(ValueError):
                MODULE["test_artifacts"](encode(items))

    def test_shell_rejects_zero_tests_even_if_exit_is_success(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "empty-libtest"
            binary.write_text('#!/bin/sh\nprintf "0 tests, 0 benchmarks\\n"\n')
            binary.chmod(0o700)
            result = subprocess.run(["bash", str(ROOT / "test/contract/entropy/entropy-io.sh"),
                                     str(binary), "tests::entropy_fault"], text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Missing exact ignored entropy test", result.stderr)

    def test_shell_requires_completion_and_exactly_one_passing_test(self):
        cases = [
            ("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", False),
            ("ZXS_ENTROPY_FAULT_COMPLETE\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", False),
            ("ZXS_ENTROPY_FAULT_COMPLETE\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", True),
        ]
        for output, success in cases:
            with self.subTest(output=output), tempfile.TemporaryDirectory() as temporary:
                binary = Path(temporary) / "fake-libtest"
                binary.write_text('#!/bin/sh\nif [ "$1" = "--list" ]; then echo "tests::entropy_fault: test"; else cat <<\'RESULT\'\n' + output + '\nRESULT\nfi\n')
                binary.chmod(0o700)
                environment = dict(os.environ, TMPDIR=temporary)
                result = subprocess.run(["bash", str(ROOT / "test/contract/entropy/entropy-io.sh"),
                                         str(binary), "tests::entropy_fault"], env=environment,
                                        text=True, capture_output=True)
                self.assertEqual(result.returncode == 0, success, result.stderr)


if __name__ == "__main__":
    unittest.main()

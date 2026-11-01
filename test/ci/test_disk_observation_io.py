"""Mechanical fault-gate checks; no compiler, ptrace, or permissions are used."""
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
GATE = runpy.run_path(str(ROOT / ".github/scripts/check-disk-observation-io.py"))
RUNNER = ROOT / GATE["RUNNER"]


def candidate():
    return {"reason": "compiler-artifact", "target": {"name": "storage_engine", "kind": ["lib"]},
            "profile": {"test": True}, "executable": "/current/libtest"}


def encode(items):
    return "\n".join(map(json.dumps, items))


class DiskObservationIoTests(unittest.TestCase):
    def test_only_one_successful_current_libtest_is_accepted(self):
        current = candidate()
        finished = {"reason": "build-finished", "success": True}
        self.assertEqual(GATE["test_artifact"](encode([current, finished])), "/current/libtest")
        for records in [[], [current], [finished], [current, current, finished],
                        [current, finished, finished], [current, dict(finished, success=False)],
                        [dict(current, profile={"test": False}), finished],
                        [dict(current, executable=None), finished],
                        [dict(current, target={"name": "storage_engine", "kind": ["bin"]}), finished],
                        [dict(current, target={"name": "storage_server", "kind": ["lib"]}), finished]]:
            with self.subTest(records=records), self.assertRaises(ValueError):
                GATE["test_artifact"](encode(records))
        for level in ["warning", "error"]:
            message = {"reason": "compiler-message", "message": {"level": level, "message": level}}
            with self.assertRaises(ValueError), contextlib.redirect_stderr(io.StringIO()):
                GATE["test_artifact"](encode([current, message, finished]))
        with self.assertRaises(ValueError):
            GATE["test_artifact"]("not JSON")

    def test_gate_selects_cargo_artifact_and_stops_on_build_failure(self):
        output = encode([candidate(), {"reason": "build-finished", "success": True}])
        with mock.patch.dict(os.environ, {"LD_PRELOAD": "/unrelated.so"}), \
                mock.patch("subprocess.run", return_value=subprocess.CompletedProcess([], 0, stdout=output)) as run:
            GATE["main"]()
        self.assertEqual(run.call_count, 2)
        self.assertEqual(run.call_args_list[0].args[0], [
            "cargo", "test", "--locked", "--offline", "-p", "storage-engine", "--lib",
            "--no-run", "--message-format=json"])
        self.assertEqual(run.call_args_list[1].args[0][:3], ["bash", GATE["RUNNER"], "/current/libtest"])
        for call in run.call_args_list:
            self.assertTrue(call.kwargs["check"])
            self.assertNotIn("LD_PRELOAD", call.kwargs["env"])
        failure = subprocess.CalledProcessError(1, ["cargo"], output="failed")
        with mock.patch("subprocess.run", side_effect=failure) as run, \
                contextlib.redirect_stderr(io.StringIO()), self.assertRaises(subprocess.CalledProcessError):
            GATE["main"]()
        self.assertEqual(run.call_count, 1)

    def test_shell_requires_named_tests_actual_fault_hits_and_one_pass(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            tools = root / "bin"
            tools.mkdir()
            scripts = {
                "cc": "import sys\nsys.exit(0)\n",
                "env": "import os,sys\na=sys.argv[1:]\nwhile a and '=' in a[0]: a.pop(0)\nos.execvpe(a[0],a,os.environ)\n",
                "strace": "import os,sys,subprocess\na=sys.argv[1:];i=a.index('-o');p=a[i+1]\nopen(p,'w').write('fstatfs(3, 0) = -1 EIO (INJECTED)\\n'*int(os.environ['HITS']))\nsys.exit(subprocess.run(a[i+2:]).returncode if os.environ['TRACE_OK']=='1' else 1)\n",
            }
            for name, body in scripts.items():
                path = tools / name
                path.write_text(f"#!{sys.executable}\n" + body)
                path.chmod(0o700)
            binary = root / "libtest"
            binary.write_text(f"#!{sys.executable}\n" + """import os,sys
name=sys.argv[sys.argv.index('--exact')+1]
if '--list' in sys.argv:
    for _ in range(int(os.environ['LISTED'])): print(name+': test')
    sys.exit(0)
if 'seal_permission' in name:
    for _ in range(int(os.environ['SEAL_HITS'])): print('ZXS_OBSERVATION_SEAL_EACCES')
print('test result: ok. '+os.environ['PASSED']+' passed; 0 failed; 0 ignored; 0 measured; 5 filtered out;')
sys.exit(int(os.environ['TEST_EXIT']))
""")
            binary.chmod(0o700)
            normal = dict(LISTED="1", SEAL_HITS="1", HITS="2", PASSED="1", TRACE_OK="1", TEST_EXIT="0")
            variants = [{}, {"LISTED": "0"}, {"LISTED": "2"}, {"SEAL_HITS": "0"}, {"SEAL_HITS": "2"},
                        {"HITS": "0"}, {"HITS": "1"}, {"HITS": "3"}, {"PASSED": "0"},
                        {"PASSED": "2"}, {"TRACE_OK": "0"}, {"TEST_EXIT": "1"}]
            for changed in variants:
                with self.subTest(changed=changed):
                    env = dict(os.environ, **normal)
                    env.update(changed)
                    env["PATH"] = f"{tools}{os.pathsep}{os.environ['PATH']}"
                    result = subprocess.run(["bash", str(RUNNER), str(binary), str(root / 'output')],
                                            env=env, text=True, capture_output=True)
                    self.assertEqual(result.returncode == 0, not changed, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()

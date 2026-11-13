"""Run the independent demo's actual state transitions, not source-string assertions."""
import pathlib
import subprocess
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


class ZxsDemoTests(unittest.TestCase):
    def test_demo_model_behaviors(self):
        result = subprocess.run(
            ['node', '--test', 'demo/tests/model.cjs'],
            cwd=ROOT, text=True, capture_output=True, timeout=90,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

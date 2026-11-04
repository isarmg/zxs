"""Static probe regressions; real cancellation acceptance remains in Chrome CI."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class MonitorCancellationProbe(unittest.TestCase):
    def test_host_regressions(self):
        subprocess.run(['node', 'test/tools/browser-smoke/unit/monitor_cancellation_probe_test.mjs'],
                       cwd=ROOT, check=True)

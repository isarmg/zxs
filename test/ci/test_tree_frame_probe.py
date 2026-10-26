"""Static-suite coverage of the browser probe itself, not browser evidence."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / 'test/tools/browser-smoke'


class TreeFrameProbe(unittest.TestCase):
    def test_javascript_parses(self):
        for name in ('tree_frames.js', 'tree_geometry.js'):
            with self.subTest(name=name):
                subprocess.run(['node', '--check', str(PROBE / 'runner' / name)],
                               check=True, cwd=ROOT)

    def test_host_instrumentation(self):
        subprocess.run(['node', str(PROBE / 'unit/tree_frame_probe_test.mjs')],
                       check=True, cwd=ROOT)

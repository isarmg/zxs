"""Exercise real-response observer behavior; the browser suite owns navigation."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / 'test/tools/browser-smoke'


class PrimaryBrowseProbe(unittest.TestCase):
    def test_observer_keeps_original_responses_errors_bounds_and_cleanup(self):
        subprocess.run(['node', str(PROBE / 'unit/primary_browse_probe_test.mjs')],
                       cwd=ROOT, check=True)


if __name__ == '__main__':
    unittest.main()

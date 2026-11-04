"""Static-suite tests for the probe, not real browser acceptance."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class AuthLayoutProbe(unittest.TestCase):
    def test_probe_and_host_regressions(self):
        subprocess.run(['node', '--check', 'test/tools/browser-smoke/runner/auth_layout.js'],
                       cwd=ROOT, check=True)
        subprocess.run(['node', 'test/tools/browser-smoke/unit/auth_layout_probe_test.mjs'],
                       cwd=ROOT, check=True)

    def test_failure_retains_real_screenshot_before_assertion(self):
        source = (ROOT / 'test/tools/browser-smoke/runner/auth_layout.rs').read_text()
        self.assertLess(source.index('dir.join(&name)'), source.index('if geometry["valid"] != true'))
        self.assertLess(source.index('dir.join("login-layout.json")'), source.index('if geometry["valid"] != true'))
        self.assertIn('return Err(', source[source.index('if geometry["valid"] != true'):])

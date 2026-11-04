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

    def test_root_layout_has_no_fixed_width_floor_above_the_content_viewport(self):
        styles = (ROOT / 'src/web/styles.css').read_text()
        reset = (ROOT / 'src/web/public/styles/tokens/reset.css').read_text()
        self.assertIn('html { min-width: 0;', styles)
        scope = reset.split('[data-zxs-scope] {', 1)[1].split('}', 1)[0]
        self.assertIn('min-width: 0;', scope)
        self.assertNotIn('min-width: 320px', styles + reset)

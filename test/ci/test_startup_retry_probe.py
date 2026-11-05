"""Focused startup-button contracts, not a production browser acceptance run."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class StartupRetryProbe(unittest.TestCase):
    def test_surface_and_keyboard_observer_cases(self):
        subprocess.run(['node', 'test/tools/browser-smoke/unit/startup_retry_probe_test.mjs'],
                       cwd=ROOT, check=True)

    def test_failure_control_is_scoped_without_an_ordinary_frame(self):
        css = (ROOT / 'src/web/public/styles/startup.css').read_text()
        button = css.split('#zxs-startup-error button {', 1)[1].split('}', 1)[0]
        for declaration in ['appearance: none;', 'border: 0;', 'border-radius: .5rem;',
                            'background: ButtonFace;', 'color: ButtonText;']:
            self.assertIn(declaration, button)
        self.assertIn('#zxs-startup-error button:focus-visible { outline: 2px solid currentColor;', css)
        self.assertIn('#zxs-startup-error button:hover { background:', css)

    def test_production_transport_failures_exercise_pointer_and_native_tab(self):
        source = (ROOT / 'test/tools/browser-smoke/runner/stylesheets.rs').read_text()
        failure = source.split('fn blocked_resource(', 1)[1].split('fn startup_retry_style(', 1)[0]
        self.assertIn('startup_retry_style(browser)?;', failure)
        self.assertIn('include_str!("startup_retry.js")', source)
        self.assertIn('"Input.dispatchMouseEvent"', source)
        self.assertIn(r'browser.keys(&browser.css("#zxs-startup-error")?, "\u{e004}")?', source)


if __name__ == '__main__':
    unittest.main()

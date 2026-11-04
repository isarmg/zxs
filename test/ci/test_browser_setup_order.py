"""Keep isolated first-setup acceptance ahead of unrelated history scenarios."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]


class BrowserSetupOrder(unittest.TestCase):
    def test_setup_runs_first_without_skipping_later_scenarios_or_errors(self):
        source = (ROOT / 'test/tools/browser-smoke/runner/main.rs').read_text()
        dispatch = source.split('        let result = if placement_only {', 1)[1]
        normal = dispatch.split('        } else {\n', 1)[1].split('\n        };', 1)[0]
        self.assertEqual(re.findall(
            r'(diagnostics::install|management::setup|exercise|management::run|'
            r'placement_configuration::run)\(', normal),
            ['diagnostics::install', 'management::setup', 'exercise',
             'management::run', 'placement_configuration::run'])
        self.assertEqual(normal.count('.and_then('), 4)
        clear = 'browser.command("DELETE", "/cookie", None)?;'
        self.assertEqual(normal.count(clear), 1)
        self.assertLess(normal.index('management::setup('), normal.index(clear))
        self.assertLess(normal.index(clear), normal.index('exercise('))
        self.assertIn('if result.is_err()', dispatch)
        self.assertIn('diagnostics::failure(&browser', dispatch)
        self.assertIn('\n        result\n', dispatch)

    def test_setup_owns_private_pools_not_the_seeded_pool(self):
        source = (ROOT / 'test/tools/browser-smoke/runner/management.rs').read_text()
        setup = source.split('pub(super) fn setup(', 1)[1].split('\nstruct SetupFixture', 1)[0]
        self.assertNotIn('source_pool', setup)
        self.assertIn('SetupFixture::new(root, "fresh-web-setup")?', setup)
        self.assertIn('server.finish(||', setup)
        self.assertIn('restarted.finish(||', setup)
        fixture = source.split('impl SetupFixture {', 1)[1].split('\nconst REAUTHORIZE_PROBE', 1)[0]
        self.assertIn('let root = root.join(label);', fixture)
        self.assertIn('pool: root.join("pool")', fixture)
        self.assertIn('let management = root.join("management");', fixture)
        self.assertIn('fs::DirBuilder::new().mode(0o700).create(&root)?;', fixture)


if __name__ == '__main__':
    unittest.main()

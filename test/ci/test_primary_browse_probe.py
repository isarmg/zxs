"""Real-response observer checks and setup wiring, not browser acceptance."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / 'test/tools/browser-smoke'


class PrimaryBrowseProbe(unittest.TestCase):
    def test_observer_keeps_original_responses_errors_bounds_and_cleanup(self):
        subprocess.run(['node', str(PROBE / 'unit/primary_browse_probe_test.mjs')],
                       cwd=ROOT, check=True)

    def test_new_primary_is_browsed_after_ready_before_conversion(self):
        source = (PROBE / 'runner/management.rs').read_text()
        setup = source.split('pub(super) fn setup(', 1)[1].split('\nstruct SetupFixture', 1)[0]
        self.assertLess(setup.index('primary_browse::observe(browser)?;'),
                        setup.index('browser.script(STORAGE_PROBE)?;'))
        self.assertLess(setup.index('fixture.wait_state("ready")?;'),
                        setup.index('primary_browse::run(browser, &storage)?;'))
        self.assertLess(setup.index('primary_browse::run(browser, &storage)?;'),
                        setup.index('conversion::run(browser, &fixture)?;'))
        self.assertIn('let storage = restore_storage_probe(browser);', setup)
        self.assertIn('let browsing = primary_browse::restore(browser);', setup)
        flow = (PROBE / 'runner/management/primary_browse.rs').read_text()
        self.assertIn('browser.button("Files", "文件")', flow)
        self.assertIn('browser.button("All disks", "所有硬盘")', flow)
        self.assertIn('s.emptyRootReady()', flow)
        self.assertIn('mutations', flow)
        self.assertNotIn('new Response(', flow)


if __name__ == '__main__':
    unittest.main()

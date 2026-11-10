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

    def test_prepared_primary_is_browsed_after_ready_without_online_activation(self):
        source = (PROBE / 'runner/management.rs').read_text()
        flow = source.split('pub(super) fn prepared_primary(', 1)[1].split('pub(super) fn reject_empty(', 1)[0]
        self.assertLess(flow.index('prepare_primary(&fixture, password)?'), flow.index('fixture.launch('))
        self.assertLess(flow.index('fixture.wait_state("ready")?'), flow.index('primary_browse::observe(browser)?'))
        self.assertLess(flow.index('browser.button("Administration", "系统管理")'), flow.index('primary_browse::observe(browser)?'))
        self.assertLess(flow.index('primary_browse::observe(browser)?'), flow.index('primary_browse::run(browser, &storage)?'))
        self.assertIn('|| primary_browse::restore(browser)', flow)
        self.assertNotIn('conversion::run', flow)
        self.assertNotIn('/api/admin/storage/', flow)
        probe = (PROBE / 'runner/management/primary_browse.rs').read_text()
        self.assertIn('browser.button("Files", "文件")', probe)
        self.assertIn('browser.button("All disks", "所有硬盘")', probe)
        self.assertIn('s.emptyRootReady()', probe)
        self.assertIn('mutations', probe)
        self.assertNotIn('new Response(', probe)


if __name__ == '__main__':
    unittest.main()

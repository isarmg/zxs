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
        self.assertLess(flow.index('sign_in(browser, "prepared-admin", password)?'), flow.index('AdminPage::Storage.open(browser)?'))
        self.assertLess(flow.index('AdminPage::Storage.open(browser)?'), flow.index('primary_browse::observe(browser)?'))
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

    def test_storage_navigation_uses_the_current_header_entry_and_section(self):
        source = (PROBE / 'runner/admin_navigation.rs').read_text()
        self.assertIn('Self::Storage => ("Disks and admission", "磁盘与接入")', source)
        self.assertIn('Self::Storage => "section[data-zxs-admin-section=\'storage\']"', source)
        self.assertIn('zxs-header-navigation', source)
        self.assertIn("@data-zxs-management-menu='true'", source)
        self.assertLess(source.index('browser.click(&button)?'), source.index('browser.css(self.section())?'))

    def test_missing_selected_directory_is_not_recreated_on_restart(self):
        source = (PROBE / 'runner/management.rs').read_text()
        flow = source.split('pub(super) fn prepared_primary(', 1)[1].split('pub(super) fn reject_empty(', 1)[0]
        rename = 'fs::rename(&storage, storage.with_file_name("selected-storage-offline"))?'
        restart = 'fixture.launch(daemon, web, "missing-directory-server.log")?'
        self.assertLess(flow.index('primary_browse::run(browser, &storage)?'), flow.index(rename))
        self.assertLess(flow.index(rename), flow.index(restart))
        offline = flow.split(restart, 1)[1]
        self.assertLess(offline.index('fixture.wait_state("unavailable")?'), offline.index('sign_in(browser, "prepared-admin", password)?'))
        self.assertIn('browser.button("Status", "状态")', offline)
        self.assertIn(".zxs-monitor-page [data-metric='cpu']", offline)
        self.assertIn(".zxs-monitor-page [data-metric='memory']", offline)
        self.assertIn('if storage.exists()', offline)
        self.assertIn('missing selected storage directory was silently recreated', offline)
        self.assertLess(offline.index('if storage.exists()'), offline.index('sign_out(browser)'))


if __name__ == '__main__':
    unittest.main()

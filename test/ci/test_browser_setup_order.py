"""Keep offline setup boundaries and the remaining real browser scenarios wired."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / 'test/tools/browser-smoke/runner'


class BrowserSetupOrder(unittest.TestCase):
    def test_prepared_primary_runs_without_skipping_later_scenarios_or_errors(self):
        source = (RUNNER / 'main.rs').read_text()
        dispatch = source.split('        let result = if placement_only {', 1)[1]
        normal = dispatch.split('        } else {\n', 1)[1].split('\n        };', 1)[0]
        self.assertEqual(re.findall(
            r'(diagnostics::install|management::prepared_primary|exercise|management::run|'
            r'placement_configuration::run)\(', normal),
            ['diagnostics::install', 'management::prepared_primary', 'exercise',
             'management::run', 'placement_configuration::run'])
        self.assertEqual(normal.count('.and_then('), 4)
        clear = 'browser.command("DELETE", "/cookie", None)?;'
        self.assertEqual(normal.count(clear), 1)
        self.assertLess(normal.index('management::prepared_primary('), normal.index(clear))
        self.assertLess(normal.index(clear), normal.index('exercise('))
        self.assertIn('if result.is_err()', dispatch)
        self.assertIn('diagnostics::failure(&browser', dispatch)
        self.assertIn('\n        result\n', dispatch)

    def test_fixture_finishes_native_activation_before_launch_and_keeps_missing_disk_case(self):
        source = (RUNNER / 'management.rs').read_text()
        flow = source.split('pub(super) fn prepared_primary(', 1)[1].split('pub(super) fn reject_empty(', 1)[0]
        self.assertLess(flow.index('prepare_primary(&fixture, password)?'), flow.index('fixture.launch('))
        self.assertIn('server.finish(||', flow)
        self.assertIn('restarted.finish(||', flow)
        self.assertLess(flow.index('fs::rename(&storage,'), flow.index('let restarted ='))
        self.assertIn('fixture.wait_state("unavailable")?', flow)
        self.assertIn('missing selected storage directory was silently recreated', flow)
        prepare = source.split('fn prepare_primary(', 1)[1]
        self.assertIn('"activate",', prepare)
        self.assertIn('backend.drain_management()', prepare)
        self.assertLess(prepare.index('drop(backend);'), prepare.index('PreparedService::open('))
        self.assertNotIn('/api/setup/initialize', source)
        self.assertNotIn('/api/admin/storage/activate', source)
        self.assertNotIn('conversion::run', source)

    def test_actual_daemon_refusals_precede_browser_and_do_not_claim_helper_ui_coverage(self):
        source = (RUNNER / 'main.rs').read_text()
        self.assertLess(source.index('management::reject_empty('), source.index('let server = server_launcher::launch('))
        refusal = (RUNNER / 'management.rs').read_text().split('pub(super) fn reject_empty(', 1)[1].split('struct PreparedFixture', 1)[0]
        for marker in ['"missing-system"', '"empty-system"', '"empty-development-system"',
                       'command.arg("--allow-development-setup")', 'server.exit_code()',
                       'exited? != 1', 'ConnectionRefused', 'fs::read_dir(path)?.next().is_some()',
                       'normal ZXS cannot start without an existing storage configuration']:
            self.assertIn(marker, refusal)
        native = (ROOT / '.github/scripts/check-debian13-single-binary.py').read_text()
        self.assertIn('initialize_storage(helper, evidence, record)', native)
        self.assertIn('storage_activation_completed', native)
        self.assertIn('reject_daemon_while_helper', native)
        self.assertIn('reject_helper_while_daemon', native)


if __name__ == '__main__':
    unittest.main()

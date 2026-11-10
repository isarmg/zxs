"""Backup navigation observer and runner wiring, not real Chrome acceptance."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / "test/tools/browser-smoke"


class BackupBrowseProbe(unittest.TestCase):
    def test_host_observer_and_false_navigation_examples(self):
        subprocess.run(["node", str(PROBE / "unit/backup_browse_probe_test.mjs")],
                       cwd=ROOT, check=True)

    def test_navigation_runs_after_retry_and_before_primary_history_mutations(self):
        main = (PROBE / "runner/main.rs").read_text()
        self.assertEqual(main.count("backup_browse::run(browser)?;"), 1)
        sequence = main.split("phase1_files::disk_read(browser)?;", 1)[1]
        self.assertLess(sequence.index("backup_browse::run(browser)?;"),
                        sequence.index("open_fixture_directory(browser)?;"))
        self.assertLess(sequence.index("backup_browse::run(browser)?;"),
                        sequence.index("phase1_files::tags(browser)?;"))
        for existing in ["tag_unknown::run(browser)?;",
                         "history_windows::run(browser)?;"]:
            self.assertIn(existing, main)
        self.assertIn("node_cards::run(browser)?;",
                      (PROBE / "runner/tree_frames.rs").read_text())

    def test_runner_follows_returned_paths_and_restores_the_primary_baseline(self):
        runner = (PROBE / "runner/backup_browse.rs").read_text()
        self.assertIn('path = entry["path"]', runner)
        self.assertIn('click_entry(browser, name)?;', runner)
        self.assertIn('for slot in ["B01", "C01"]', runner)
        self.assertEqual(runner.count('open_seed_file(browser, "P01")?;'), 2)
        self.assertIn('for _ in 0..8', runner)
        self.assertIn('not a product limit', runner)
        self.assertIn('super::assert_metadata_only(browser)?;', runner)
        self.assertNotIn('/api/', runner)
        self.assertNotIn('P01/browser-fixtures', runner)
        self.assertIn('window.__zxsBackupBrowse.finish()', runner)

    def test_pass_through_guard_and_cleanup_remain_test_only(self):
        runner = (PROBE / "runner/backup_browse.rs").read_text()
        observer = (PROBE / "runner/backup_browse.js").read_text()
        self.assertIn('Reflect.apply(original, this, args)', observer)
        self.assertIn('return response;', observer)
        self.assertNotIn('new Response(', observer)
        self.assertNotIn('setInterval(', observer)
        self.assertNotIn('setTimeout(', observer)
        self.assertNotIn('localStorage', observer)
        self.assertNotIn('sessionStorage', observer)
        self.assertLess(observer.index('guard blocked a mutation or content request before sending'),
                        observer.index('Reflect.apply(original, this, args)'))
        self.assertIn('window.fetch !== observed', observer)
        self.assertIn('window.fetch = original', observer)
        self.assertIn('with_cleanup(|| saved, || restored)', runner)
        self.assertIn('"backup-navigation-final"', runner)

    def test_reuses_the_existing_real_three_copy_cold_open_fixture(self):
        fixture = (PROBE / "runner/fixture.rs").read_text()
        self.assertIn('["P01", "B01", "C01"]', fixture)
        self.assertIn('storage_engine::Config::development(paths)?', fixture)
        self.assertIn('RepositoryName::parse("browser-fixtures")', fixture)
        self.assertIn('"P01",\n        3,', fixture)
        self.assertIn('fixture current bytes did not survive reopen', fixture)

    def test_exact_host_wrapper_is_wired_into_the_real_runner_target(self):
        runner = (PROBE / "runner/backup_browse.rs").read_text()
        wrapper = (PROBE / "unit/backup_browse_tests.rs").read_text()
        self.assertIn('#[cfg(test)]\n#[path = "../unit/backup_browse_tests.rs"]', runner)
        self.assertIn('backup_browse_probe_test.mjs', wrapper)
        self.assertIn('result.status.success()', wrapper)


if __name__ == "__main__":
    unittest.main()

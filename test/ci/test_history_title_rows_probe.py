"""Focused history-row assertion tests, not production browser evidence."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / 'test/tools/browser-smoke'


class HistoryTitleRowsProbe(unittest.TestCase):
    def test_assertion_rejects_extra_height_and_repeated_text_insets(self):
        subprocess.run(['node', str(PROBE / 'unit/history_title_rows_probe_test.mjs')],
                       cwd=ROOT, check=True)

    def test_real_browser_checks_file_titles(self):
        for runner in ['tree_frames.rs']:
            source = (PROBE / 'runner' / runner).read_text()
            self.assertIn('include_str!("history_title_rows.js")', source)


if __name__ == '__main__':
    unittest.main()

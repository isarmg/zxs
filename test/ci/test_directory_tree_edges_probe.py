"""Focused contract tests for the directory SVG probe, not browser evidence."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / 'test/tools/browser-smoke'


class DirectoryTreeEdgesProbe(unittest.TestCase):
    def test_assertion_detects_missing_invented_invisible_or_misplaced_edges(self):
        subprocess.run(['node', str(PROBE / 'unit/directory_tree_edges_probe_test.mjs')],
                       cwd=ROOT, check=True)

    def test_real_browser_uses_probe_before_and_after_refresh_and_anchor(self):
        source = (PROBE / 'runner/directory_history.rs').read_text()
        self.assertEqual(source.count('include_str!("directory_tree_edges.js")'), 3)
        self.assertIn('p.originalEdge.isConnected', source)
        self.assertIn('p.originalEdge.getTotalLength()>0', source)

    def test_fixture_initializes_real_directory_history_before_two_content_writes(self):
        source = (PROBE / 'runner/fixture.rs').read_text()
        self.assertLess(source.index('engine.initialize_directory_scope('),
                        source.index('for payload in PAYLOADS'))
        unit = (PROBE / 'unit/fixture_tests.rs').read_text()
        self.assertIn('root scope plus two writes are real directory snapshots', unit)
        self.assertIn('the browser fixture must exercise actual SVG parent edges', unit)
        self.assertIn('engine.state().directory_trees[&node.tree_id].parent_tree_id', unit)

    def test_readonly_history_uses_a_selected_real_preview_after_rejecting_unsolicited_preview(self):
        source = (PROBE / 'runner/directory_history.rs').read_text()
        self.assertIn('response.ok&&p.unexpectedPreview', source)
        self.assertIn('p.unexpectedPreview=false;p.rejectedPages++', source)
        self.assertIn("includes('INVALID_DIRECTORY_PAGE')", source)
        self.assertIn("querySelectorAll('.zxs-file-table .zxs-file-name').length===0", source)
        self.assertIn('fixture::DIRECTORY_PREVIEW', source)
        self.assertIn("url.searchParams.has('directory_branch_id')&&response.ok", source)
        self.assertIn("url.searchParams.get('expected_tree_id')", source)
        self.assertEqual(source.count('body.preview=true;body.writable=false;'), 1)
        fixture = (PROBE / 'runner/fixture.rs').read_text()
        self.assertIn('seed_directory_history(&mut engine)?;', fixture)
        unit = (PROBE / 'unit/fixture_tests.rs').read_text()
        self.assertIn('engine\n        .preview_directory_branch(', unit)
        self.assertIn('branches[0].head_tree_id, branches[1].head_tree_id', unit)


if __name__ == '__main__':
    unittest.main()

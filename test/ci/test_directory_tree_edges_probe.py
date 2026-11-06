"""Focused contract tests for the directory SVG probe, not browser evidence."""
from pathlib import Path
import json
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / 'test/tools/browser-smoke'


def snapshot_target_script(source):
    return re.search(r'let target = browser\.script\(r#"(.*?)"#\)\?;', source, re.S).group(1)


def native_snapshot_wiring(source):
    """Reject script activation or losing the selected real tree identity."""
    if 'let target = browser.script' not in source:
        return False
    script = snapshot_target_script(source)
    start = source.index('let target = browser.script')
    end = source.index('"explicit historical snapshot loads read-only', start)
    block = source[start:end]
    return ("CSS.escape(node.tree_id)" in script
            and "node.tree_id!==p.nodes.current.committed_tree_id" in script
            and 'return {selector,open:detail.open}' in script
            and '.click(' not in script and '.open=' not in script
            and 'if target["open"] != true' in block
            and '"{selector} > .zxs-node-disclosure > .zxs-node-card"' in block
            and 'browser.click(&browser.css(&format!("{selector} [data-directory-snapshot]"))?)?;' in block
            and block.index('if target["open"] != true')
            < block.index('"{selector} > .zxs-node-disclosure > .zxs-node-card"')
            < block.index('"{selector} [data-directory-snapshot]"'))


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
        self.assertLess(source.index('initialize_directory_fixture(&mut engine, &repository, "")?;'),
                        source.index('for payload in PAYLOADS'))
        self.assertLess(source.index('commit_directory_fixture(&mut engine, &repository.repository_id, "")?;'),
                        source.index('for payload in PAYLOADS'))
        self.assertIn('engine.initialize_directory_git(', source)
        self.assertNotIn('engine.initialize_directory_scope(', source)
        unit = (PROBE / 'unit/fixture_tests.rs').read_text()
        self.assertIn('the root scope and both writes have explicit user directory commits', unit)
        self.assertIn('the browser fixture must exercise actual SVG parent edges', unit)
        self.assertIn('user_commit_parent(&engine, &node.tree_id)', unit)
        self.assertIn('committed_directory_ancestor(', unit)

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
        self.assertIn('branches[0].committed_tree_id, branches[1].committed_tree_id', unit)

    def test_real_snapshot_target_is_observed_without_script_activation(self):
        source = (PROBE / 'runner/directory_history.rs').read_text()
        program = r'''
const assert = require('node:assert/strict');
const select = new Function(JSON.parse(process.argv[1]));
const current = 'tree_current', historical = 'tree_historical';
const selector = '[data-directory-tree-id="'+historical+'"]';
let detail, button;
globalThis.CSS = {escape(value) { assert.equal(value,historical);return value; }};
globalThis.document = {querySelector(value) {
    assert.equal(value,selector);
    return {querySelector(value) {
        if(value===':scope > .zxs-node-disclosure')return detail;
        assert.equal(value,'[data-directory-snapshot="'+historical+'"]');return button;
    }};
}};
function fixture(open) {
    globalThis.window = {__zxsDirectoryHistory:{nodes:{
        nodes:[{tree_id:current},{tree_id:historical}],current:{committed_tree_id:current}}}};
    detail = {open};button = {disabled:false,click() { throw new Error('script click bypass'); }};
}
for(const open of [false,true]) {
    fixture(open);
    assert.deepEqual(select(),{selector,open});
    assert.equal(window.__zxsDirectoryHistory.selectedSnapshot,historical);
    assert.equal(detail.open,open,'observation must not toggle native details');
}
fixture(false);button.disabled=true;
assert.throws(select,/historical snapshot is unavailable/);
fixture(false);button=null;
assert.throws(select,/historical snapshot is unavailable/);
fixture(false);detail=null;
assert.throws(select,/historical snapshot is unavailable/);
fixture(false);window.__zxsDirectoryHistory.nodes.nodes=[{tree_id:current}];
assert.throws(select,/requires a non-current committed node/);
'''
        subprocess.run(['node', '-e', program, json.dumps(snapshot_target_script(source))],
                       cwd=ROOT, check=True)

    def test_real_snapshot_has_native_disclosure_before_native_activation(self):
        source = (PROBE / 'runner/directory_history.rs').read_text()
        self.assertTrue(native_snapshot_wiring(source))
        self.assertIn('p.previewTree===p.selectedSnapshot', source)
        self.assertIn("[data-zxs-set-directory-default]", source)
        self.assertIn("&&!document.querySelector('[data-commit-directory]')&&p.writes===0", source)

    def test_hidden_snapshot_script_click_or_missing_disclosure_is_rejected(self):
        source = (PROBE / 'runner/directory_history.rs').read_text()
        mutants = [
            source.replace('let target = browser.script', 'browser.script', 1),
            source.replace('return {selector,open:detail.open}', 'button.click();return {selector,open:detail.open}', 1),
            source.replace('"{selector} > .zxs-node-disclosure > .zxs-node-card"', '".zxs-node-card"', 1),
            source.replace('if target["open"] != true', 'if false', 1),
            source.replace('CSS.escape(node.tree_id)', 'CSS.escape(node.name)'),
        ]
        for mutant in mutants:
            with self.subTest(mutant=mutant[source.index('let target'):][:180]):
                self.assertFalse(native_snapshot_wiring(mutant))


if __name__ == '__main__':
    unittest.main()

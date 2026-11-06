"""Static-suite coverage of the browser probe itself, not browser evidence."""
from pathlib import Path
import json
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / 'test/tools/browser-smoke'


def detail_fixture_script(source):
    function = source.split('pub(super) fn metadata_edit', 1)[1]
    return re.search(r'browser\.script\(r#"(.*?)"#\)\?;', function, re.S).group(1)


def detail_fixture_host(script):
    """Execute the actual delivery wrapper, not a substitute graph builder."""
    program = r'''
const assert = require('node:assert/strict');
const script = JSON.parse(process.argv[1]);
const historyUrl = 'http://127.0.0.1/api/file-manager/files/real-file/history';
const base = {node_id:'ver_real',version_id:'ver_body',sequence:'2',current:true,
    kind:'content',note:'real note',parents:{state:'complete'}};
const older = {...base,node_id:'ver_older',sequence:'1',current:false};
let response;
const original = async () => response;
globalThis.window = {fetch:original};
globalThis.location = {href:'http://127.0.0.1/'};
new Function(script)();
(async () => {
    const input = {identity:{limit:'32'},nodes:[base,older],edges:[]};
    response = new Response(JSON.stringify(input));
    const result = await (await window.fetch(historyUrl)).json();
    assert.equal(result.nodes.length,26,'24 closed compact tips must widen the desktop fixture');
    const tips = result.nodes.filter(node => node.node_id.startsWith('fmeta_'));
    assert.equal(tips.length,24);
    assert.equal(new Set(result.nodes.map(node => node.node_id)).size,26);
    assert.deepEqual(result.nodes.find(node => node.node_id===base.node_id),base);
    assert.deepEqual(result.nodes.find(node => node.node_id===older.node_id),older);
    assert(tips.every(node => node.version_id===base.version_id && !node.current));
    assert.deepEqual(result.edges,tips.map(node => ({node_id:node.node_id,
        parent_node_id:base.node_id,endpoint:'in_window'})));
    assert.deepEqual(result.identity,input.identity);
    assert.equal(window.__zxsDetailRefresh.historyResponses,1);
    response = new Response(JSON.stringify({...input,identity:{limit:'25'}}));
    await assert.rejects(window.fetch(historyUrl),/exceeds the real requested node budget/);
    assert.equal(window.__zxsDetailRefresh.historyResponses,1);
    response = new Response(JSON.stringify({...input,nodes:[]}));
    await assert.rejects(window.fetch(historyUrl),/real node required/);
    response = new Response('{"error":{"code":"RESOURCE_LIMIT_EXCEEDED"}}',{status:429});
    assert.equal(await window.fetch(historyUrl),response,'429 must pass through unchanged');
    response = new Response('original non-history bytes');
    assert.equal(await window.fetch('http://127.0.0.1/api/management/status'),response);
    response = new Response(JSON.stringify({selected_entry:{note:'browser metadata note'}}));
    assert.equal(await window.fetch('http://127.0.0.1/api/file-manager/disks/P01/entries'),response);
    assert.equal(window.__zxsDetailRefresh.savedListingSeen,true);
    window.__zxsDetailRefresh.restore();
    assert.equal(window.fetch,original);
})().catch(error => { console.error(error);process.exitCode=1; });
'''
    return subprocess.run(['node', '-e', program, json.dumps(script)], cwd=ROOT,
                          capture_output=True, text=True)


class TreeFrameProbe(unittest.TestCase):
    def test_javascript_parses(self):
        for name in ('tree_frames.js', 'tree_geometry.js'):
            with self.subTest(name=name):
                subprocess.run(['node', '--check', str(PROBE / 'runner' / name)],
                               check=True, cwd=ROOT)

    def test_host_instrumentation(self):
        subprocess.run(['node', str(PROBE / 'unit/tree_frame_probe_test.mjs')],
                       check=True, cwd=ROOT)

    def test_compact_wide_fixture_preserves_real_records_budget_and_errors(self):
        source = (PROBE / 'runner/metadata.rs').read_text()
        result = detail_fixture_host(detail_fixture_script(source))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_old_twelve_tip_fixture_is_rejected(self):
        source = (PROBE / 'runner/metadata.rs').read_text()
        script = detail_fixture_script(source)
        self.assertIn('length:24', script)
        result = detail_fixture_host(script.replace('length:24', 'length:12', 1))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('24 closed compact tips', result.stderr)

    def test_real_scroll_save_reentry_and_native_note_stimuli_remain_required(self):
        source = (PROBE / 'runner/metadata.rs').read_text()
        self.assertIn('tree.scrollWidth-tree.clientWidth>100', source)
        self.assertIn('return probe.anchor.left>100', source)
        self.assertIn('Math.abs(a.tree.scrollLeft-a.left)<=1', source)
        self.assertIn('probe.savedListingSeen&&probe.historyResponses>a.histories', source)
        self.assertIn('a.control===a.node.querySelector', source)
        self.assertIn('persisted note or seeded version tree missing', source)
        self.assertIn('window.__zxsDetailRefresh.restore()', source)
        tree = (PROBE / 'runner/tree_frames.rs').read_text()
        self.assertLess(tree.index('node_cards::open_first(browser)?;'),
                        tree.index('browser.click(&browser.css(".zxs-version-node .zxs-version-note-action")?)?;'))
        self.assertIn('browser.keys(&editor, "\\u{e00c}")?;', tree)

    def test_g4_extent_is_checked_on_the_non_inherited_native_card_host(self):
        geometry = (ROOT / 'src/web/public/styles/components/geometry.css').read_text()
        registration = re.search(r'@property --zxs-contour-extent\s*\{([^}]+)\}', geometry).group(1)
        self.assertIn('inherits: false', registration)
        self.assertIn('initial-value: 0px', registration)
        frame = re.search(r'\.zxs-app-shell \.zxs-frame\s*\{([^}]+)\}', geometry).group(1)
        self.assertIn('--zxs-contour-extent: var(--zxs-contour-panel)', frame)
        source = (PROBE / 'runner/tree_geometry.js').read_text()
        self.assertIn(':scope > details.zxs-node-disclosure > summary.zxs-node-card.zxs-frame', source)
        self.assertIn("getComputedStyle(card).getPropertyValue('--zxs-contour-extent')", source)
        self.assertNotIn("style.getPropertyValue('--zxs-contour-extent')", source)
        self.assertIn('node.getBoundingClientRect(), style = getComputedStyle(node)', source)
        self.assertIn("Math.abs(parseFloat(style.fontSize) - 14.4) < .1", source)

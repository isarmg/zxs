"""Execute the harness's graph predicate against delayed DOM/response states."""
import json
from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


def assert_date_disclosures(test, source):
    fields = source.split('fn fields_and_cancellation(', 1)[1]
    clicks = list(re.finditer(r'\[data-node-id=\'\{current\}\'\] '
                              r'\[data-history-field=created_at\]', fields))
    test.assertEqual(len(clicks), 2)
    previous = 0
    for click in clicks:
        prefix = fields[previous:click.start()]
        test.assertRegex(prefix, r'node_cards::open_current\(browser\)\?;\s+'
                         r'browser\.script\("window\.__zxsHistoryWindows\.holdField=true;return true"\)\?;\s+'
                         r'browser\.click\(&browser\.css\(&format!\(\s+"$')
        previous = click.end()


class HistoryWindowWait(unittest.TestCase):
    def test_requested_window_not_previous_self_consistent_window(self):
        source = (ROOT / 'test/tools/browser-smoke/runner/history_windows.rs').read_text()
        graph = source.split('fn graph(', 1)[1].split('enum Choice', 1)[0]
        predicate = re.search(r'r#"(.*?)"#', graph, re.S).group(1)
        result = subprocess.run(['node', '-e', r'''
const predicate=new Function('expectedIndex', process.argv[1]);
const ids=index=>Array.from({length:32},(_,i)=>index+':'+i);
const root={isConnected:true};
const p={index:2,expected:ids(2),roots:[]};
let dom=ids(2);
const records=[{root,disconnected:false},{root,disconnected:false}];
global.window={__zxsHistoryWindows:p,__zxsTreeFrames:{observers:records}};
global.document={
 querySelector:()=>root,
 querySelectorAll:selector=>selector==='.zxs-version-node'?dom.map(nodeId=>({dataset:{nodeId}})):[root]
};
const results=[];
// Before the asynchronous response: old expected + old DOM must not pass.
results.push(predicate(1));
// The requested response arrived, but its DOM has not yet been installed.
p.index=1;p.expected=ids(1);results.push(predicate(1));
// Only the exact requested 32-node page with both observers may pass.
dom=ids(1);results.push(predicate(1));
dom=ids(1).slice(1);results.push(predicate(1));
dom=ids(1);records[1].disconnected=true;results.push(predicate(1));
// A retired graph must still have both owners disconnected.
records[1].disconnected=false;
const old={isConnected:false};p.roots.push(old);
records.push({root:old,disconnected:false},{root:old,disconnected:true});
results.push(predicate(1));records[2].disconnected=true;results.push(predicate(1));
console.log(JSON.stringify(results));
''', predicate], check=True, capture_output=True, text=True)
        self.assertEqual(json.loads(result.stdout), [False, False, True, False, False, False, True])

    def test_page_sequence_waits_for_exact_requested_window(self):
        source = (ROOT / 'test/tools/browser-smoke/runner/history_windows.rs').read_text()
        parts = re.split(r'^fn ([a-z_]+)\(', source, flags=re.MULTILINE)
        scenarios = dict(zip(parts[1::2], parts[2::2]))
        actual = {name: re.findall(r'graph\(browser, (\d+)\)', body)
                  for name, body in scenarios.items()
                  if re.search(r'graph\(browser, \d+\)', body)}
        self.assertEqual(actual, {
            'exercise': ['0', '1', '2', '1', '2'],
            'container_tag_draft': ['1'],
            'merge_review': ['2', '1'],
            'graph_merge_review': ['1', '2', '1'],
            'fields_and_cancellation': ['1', '2', '1', '1', '0'],
        })
        self.assertEqual(re.findall(
            r'^    (container_tag_draft|merge_review|graph_merge_review|fields_and_cancellation)'
            r'\(browser\)',
            scenarios['exercise'], re.MULTILINE),
            ['container_tag_draft', 'merge_review', 'graph_merge_review',
             'fields_and_cancellation'])
        tag_draft = scenarios['container_tag_draft']
        self.assertEqual(re.findall(
            r'click\(browser, "(Locate current node|Next history page|Previous history page)"'
            r'[^\n]+\n\s+graph\(browser, (\d+)\)', tag_draft),
            [('Previous history page', '1')])
        graph_merge = scenarios['graph_merge_review']
        self.assertEqual(re.findall(
            r'click\(browser, "(Locate current node|Next history page|Previous history page)"'
            r'[^\n]+\n\s+graph\(browser, (\d+)\)', graph_merge),
            [('Locate current node', '1'), ('Next history page', '2'),
             ('Previous history page', '1')])
        for contract in (
            'one source cannot choose or submit a result',
            'historical inputs keep source bases across graph pages',
            'chosen result survives leaving its page',
            'cancelling graph merge preserves the current window and removes only its selection controls',
        ):
            self.assertIn(contract, graph_merge)
        fields = scenarios['fields_and_cancellation']
        self.assertEqual(re.findall(
            r'click\(browser, "(Locate current node|Next history page|Previous history page)"'
            r'[^\n]+\n\s+graph\(browser, (\d+)\)', fields),
            [('Next history page', '2'), ('Locate current node', '1'),
             ('Locate current node', '1')])
        for contract in (
            'field review starts with the current node outside the visible window',
            'field review locates the exact current node',
        ):
            self.assertIn(contract, fields)
        self.assertIn('for _ in 0..4', source)
        for contract in (
            'click(browser, "Reload tags", "重新读取标签")?;',
            'phase1_files::edit_current_tags(browser)?;',
            'read.body.file_id!==p.seed.identity.file_id',
            'structuredClone(p.tagReads.at(-1))',
            "panel===p.tagEditor&&panel.textContent.includes('window-proof')&&sameSnapshot",
            "document.querySelectorAll('[data-container-tag-editor]').length===1",
            "['Save tags','保存标签'].includes(button.textContent.trim())&&!button.disabled",
            'file-context refresh retains the container draft but closes submission',
            "panel?.textContent.includes('window-proof')&&p.tagWrites===0",
            "['Save tags','保存标签'].includes(button.textContent.trim())&&button.disabled",
            "['Cancel','取消'].includes(button.textContent.trim())).click()",
            "return !document.querySelector('[data-container-tag-editor]')",
            '&&window.__zxsHistoryWindows.tagWrites===0',
        ):
            self.assertIn(contract, tag_draft)
        self.assertNotIn('.zxs-node-checkbox', source)

    def test_date_reads_open_the_current_native_card_before_each_held_request(self):
        source = (ROOT / 'test/tools/browser-smoke/runner/history_windows.rs').read_text()
        assert_date_disclosures(self, source)
        fields = source.split('fn fields_and_cancellation(', 1)[1]
        for contract in (
            'auxiliary A held',
            'main refresh aborts old auxiliary A',
            'new auxiliary B held',
            'late auxiliary A cannot install or release B',
            'B remains owned and is aborted by replacement',
            'late auxiliary B cannot restore a retired fragment',
        ):
            self.assertIn(contract, fields)

    def test_either_missing_disclosure_reproduces_the_regression(self):
        source = (ROOT / 'test/tools/browser-smoke/runner/history_windows.rs').read_text()
        call = '    node_cards::open_current(browser)?;\n'
        self.assertEqual(source.count(call), 2)
        first, middle, last = source.split(call)
        for mutated in (first + middle + call + last, first + call + middle + last,
                        first + middle + last):
            with self.subTest(disclosures=mutated.count(call)):
                with self.assertRaises(AssertionError):
                    assert_date_disclosures(self, mutated)

    def test_current_disclosure_uses_real_summary_click_not_hidden_control_click(self):
        source = (ROOT / 'test/tools/browser-smoke/runner/node_cards.rs').read_text()
        current = source.split('pub(super) fn open_current(', 1)[1].split('fn check_cards(', 1)[0]
        self.assertIn("return card.parentElement.open", current)
        self.assertIn('browser.click(&browser.css(".zxs-node-card[aria-current=true]")?)?;', current)
        self.assertNotIn('.click()', current)
        self.assertNotRegex(current, r'\.open\s*=')
        self.assertNotIn('data-history-field', current)


if __name__ == '__main__':
    unittest.main()

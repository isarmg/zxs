"""Execute the harness's graph predicate against delayed DOM/response states."""
import json
from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


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
            'exercise': ['0', '1', '2', '1', '2', '1'],
            'merge_review': ['2', '1'],
            'graph_merge_review': ['1', '2', '1'],
            'fields_and_cancellation': ['1', '1', '0'],
        })
        self.assertEqual(re.findall(
            r'^    (merge_review|graph_merge_review|fields_and_cancellation)\(browser\)',
            scenarios['exercise'], re.MULTILINE),
            ['merge_review', 'graph_merge_review', 'fields_and_cancellation'])
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
            'cancelling graph merge removes only its selection controls',
        ):
            self.assertIn(contract, graph_merge)
        self.assertIn('for _ in 0..4', source)
        self.assertIn(".zxs-node-checkbox').click()", source)


if __name__ == '__main__':
    unittest.main()

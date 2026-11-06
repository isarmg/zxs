"""Host checks of the actual disk-tree browser oracle, not Chrome evidence."""
from pathlib import Path
import json
import re
import subprocess
import unittest


ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / 'test/tools/browser-smoke/runner/disk_tree.rs'


def geometry_script(source):
    geometry = source.split('fn geometry(browser:', 1)[1].split(
        'fn keyboard_selection(', 1)[0]
    return re.search(r'browser\.script\(r#"(.*?)"#\)\?;',
                     geometry, re.S).group(1)


class DiskTreeTiers(unittest.TestCase):
    def test_actual_geometry_oracle_accepts_both_locales_and_rejects_regressions(self):
        program = r'''
const assert = require('node:assert/strict');
const probe = new Function(JSON.parse(process.argv[1]));
let nodes, edges;
const box = (top,bottom) => ({left:10,right:290,top,bottom,
    toJSON(){return {left:this.left,right:this.right,top:this.top,bottom:this.bottom};}});
function fixture(locale='en') {
    nodes = [['P01',1,332],['B01',2,166],['C01',3,0]].map(([slot,level,top]) => ({
        dataset:{slot,level:String(level)},
        label:locale==='zh'?(level===1?'主盘 · 第 1 层':`备份盘 · 第 ${level} 层`):
            (level===1?'Primary · Tier 1':`Backup disk · Tier ${level}`),
        rect:box(top,top+130),
        getBoundingClientRect(){return this.rect;},
        querySelector(selector){
            assert.equal(selector,'[data-disk-tier-label]');
            return this.label===null?null:{textContent:this.label};
        }
    }));
    edges = [['P01','B01',296],['B01','C01',130]].map(([source,target,top]) => ({
        dataset:{sourceSlot:source,targetSlot:target}, reversed:false,
        arrow:'M2 10 L6 2 L10 10 Z', rect:box(top,top+36),
        getBoundingClientRect(){return this.rect;},
        querySelector(selector){
            if(selector==='.zxs-disk-tree-line')return {
                getTotalLength:()=>32,
                getPointAtLength:d=>({x:6,y:this.reversed?(d===0?4:36):(d===0?36:4)})
            };
            assert.equal(selector,'.zxs-disk-tree-arrow');
            return {getAttribute:name=>{assert.equal(name,'d');return this.arrow;}};
        }
    }));
    globalThis.document = {
        documentElement:{scrollWidth:320,clientWidth:320},
        querySelector:selector=>{
            assert.equal(selector,'.zxs-disk-tree-scroll');
            return {scrollWidth:320,clientWidth:320};
        },
        querySelectorAll:selector=>{
            if(selector==='.zxs-disk-tree-node')return nodes;
            assert.equal(selector,'.zxs-disk-tree-edge');return edges;
        }
    };
    globalThis.getComputedStyle=()=>({pointerEvents:'none'});
}
for(const locale of ['en','zh']) {
    fixture(locale);
    assert.equal(probe().valid,true,`${locale} registered tiers and upward arrows`);
}
for(const [name,change] of [
    ['old backup-count label',()=>nodes[1].label='Backup 1 · Tier 2'],
    ['old Chinese backup-count label',()=>nodes[1].label='第 1 级备份 · 第 2 层'],
    ['missing visible tier',()=>nodes[1].label=null],
    ['wrong visible number',()=>nodes[1].label='Backup disk · Tier 3'],
    ['reversed arrow',()=>edges[0].reversed=true],
    ['wrong source identity',()=>edges[0].dataset.sourceSlot='C01'],
    ['arrow misses upper node',()=>edges[0].rect.top+=2],
    ['primary above backup',()=>nodes[0].rect=box(0,130)],
    ['unregistered candidate treated as a source',()=>edges[0].dataset.sourceSlot='/mnt/unadmitted']
]) {
    fixture();change();assert.equal(probe().valid,false,name);
}
fixture();edges.pop();assert.equal(probe().ready,false,'missing real edge');
console.log('disk tree: 2 positive and 10 negative host cases passed');
'''
        subprocess.run(['node', '-e', program,
                        json.dumps(geometry_script(RUNNER.read_text()))],
                       check=True, cwd=ROOT)

    def test_label_wiring_preserves_candidate_and_registered_identity_boundaries(self):
        tree = (ROOT / 'src/web/src/admin/disks/tree.rs').read_text()
        view = (ROOT / 'src/web/src/admin/disks/tree/view.rs').read_text()
        self.assertIn('"Backup disk · Tier {0}"', tree)
        self.assertIn('&[&self.level.to_string()]', tree)
        self.assertNotIn('level - 1', tree + view)
        self.assertIn('node.tier_label()', view)
        self.assertIn('data-disk-tier-label=""', view)
        candidate = view.split('fn candidate_node(', 1)[1].split(
            'pub(in super::super) fn view(', 1)[0]
        self.assertIn('data-level="-1"', candidate)
        self.assertIn('tree.pending_row()', candidate)
        self.assertIn('aria-controls="zxs-disk-wizard"', candidate)
        self.assertNotIn('data-slot=', candidate)
        self.assertNotIn('data-source-slot=', candidate)
        self.assertNotIn('data-target-slot=', candidate)
        self.assertIn('storage::choose_disk(state,&path.get_value())', candidate)
        self.assertIn('tree.edges.keys()', view)
        self.assertIn('d="M6 36 V4"', view)
        self.assertIn('d="M2 10 L6 2 L10 10 Z"', view)


if __name__ == '__main__':
    unittest.main()

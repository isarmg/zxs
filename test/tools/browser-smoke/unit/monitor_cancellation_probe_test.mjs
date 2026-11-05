// Synthetic host models exercise the real probe expressions, not product WASM.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const rust=readFileSync(new URL('../runner/history_cancellation.rs',import.meta.url),'utf8');
const script=name=>rust.split(`const ${name}: &str = r#"`)[1].split('"#;')[0];
function fixture(settled=false){
    const pending=Array.from({length:21},(_,i)=>({ready:true,delivered:i<20||settled,
        abortedAtDelivery:i<20?true:(settled?false:null),signal:{aborted:i<20||settled}}));
    const charts=Array.from({length:3},()=>({busy:String(!settled),getAttribute(name){assert.equal(name,'aria-busy');return this.busy;}}));
    const context={window:{__zxsMonitorCancellation:{pending}},document:{querySelectorAll(selector){assert.equal(selector,'.zxs-monitor-page .zxs-metric-chart');return charts;}}};
    return {pending,charts,run:()=>runInNewContext(`(function(){${script(settled?'MONITOR_HISTORY_SETTLED':'MONITOR_HISTORY_PENDING')}})()`,context)};
}
let count=0;
const check=fn=>{fn();count++;};
for(const settled of [false,true]){
    check(()=>assert.equal(fixture(settled).run(),true));
    for(const busy of [null,undefined,'',String(settled)])check(()=>{const f=fixture(settled);f.charts[1].busy=busy;assert.equal(f.run(),false);});
    check(()=>{const f=fixture(settled);f.charts.length=0;assert.equal(f.run(),false);});
    check(()=>{const f=fixture(settled);f.pending[0].delivered=false;assert.equal(f.run(),false);});
    check(()=>{const f=fixture(settled);f.pending[0].signal.aborted=false;assert.equal(f.run(),false);});
    check(()=>{const f=fixture(settled);f.pending[0].abortedAtDelivery=false;assert.equal(f.run(),false);});
    check(()=>{const f=fixture(settled);f.pending.at(-1).signal.aborted=!settled;assert.equal(f.run(),false);});
    check(()=>{const f=fixture(settled);f.pending.at(-1).abortedAtDelivery=settled?true:false;assert.equal(f.run(),false);});
    check(()=>{const f=fixture(settled);f.pending.at(-1).delivered=!settled;assert.equal(f.run(),false);});
    check(()=>{const f=fixture(settled);f.pending.pop();assert.equal(f.run(),false);});
    check(()=>{const f=fixture(settled);f.pending.push({...f.pending.at(-1)});assert.equal(f.run(),false);});
}
check(()=>{const f=fixture();f.pending.at(-1).ready=false;assert.equal(f.run(),false);});
check(()=>{assert.match(rust,/pending\.abortedAtDelivery = pending\.signal\.aborted;/);});
check(()=>{assert.doesNotMatch(rust,/Loading history|正在读取历史/);assert.match(rust,/for index in 0\.\.21/);assert.match(rust,/loading heartbeat replaced unchanged history SVG/);assert.match(rust,/equal heartbeat replaced unchanged history SVG/);});
console.log(`PASS ${count} monitor cancellation probe models (not real browser acceptance)`);

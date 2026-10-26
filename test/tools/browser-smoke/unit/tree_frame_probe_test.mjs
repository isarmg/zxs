// Host instrumentation unit test only, NOT production WASM/browser evidence.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const source = readFileSync(new URL('../runner/tree_frames.js', import.meta.url), 'utf8');
const queue = new Map();
let id = 0;
class Element {
    constructor(canvas = false) { this.canvas = canvas; this.width = 294; this.height = 100; this.isConnected = true; }
    getBoundingClientRect() { return {width:this.width, height:this.height}; }
    getAttribute(name) { return name === 'class' ? (this.canvas ? 'zxs-version-tree-canvas' : 'zxs-version-node') : 'test-version'; }
    matches(selector) { return this.canvas && selector === '.zxs-version-tree-canvas'; }
    querySelectorAll() { return []; }
}
class Observer {
    constructor(callback) { this.callback = callback; }
    observe() {}
    disconnect() {}
}
const root = new Element(true), node = new Element();
const nativeRaf = callback => { queue.set(++id, callback); return id; };
const nativeCancel = id => queue.delete(id);
const context = {Element, innerWidth:390, innerHeight:844, ResizeObserver: Observer, MutationObserver: Observer,
    requestAnimationFrame: nativeRaf, cancelAnimationFrame: nativeCancel,
    document: {querySelector: () => root, fonts: {ready: Promise.resolve()}},
    performance: {now: () => 1}};
context.window = context;
runInNewContext(source, context);
const p = context.__zxsTreeFrames;
const measure = () => root.querySelectorAll('.zxs-version-node[data-version-id]');
measure();
let scheduled = false, closed = false, pending;
const request = () => {
    if (scheduled || closed) return;
    scheduled = true;
    pending = context.requestAnimationFrame(() => { if (!closed) {scheduled = false; measure();} });
};
const resize = new context.ResizeObserver(request), mutation = new context.MutationObserver(request);
resize.observe(root);resize.observe(node);mutation.observe(root);
assert.equal(p.roots.get(root).initial, 1);
// A G4-like observer has unrelated first targets, even if it later sees canvas.
const g4 = new context.ResizeObserver(() => context.requestAnimationFrame(() => {}));
g4.observe(new Element());g4.observe(root);g4.callback([],g4);
assert.equal(p.frames.size, 0);
root.querySelectorAll('*');
assert.equal(p.roots.get(root).measures, 1);
p.nativeNode = node;
p.beforeNode = {width:294, height:100};
p.nativeBefore = p.observers.filter(o=>o.root===root).map(o=>({o,count:o.nativeDeliveries,nodeCount:0}));
p.hold = true;
p.burst();
assert.equal(p.nativeReady(), false, 'synthetic burst is not native-delivery evidence');
resize.callback([{target:node,contentRect:{width:294,height:100}}],resize);
mutation.callback([{target:root,type:'childList'}],mutation);
assert.equal(p.nativeReady(), false, 'unchanged capped node size is not the required resize stimulus');
node.height = 136;
resize.callback([{target:node,contentRect:{width:294,height:136}}],resize);
assert.equal(p.nativeReady(), true, 'both native observers and observed-node height change required');
const snapshot = p.nativeSnapshot();
assert.equal(snapshot.beforeNode.height, 100);
assert.equal(snapshot.node.height, 136);
assert.equal(snapshot.observers[0].nativeDeliveries, 2);
assert.equal(snapshot.observers[0].deliveries, 6);
assert.equal(snapshot.observers[0].notifications[0].height, 136);
assert.equal(snapshot.observers[1].nativeDeliveries, 1);
assert.equal(p.frames.size, 1);
assert.equal(p.roots.get(root).measures, 1);
const tick = () => {const entries=[...queue];queue.clear();for(const [,callback] of entries)callback(1);};
tick(); // Held native frame does not measure.
assert.equal(p.roots.get(root).measures, 1);
const flushed = p.flush(root);
for (let n=0;n<12;n++) {tick();await Promise.resolve();}
await flushed;
assert.equal(p.roots.get(root).measures, 2);
p.hold = true;p.burst();
closed = true;context.cancelAnimationFrame(pending);resize.disconnect();mutation.disconnect();
const last = [...p.frames.values()].at(-1);
assert.equal(last.canceled, true);
last.callback(1);
assert.equal(p.roots.get(root).measures, 2);
p.restore();
assert.equal(context.requestAnimationFrame, nativeRaf);
assert.equal(context.cancelAnimationFrame, nativeCancel);
assert.equal(context.ResizeObserver, Observer);
console.log('PASS tree frame instrumentation isolation, burst, flush, cancellation and restoration (mock host only)');

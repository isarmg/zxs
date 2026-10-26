// Host instrumentation unit test only, NOT production WASM/browser evidence.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const source = readFileSync(new URL('../runner/tree_frames.js', import.meta.url), 'utf8');
const queue = new Map(), nativeObservers = new Set();
let id = 0;
const nodeList = nodes => Object.assign([...nodes], {item(index) { return this[index] ?? null; }});
const mutationRecord = (target, added, removed) => ({
    target, type: 'childList', addedNodes: nodeList(added), removedNodes: nodeList(removed),
});
const recordMutation = (target, added, removed) => {
    for (const observer of nativeObservers) {
        if (observer.targets.get(target)?.childList)
            observer.records.push(mutationRecord(target, added, removed));
    }
};
class Element {
    constructor(kind = 'node') {
        this.kind = kind; this.width = 294; this.height = 100; this.isConnected = true;
        this.children = []; this.nodeName = 'DIV';
    }
    getBoundingClientRect() { return {width:this.width, height:this.height}; }
    getAttribute(name) {
        if (name === 'class') return this.kind === 'canvas' ? 'zxs-version-tree-canvas'
            : this.kind === 'node' ? 'zxs-version-node' : '';
        return name === 'data-version-id' && this.kind === 'node' ? 'test-version' : null;
    }
    matches(selector) {
        return this.kind === 'canvas' && selector === '.zxs-version-tree-canvas'
            || this.kind === 'node' && selector === '.zxs-version-node';
    }
    querySelectorAll(selector) {
        return selector === '.zxs-version-node[data-version-id]'
            ? this.children.filter(child => child instanceof Element && child.kind === 'node') : [];
    }
    append(child) {
        child.remove?.(); child.parent = this; this.children.push(child);
        recordMutation(this, [child], []);
    }
}
class Comment {
    remove() {
        if (!this.parent) return;
        const parent = this.parent;
        parent.children.splice(parent.children.indexOf(this), 1); this.parent = null;
        recordMutation(parent, [], [this]);
    }
}
class Observer {
    constructor(callback) { this.callback = callback; this.targets = new Map(); this.records = []; nativeObservers.add(this); }
    observe(target, options) { this.targets.set(target, options); }
    unobserve(target) { this.targets.delete(target); }
    takeRecords() { const records = this.records; this.records = []; return records; }
    disconnect() { this.targets.clear(); this.records = []; }
}
const root = new Element('canvas'), node = new Element();
root.children.push(node);
const originalRect = Element.prototype.getBoundingClientRect;
const originalQuery = Element.prototype.querySelectorAll;
const nativeRaf = callback => { queue.set(++id, callback); return id; };
const nativeCancel = id => queue.delete(id);
const context = {Element, innerWidth:390, innerHeight:844, ResizeObserver: Observer, MutationObserver: Observer,
    requestAnimationFrame: nativeRaf, cancelAnimationFrame: nativeCancel,
    document: {querySelector: () => root, createComment: () => new Comment(), fonts: {ready: Promise.resolve()}},
    performance: {now: () => 1}};
context.window = context;
runInNewContext(source, context);
const p = context.__zxsTreeFrames;
// Model the actual measurement operations, not query calls as a proxy for them.
const measure = () => {
    root.getBoundingClientRect();
    for (const member of root.querySelectorAll('.zxs-version-node[data-version-id]')) member.getBoundingClientRect();
};
measure();
let scheduled = false, closed = false, pending;
const request = () => {
    if (scheduled || closed) return;
    pending = context.requestAnimationFrame(() => { if (!closed) {scheduled = false; measure();} });
    scheduled = true;
};
const resize = new context.ResizeObserver(request);
const mutation = new context.MutationObserver(records => {
    if (records.some(record => record.addedNodes.length || record.removedNodes.length)) request();
});
resize.observe(root);resize.observe(node);mutation.observe(root);
const state = p.roots.get(root);
p.assertInitialDiscovery(root);
assert.equal(state.initialQueries, 1);
// Execute the actual geometry consumer through the shared initialization check.
// An observed root with no prior discovery must fail before geometry assertions.
const undiscovered = new Element('canvas');
const earlyObserver = new context.ResizeObserver(() => {});
earlyObserver.observe(undiscovered);
undiscovered.querySelectorAll('.zxs-version-node[data-version-id]');
assert.throws(() => p.assertInitialDiscovery(undiscovered), /initial node discovery synchronous/);
const settle = p.settle, current = p.current;
p.settle = async () => {}; p.current = () => undiscovered;
try {
    const geometry = readFileSync(new URL('../runner/tree_geometry.js', import.meta.url), 'utf8');
    await assert.rejects(runInNewContext(geometry, context), /initial node discovery synchronous/);
} finally { p.settle = settle; p.current = current; earlyObserver.disconnect(); }
assert.equal(state.queries, 1);
assert.equal(state.frameReads.canvas, 0, 'initial synchronous read is not attributed to a tree RAF');
assert.equal(state.frameReads.node, 0);
assert.equal(resize.record.observed, 2);
assert.equal(resize.record.observeCounts.get(node), 1);
assert.equal(resize.record.targets.size, 2);
mutation.record.invoke([]);
assert.equal(p.frames.size, 0, 'empty mutation delivery cannot request work');
// A G4-like observer has unrelated first targets, even if it later sees canvas.
const g4 = new context.ResizeObserver(() => context.requestAnimationFrame(() => {
    root.getBoundingClientRect(); node.getBoundingClientRect();
}));
g4.observe(new Element('other'));g4.observe(root);g4.callback([],g4);
assert.equal(p.frames.size, 0);
root.querySelectorAll('*');
assert.equal(state.queries, 1);
p.nativeNode = node;
p.beforeNode = {width:294, height:100};
p.nativeBefore = p.observers.filter(o=>o.root===root).map(o=>({o,count:o.nativeDeliveries,nodeCount:0}));
p.hold = true;
p.burst();
assert.equal(root.children.length, 1, 'capture fixture appends and removes its comment');
assert.equal(p.nativeReady(), false, 'synthetic burst is not native-delivery evidence');
resize.callback([{target:node,contentRect:{width:294,height:100}}],resize);
mutation.callback([mutationRecord(root, [node], [])],mutation);
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
assert.equal(snapshot.measureQueries, 1);
assert.equal(snapshot.treeObserverGeometryReads, 0);
assert.equal(snapshot.treeFrameGeometryReads.canvas, 0);
assert.equal(p.frames.size, 1);
assert.equal(state.queries, 1);
const tick = () => {const entries=[...queue];queue.clear();for(const [,callback] of entries)callback(1);};
tick(); // G4 reads are excluded; held tree frame does not measure.
assert.equal(state.queries, 1);
assert.equal(state.frameReads.canvas, 0);
assert.equal(state.frameReads.node, 0);
const flushed = p.flush(root);
for (let n=0;n<12;n++) {tick();await Promise.resolve();}
await flushed;
assert.equal(state.queries, 2, 'selector diagnostics remain separate');
assert.equal(state.frameReads.canvas, 1, 'one actual canvas rect read');
assert.equal(state.frameReads.node, 1, 'one actual node rect read');
assert.equal(state.frameReads.level, 0);
assert.equal(state.frameReads.other, 0);
p.hold = true;p.burst();
resize.unobserve(node);
assert.equal(resize.record.unobserved, 1);
assert.equal(resize.record.unobserveCounts.get(node), 1);
assert.equal(resize.record.targets.has(node), false);
closed = true;context.cancelAnimationFrame(pending);resize.disconnect();mutation.disconnect();
assert.equal(resize.record.targets.size, 0);
assert.equal(mutation.record.targets.size, 0);
const last = [...p.frames.values()].at(-1);
assert.equal(last.canceled, true);
last.callback(1);
assert.equal(state.queries, 2);
assert.equal(state.frameReads.canvas, 1);
assert.equal(state.frameReads.node, 1);
p.restore();
assert.equal(context.requestAnimationFrame, nativeRaf);
assert.equal(context.cancelAnimationFrame, nativeCancel);
assert.equal(context.ResizeObserver, Observer);
assert.equal(context.MutationObserver, Observer);
assert.equal(Element.prototype.getBoundingClientRect, originalRect);
assert.equal(Element.prototype.querySelectorAll, originalQuery);
console.log('PASS tree frame geometry/query isolation, native-record capture, resize registration, burst, flush, cancellation and restoration (mock host only)');

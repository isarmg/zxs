// Test-only host instrumentation, installed before WASM imports browser methods.
// Geometry counters cover only identified tree RAF callbacks, not G4 or probe reads.
// Selector calls remain separately labelled diagnostics, never measurement evidence.
(() => {
    const raf = window.requestAnimationFrame, cancel = window.cancelAnimationFrame;
    const query = Element.prototype.querySelectorAll, rect = Element.prototype.getBoundingClientRect;
    const originals = {ResizeObserver, MutationObserver};
    const roots = new Map(), observers = [], frames = new Map();
    let delivery = null, treeFrame = null;
    const rootState = root => {
        if (!roots.has(root)) roots.set(root, {root, queries: 0, registrationQueries: 0, frameReads: {canvas: 0, node: 0, level: 0, other: 0}, observerReads: 0, initialQueries: null});
        return roots.get(root);
    };
    Element.prototype.querySelectorAll = function(selector) {
        if (selector === '.zxs-version-node[data-version-id]' && this.matches('.zxs-version-tree-canvas'))
            rootState(this).queries++;
        if (selector === '.zxs-version-node' && this.matches('.zxs-version-tree-canvas'))
            rootState(this).registrationQueries++;
        return Reflect.apply(query, this, [selector]);
    };
    Element.prototype.getBoundingClientRect = function() {
        if (delivery?.root && !treeFrame) rootState(delivery.root).observerReads++;
        if (treeFrame) {
            const kind = this === treeFrame ? 'canvas'
                : this.matches('.zxs-version-node') ? 'node'
                : this.matches('.zxs-version-tree-level') ? 'level' : 'other';
            rootState(treeFrame).frameReads[kind]++;
        }
        return Reflect.apply(rect, this, []);
    };
    const probe = window.__zxsTreeFrames = {hold: false, roots, observers, frames};
    for (const kind of Object.keys(originals)) {
        const Native = originals[kind];
        window[kind] = class extends Native {
            constructor(callback) {
                const record = {kind, root: null, deliveries: 0, nativeDeliveries: 0, targets: new Set(), nativeTargets: new Map(), disconnected: false, observed: 0, unobserved: 0, observeCounts: new Map(), unobserveCounts: new Map()};
                const invoke = (entries, observer, native = true) => {
                    if (native) {
                        record.nativeDeliveries++;
                        for (const entry of record.root ? entries : []) {
                            const prior = record.nativeTargets.get(entry.target);
                            record.nativeTargets.set(entry.target, {
                                count: (prior?.count ?? 0) + 1,
                                width: entry.contentRect?.width, height: entry.contentRect?.height,
                                type: entry.type,
                            });
                        }
                    }
                    const previous = delivery;
                    delivery = record;
                    try { record.deliveries++; callback(entries, observer); }
                    finally { delivery = previous; }
                };
                super(invoke);
                // Repeated RO delivery is a meaningful notification. MO bursts
                // use actual native records captured by the host fixture below.
                record.invoke = entries => invoke(entries ?? [{target: record.root}], this, false);
                this.record = record;
                observers.push(record);
            }
            observe(target, options) {
                this.record.targets.add(target);
                this.record.observeCounts.set(target, (this.record.observeCounts.get(target) ?? 0) + 1);
                // The tree observes its own canvas first; global G4 observes
                // many unrelated surfaces and must never own a tree frame.
                if (this.record.observed++ === 0 && target.matches?.('.zxs-version-tree-canvas')) {
                    this.record.root = target;
                    const state = rootState(target);
                    if (state.initialQueries === null) state.initialQueries = state.queries;
                }
                return super.observe(target, options);
            }
            unobserve(target) {
                this.record.targets.delete(target);
                this.record.unobserved++;
                this.record.unobserveCounts.set(target, (this.record.unobserveCounts.get(target) ?? 0) + 1);
                return super.unobserve(target);
            }
            disconnect() {
                this.record.disconnected = true;
                this.record.targets.clear();
                return super.disconnect();
            }
        };
    }
    window.requestAnimationFrame = function(callback) {
        const root = delivery?.root;
        if (!root) return Reflect.apply(raf, window, [callback]);
        if (probe.rejectFrame) throw new Error('injected host RAF scheduling failure');
        const frame = {root, canceled: false, ran: false, callback: time => {
            const previous = treeFrame;
            treeFrame = root;
            try { callback(time); } finally { treeFrame = previous; }
        }};
        const id = Reflect.apply(raf, window, [time => {
            if (probe.hold || frame.ran) return;
            frame.ran = true;
            frame.callback(time);
        }]);
        frames.set(id, frame);
        return id;
    };
    window.cancelAnimationFrame = function(id) {
        if (frames.has(id)) frames.get(id).canceled = true;
        return Reflect.apply(cancel, window, [id]);
    };
    probe.pending = root => [...frames.values()].filter(f => f.root === root && !f.ran && !f.canceled);
    probe.current = () => document.querySelector('.zxs-version-tree-canvas');
    const describe = target => {
        const box = target instanceof Element ? Reflect.apply(rect, target, []) : null;
        return {tag: target.nodeName, classes: target.getAttribute?.('class'),
            version: target.getAttribute?.('data-version-id'), connected: target.isConnected,
            width: box?.width, height: box?.height};
    };
    probe.nativeSnapshot = () => ({
        viewport: {width: innerWidth, height: innerHeight},
        root: describe(probe.current()), node: describe(probe.nativeNode),
        before: probe.nativeBefore.map(({o, count, nodeCount}) => ({kind:o.kind, count, nodeCount})),
        beforeNode: probe.beforeNode,
        measureQueries: rootState(probe.current()).queries,
        registrationQueries: rootState(probe.current()).registrationQueries,
        treeFrameGeometryReads: {...rootState(probe.current()).frameReads},
        treeObserverGeometryReads: rootState(probe.current()).observerReads,
        pending: probe.pending(probe.current()).length,
        observers: observers.filter(o => o.root === probe.current()).map(o => ({
            kind: o.kind, nativeDeliveries: o.nativeDeliveries, deliveries: o.deliveries,
            disconnected: o.disconnected, observeCalls: o.observed, unobserveCalls: o.unobserved, targets: [...o.targets].map(describe),
            notifications: [...o.nativeTargets].map(([target, data]) => ({target:describe(target), ...data})),
        })),
    });
    probe.nativeReady = () => probe.nativeBefore.length === 2
        && probe.nativeNode.getBoundingClientRect().height > probe.beforeNode.height
        && probe.nativeBefore.every(({o, count, nodeCount}) => o.nativeDeliveries > count
            && (o.kind !== 'ResizeObserver' || (o.nativeTargets.get(probe.nativeNode)?.count ?? 0) > nodeCount));
    probe.turn = () => new Promise(resolve => Reflect.apply(raf, window, [resolve]));
    probe.settle = async () => { await document.fonts.ready; await probe.turn(); await probe.turn(); await probe.turn(); };
    probe.check = (condition, message) => { if (!condition) throw Error('version-tree RAF: ' + message); };
    probe.rejectScheduling = () => {
        const root = probe.current(), before = rootState(root).frameReads.canvas;
        probe.check(probe.pending(root).length === 0, 'no frame before scheduling rejection');
        const observer = observers.find(o => o.root === root && o.kind === 'ResizeObserver' && !o.disconnected);
        probe.check(!!observer, 'mounted observer for scheduling rejection');
        probe.rejectFrame = true;
        try { observer.invoke(); } finally { probe.rejectFrame = false; }
        probe.check(rootState(root).frameReads.canvas === before, 'failed RAF must not measure synchronously');
        probe.check(probe.pending(root).length === 0, 'failed RAF must not publish a pending frame');
    };
    probe.burst = () => {
        const root = probe.current(), state = rootState(root), before = state.frameReads.canvas;
        const records = observers.filter(o => o.root === root && !o.disconnected);
        probe.check(records.length === 2, 'exact mounted RO/MO pair');
        const count = frames.size;
        const fixture = document.createComment('meaningful synthetic child-list stimulus');
        const capture = new originals.MutationObserver(() => {});
        capture.observe(root, {childList: true});
        root.append(fixture);
        fixture.remove();
        const mutations = capture.takeRecords();
        capture.disconnect();
        probe.check(mutations.length === 2, 'host captured real tree mutation records');
        for (let i = 0; i < 4; i++) for (const observer of records)
            observer.invoke(observer.kind === 'MutationObserver' ? mutations : undefined);
        probe.check(state.frameReads.canvas === before && state.observerReads === 0, 'observer deliveries must defer all geometry reads');
        probe.check(probe.pending(root).length === 1, 'one pending frame after eight RO/MO deliveries');
        probe.check(frames.size - count <= 1, 'burst must not enqueue multiple RAFs');
        return {root, before};
    };
    probe.flush = async root => {
        const pending = probe.pending(root);
        probe.check(pending.length === 1, 'flush exactly one component frame');
        const before = {...rootState(root).frameReads};
        const nodes = Reflect.apply(query, root, ['.zxs-version-node[data-version-id]']).length;
        await new Promise((resolve, reject) => Reflect.apply(raf, window, [time => {
            try {
                pending[0].ran = true;
                pending[0].callback(time);
                resolve();
            } catch (error) { reject(error); }
        }]));
        probe.check(rootState(root).frameReads.canvas === before.canvas + 1, 'one frame reads canvas geometry once');
        probe.check(rootState(root).frameReads.node === before.node + nodes,
            'one frame reads every version node geometry (still full measurement)');
        probe.hold = false;
        await probe.settle();
    };
    // Native MO records exercise the production reconciliation without app hooks.
    // The extra root is a test-only DOM destination, not a second mounted tree.
    probe.registrationMatrix = async () => {
        const root = probe.current(), state = rootState(root);
        const ro = observers.find(o => o.root === root && o.kind === 'ResizeObserver' && !o.disconnected);
        const mo = observers.find(o => o.root === root && o.kind === 'MutationObserver' && !o.disconnected);
        const links = root.querySelector(':scope > .zxs-version-tree-links');
        const batch = async action => { action(); await Promise.resolve(); await Promise.resolve(); };
        const reads = () => JSON.stringify(state.frameReads);
        const drain = async () => {
            if (probe.pending(root).length) await probe.flush(root);
            else { probe.hold = false; await probe.settle(); }
            probe.hold = true;
        };
        probe.check(!!ro && !!mo && !!links, 'mounted tree observers and connector');
        probe.check(ro.targets.size === 1 + Reflect.apply(query, root, ['.zxs-version-node']).length,
            'initial resize targets include canvas and every version node');
        probe.check([...ro.observeCounts.values()].every(count => count === 1),
            'initial resize registration exactly once per target');
        probe.hold = true;
        const beforeEmpty = frames.size, beforeOwned = reads(), registrationQueries = state.registrationQueries;
        mo.invoke([]);
        probe.check(frames.size === beforeEmpty && probe.pending(root).length === 0,
            'empty MO batch cannot force a frame');
        const output = document.createElementNS('http://www.w3.org/2000/svg', 'g');
        await batch(() => { links.append(output); output.remove(); });
        await batch(() => { const next = links.nextSibling; links.remove(); root.insertBefore(links, next); });
        probe.check(frames.size === beforeEmpty && reads() === beforeOwned && state.observerReads === 0,
            'owned SVG child and root-level output never schedule or read geometry');
        const real = document.createComment('mixed real change');
        await batch(() => { links.append(output); root.append(real); });
        probe.check(probe.pending(root).length === 1, 'mixed owned and real mutations schedule');
        await batch(() => { output.remove(); real.remove(); });
        await drain();
        const mixedCapture = new originals.MutationObserver(() => {});
        mixedCapture.observe(root, {childList: true});
        const linksNext = links.nextSibling;
        let mixedRecords;
        await batch(() => {
            root.append(links, real);
            mixedRecords = mixedCapture.takeRecords();
        });
        mixedCapture.disconnect();
        probe.check(mixedRecords.some(record => record.addedNodes.length === 2
            && record.addedNodes[0] === links && record.addedNodes[1] === real),
            'native fixture supplies one record containing owned and real additions');
        probe.check(probe.pending(root).length === 1, 'single mixed record retains its real mutation');
        await batch(() => { real.remove(); root.insertBefore(links, linksNext); });
        await drain();
        const fake = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        fake.setAttribute('class', 'zxs-version-tree-links');
        await batch(() => root.append(fake));
        probe.check(probe.pending(root).length === 1, 'lookalike connector class cannot claim ownership');
        await drain();
        await batch(() => fake.append(document.createElementNS('http://www.w3.org/2000/svg', 'g')));
        probe.check(probe.pending(root).length === 1, 'lookalike SVG descendants are real mutations');
        await batch(() => fake.remove());
        await drain();
        const retainedObserves = ro.observed, retainedUnobserves = ro.unobserved;
        const textWalker = document.createTreeWalker(root.querySelector('.zxs-version-node'), NodeFilter.SHOW_TEXT);
        let text;
        while ((text = textWalker.nextNode()) && !text.data.trim()) {}
        probe.check(!!text, 'real version node contains text for same-size replacement');
        await batch(() => {
            const replacement = text.cloneNode();
            text.replaceWith(replacement);
            replacement.replaceWith(text);
        });
        probe.check(probe.pending(root).length === 1, 'same-size real text replacement schedules measurement');
        probe.check(ro.observed === retainedObserves && ro.unobserved === retainedUnobserves,
            'same-size text replacement does no registration work');
        await drain();
        const wrapper = document.createElement('div'), node = document.createElement('div');
        wrapper.hidden = true;
        node.className = 'zxs-version-node';
        wrapper.append(node);
        const observedBefore = ro.observed, removedBefore = ro.unobserved;
        await batch(() => root.append(wrapper));
        probe.check(ro.targets.has(node) && ro.observeCounts.get(node) === 1,
            'new nested version node is observed at MO delivery before RAF');
        probe.check(ro.observed === observedBefore + 1, 'new subtree does not reobserve retained nodes');
        await drain();
        const second = document.createElement('div');
        second.hidden = true;
        await batch(() => { root.append(second); second.append(node); wrapper.append(node); });
        probe.check(ro.observeCounts.get(node) === 1 && !ro.unobserveCounts.has(node),
            'same-root batch reparent preserves registration');
        await drain();
        const outside = document.createElement('div');
        outside.className = 'zxs-version-tree-canvas';
        outside.hidden = true;
        document.body.append(outside);
        await batch(() => outside.append(node));
        probe.check(node.isConnected && !ro.targets.has(node) && ro.unobserveCounts.get(node) === 1,
            'cross-root connected move releases source-root observation');
        await drain();
        await batch(() => wrapper.append(node));
        probe.check(ro.targets.has(node) && ro.observeCounts.get(node) === 2,
            'return from another root registers once again');
        await drain();
        await batch(() => { node.className = 'former-version-node'; wrapper.remove(); });
        probe.check(!ro.targets.has(node) && ro.unobserveCounts.get(node) === 2,
            'removed subtree releases identity even after version class changes');
        probe.check(ro.observed === observedBefore + 2 && ro.unobserved === removedBefore + 2,
            'registration work equals actual membership changes');
        await batch(() => { second.remove(); outside.remove(); });
        await drain();
        const detachedParent = document.createElement('div'), inner = document.createElement('div');
        const extracted = document.createElement('div');
        detachedParent.hidden = true;
        extracted.className = 'zxs-version-node';
        inner.append(extracted);detachedParent.append(inner);
        await batch(() => root.append(detachedParent));
        probe.check(ro.targets.has(extracted) && ro.observeCounts.get(extracted) === 1,
            'nested transient-removal fixture is registered');
        await drain();
        const transientCapture = new originals.MutationObserver(() => {});
        transientCapture.observe(root, {childList: true, subtree: true});
        let transientRecords;
        await batch(() => {
            detachedParent.remove();
            extracted.remove();
            transientRecords = transientCapture.takeRecords();
        });
        transientCapture.disconnect();
        probe.check(!detachedParent.contains(extracted)
            && transientRecords.some(record => record.target === inner
                && [...record.removedNodes].includes(extracted)),
            'native transient record retains descendant extracted after parent removal');
        probe.check(!ro.targets.has(extracted) && ro.unobserveCounts.get(extracted) === 1,
            'transient detached-subtree record releases extracted descendant');
        await drain();
        probe.check(state.registrationQueries === registrationQueries,
            'mutation batches never repeat whole-root node discovery');
        probe.check(state.observerReads === 0, 'native mutation reconciliation does not read geometry');
        probe.hold = false;
        return {observeCalls: ro.observed - observedBefore, unobserveCalls: ro.unobserved - removedBefore,
            activeTargets: ro.targets.size, treeFrameGeometryReads: {...state.frameReads},
            measureQueries: state.queries, registrationQueries: state.registrationQueries};
    };
    probe.restore = () => {
        probe.hold = false;
        probe.rejectFrame = false;
        window.requestAnimationFrame = raf;
        window.cancelAnimationFrame = cancel;
        Element.prototype.querySelectorAll = query;
        Element.prototype.getBoundingClientRect = rect;
        Object.assign(window, originals);
    };
})();

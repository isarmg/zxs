// Test-only host instrumentation, installed before WASM imports browser methods.
// G4 reads are excluded: measure's exact canvas/node selector is the witness.
(() => {
    const raf = window.requestAnimationFrame, cancel = window.cancelAnimationFrame;
    const query = Element.prototype.querySelectorAll;
    const originals = {ResizeObserver, MutationObserver};
    const roots = new Map(), observers = [], frames = new Map();
    let delivery = null;
    const rootState = root => {
        if (!roots.has(root)) roots.set(root, {root, measures: 0, initial: null});
        return roots.get(root);
    };
    Element.prototype.querySelectorAll = function(selector) {
        if (selector === '.zxs-version-node[data-version-id]' && this.matches('.zxs-version-tree-canvas'))
            rootState(this).measures++;
        return Reflect.apply(query, this, [selector]);
    };
    const probe = window.__zxsTreeFrames = {hold: false, roots, observers, frames};
    for (const kind of Object.keys(originals)) {
        const Native = originals[kind];
        window[kind] = class extends Native {
            constructor(callback) {
                const record = {kind, root: null, deliveries: 0, disconnected: false, observed: 0};
                const invoke = (entries, observer) => {
                    const previous = delivery;
                    delivery = record;
                    try { record.deliveries++; callback(entries, observer); }
                    finally { delivery = previous; }
                };
                super(invoke);
                record.invoke = () => invoke([], this);
                this.record = record;
                observers.push(record);
            }
            observe(target, options) {
                // The tree observes its own canvas first; global G4 observes
                // many unrelated surfaces and must never own a tree frame.
                if (this.record.observed++ === 0 && target.matches?.('.zxs-version-tree-canvas')) {
                    this.record.root = target;
                    const state = rootState(target);
                    if (state.initial === null) state.initial = state.measures;
                }
                return super.observe(target, options);
            }
            disconnect() { this.record.disconnected = true; return super.disconnect(); }
        };
    }
    window.requestAnimationFrame = function(callback) {
        const root = delivery?.root;
        if (!root) return Reflect.apply(raf, window, [callback]);
        const frame = {root, callback, canceled: false, ran: false};
        const id = Reflect.apply(raf, window, [time => {
            if (probe.hold || frame.ran) return;
            frame.ran = true;
            callback(time);
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
    probe.turn = () => new Promise(resolve => Reflect.apply(raf, window, [resolve]));
    probe.settle = async () => { await document.fonts.ready; await probe.turn(); await probe.turn(); await probe.turn(); };
    probe.check = (condition, message) => { if (!condition) throw Error('version-tree RAF: ' + message); };
    probe.burst = () => {
        const root = probe.current(), state = rootState(root), before = state.measures;
        const records = observers.filter(o => o.root === root && !o.disconnected);
        probe.check(records.length === 2, 'exact mounted RO/MO pair');
        const count = frames.size;
        for (let i = 0; i < 4; i++) for (const observer of records) observer.invoke();
        probe.check(state.measures === before, 'observer deliveries must defer measure');
        probe.check(probe.pending(root).length === 1, 'one pending frame after eight RO/MO deliveries');
        probe.check(frames.size - count <= 1, 'burst must not enqueue multiple RAFs');
        return {root, before};
    };
    probe.flush = async root => {
        const pending = probe.pending(root);
        probe.check(pending.length === 1, 'flush exactly one component frame');
        const before = rootState(root).measures;
        await new Promise((resolve, reject) => Reflect.apply(raf, window, [time => {
            try {
                pending[0].ran = true;
                pending[0].callback(time);
                resolve();
            } catch (error) { reject(error); }
        }]));
        probe.check(rootState(root).measures === before + 1, 'one frame performs exactly one measure');
        probe.hold = false;
        await probe.settle();
    };
    probe.restore = () => {
        probe.hold = false;
        window.requestAnimationFrame = raf;
        window.cancelAnimationFrame = cancel;
        Element.prototype.querySelectorAll = query;
        Object.assign(window, originals);
    };
})();

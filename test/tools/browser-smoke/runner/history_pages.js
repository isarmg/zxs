// Synthetic protocol delivery only. The production WASM owns all model,
// selection, editor, cancellation and DOM behavior exercised by the runner.
const original = window.fetch;
const NativeResizeObserver = window.ResizeObserver;
const NativeMutationObserver = window.MutationObserver;
const probe = window.__zxsHistoryPages = {
    seed: null, root: null, calls: [], writes: [], pending: [], aborted: 0,
    branchFailure: null, holdBranches: false, delivered: 0,
    observers: [],
    restore() {
        window.fetch = original;
        window.ResizeObserver = NativeResizeObserver;
        window.MutationObserver = NativeMutationObserver;
        for (const pending of this.pending) pending.release();
    },
};
for (const [name, Native] of [['ResizeObserver', NativeResizeObserver], ['MutationObserver', NativeMutationObserver]]) {
    window[name] = class extends Native {
        constructor(callback) { super(callback); this.record = {name, root: null, targets: new Set(), disconnected: false}; probe.observers.push(this.record); }
        observe(target, options) {
            if (target.matches?.('.zxs-version-tree-canvas')) this.record.root = target;
            this.record.targets.add(target);
            return super.observe(target, options);
        }
        unobserve(target) { this.record.targets.delete(target); return super.unobserve(target); }
        disconnect() { this.record.disconnected = true; this.record.targets.clear(); return super.disconnect(); }
    };
}
const response = body => new Response(JSON.stringify(body), {status: 200, headers: {'Content-Type': 'application/json'}});
const rejected = (status, message) => new Response(JSON.stringify({code: 'INVALID_REQUEST', message, retryable: false}), {status, headers: {'Content-Type': 'application/json'}});
const nodeId = sequence => `history-node-${String(sequence).padStart(3, '0')}`;
const branchId = index => `history-branch-${String(index).padStart(3, '0')}`;
const pageOffset = (cursor, prefix) => cursor?.startsWith(prefix) ? Number(cursor.slice(prefix.length)) : 0;
const header = () => ({file_id: probe.seed.file_id, history_tree_id: probe.seed.history_tree_id, history_root: probe.root});

window.fetch = async function (...args) {
    const request = args[0], url = new URL(typeof request === 'string' ? request : request.url, location.href);
    if (!/^\/api\/file-manager\/files\/[^/]+\//.test(url.pathname)) return original.apply(this, args);
    const method = request.method || 'GET';
    if (method !== 'GET') {
        probe.writes.push({path: url.pathname, body: await request.clone().json()});
        return rejected(409, 'Synthetic browser action capture; no mutation was published');
    }
    if (!url.pathname.includes('/history')) return original.apply(this, args);
    probe.calls.push({path: url.pathname, cursor: url.searchParams.get('cursor'), root: url.searchParams.get('history_root'), anchor: url.searchParams.get('anchor_node_id')});
    if (!probe.seed) {
        const seed = await original.apply(this, args);
        if (!seed.ok || !url.pathname.endsWith('/history')) throw new Error('A real first history response must seed this fixture');
        probe.seed = await seed.json();
        probe.root = probe.seed.history_root;
    }
    if (url.searchParams.has('history_root') && url.searchParams.get('history_root') !== probe.root) {
        return new Response(JSON.stringify({code: 'HISTORY_CHANGED', message: 'Synthetic root changed', retryable: false}), {status: 409, headers: {'Content-Type': 'application/json'}});
    }
    if (url.pathname.endsWith('/history')) {
        const anchor = url.searchParams.get('anchor_node_id');
        const offset = anchor?.startsWith('history-node-') ? 200 - Number(anchor.slice(-3)) : pageOffset(url.searchParams.get('cursor'), 'nodes:');
        const sequences = Array.from({length: Math.min(50, 101 - offset)}, (_, index) => 200 - offset - index);
        const resident = new Set(sequences.map(nodeId));
        const base = probe.seed.nodes[0];
        const nodes = sequences.map(sequence => ({...base, node_id: nodeId(sequence), version_id: nodeId(sequence), sequence: String(sequence),
            branch_id: branchId(0), current: sequence === 200, note: `Node ${sequence}`,
            parents: sequence === sequences[0]
                ? {status: 'more', edges: [{node_id: 'outside-parent', in_window: false}], cursor: 'parents:0'}
                : {status: 'complete', edges: sequence > 100 ? [{node_id: nodeId(sequence - 1), in_window: resident.has(nodeId(sequence - 1))}] : [], cursor: null},
        }));
        return response({...probe.seed, history_root: probe.root,
            context: {...probe.seed.context, current_node_id: nodeId(200), selected_branch_id: branchId(0), version_id: nodeId(200), own_version_id: nodeId(200)},
            nodes, previous_cursor: offset ? `nodes:${Math.max(0, offset - 50)}` : null,
            next_cursor: offset + nodes.length < 101 ? `nodes:${offset + nodes.length}` : null,
            anchor: anchor ? {node_id: anchor, in_page: resident.has(anchor)} : null,
            budget: {limit: 50, parent_edges: 2048},
        });
    }
    if (url.pathname.endsWith('/branches')) {
        const capturedHeader = header();
        const offset = pageOffset(url.searchParams.get('cursor'), 'branches:');
        const branches = Array.from({length: Math.min(100, 201 - offset)}, (_, position) => {
            const index = offset + position;
            return {branch_id: branchId(index), history_tree_id: probe.seed.history_tree_id, name: `Branch ${String(index).padStart(3, '0')}`,
                head_commit_id: `commit-${index}`, fork_commit_id: 'fork', branch_kind: 'file', file_id: probe.seed.file_id,
                generation: '1', state: 'active', materialized: true, inherited: false, content_state: 'LIVE',
                version_id: nodeId(200), own_version_id: nodeId(200), source_version_id: null, visible_content_state: 'LIVE',
                read_only: false, alias_of: null, node_id: nodeId(200), current: index === 0, pending_count: 0};
        });
        if (probe.holdBranches) {
            let release;
            const gate = new Promise(resolve => release = resolve);
            probe.pending.push({release, root: capturedHeader.history_root});
            request.signal.addEventListener('abort', () => probe.aborted++, {once: true});
            await gate; // Intentionally deliver after cancellation to test the fence.
            probe.delivered++;
        }
        if (probe.branchFailure === 'json') return new Response('{broken', {status: 200, headers: {'Content-Type': 'application/json'}});
        if (probe.branchFailure) return rejected(probe.branchFailure, 'Synthetic persistent branch failure');
        return response({...capturedHeader, selected_branch_id: branchId(0), branches,
            previous_cursor: offset ? `branches:${Math.max(0, offset - 100)}` : null,
            next_cursor: offset + branches.length < 201 ? `branches:${offset + branches.length}` : null});
    }
    if (/\/nodes\/[^/]+\/parents$/.test(url.pathname)) {
        const offset = pageOffset(url.searchParams.get('cursor'), 'parents:');
        return response({...header(), node_id: decodeURIComponent(url.pathname.split('/').at(-2)),
            parents: Array.from({length: Math.min(100, 201 - offset)}, (_, index) => ({node_id: `parent-${String(offset + index).padStart(3, '0')}`})),
            previous_cursor: offset ? `parents:${Math.max(0, offset - 100)}` : null,
            next_cursor: offset < 200 ? `parents:${offset + 100}` : null});
    }
    return original.apply(this, args);
};
return true;

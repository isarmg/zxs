// Observe real requests and responses, without recording authentication headers
// or replacing business data. A held real POST receipt tests duplicate clicks.
if (window.__zxsPhase1Tags) throw new Error('Tag probe already installed');
const original = window.fetch;
const canonical = value => Array.isArray(value) ? value.map(canonical)
    : value !== null && typeof value === 'object'
        ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;
const same = (a, b) => JSON.stringify(canonical(a)) === JSON.stringify(canonical(b));
const p = window.__zxsPhase1Tags = {
    reads: [], writes: [], unexpected: [], base: null, snapshots: [], closed: false,
    label: '一期已审核',
    release() { for (const write of p.writes) write.release?.(); },
    snapshot() {
        return {scope: 'observed real tag request, receipt and history; no authentication headers',
            base: p.base, snapshots: p.snapshots, readCount: p.reads.length,
            lastHistory: p.reads.at(-1)?.body ?? null,
            writes: p.writes.map(({release, ...write}) => write), unexpected: p.unexpected};
    },
    restore() {
        p.closed = true;
        window.fetch = original;
        p.release();
        delete window.__zxsPhase1Tags;
    },
    capture() {
        const page = p.reads.at(-1)?.body;
        const node = page?.nodes?.find(node => node.node_id === page.identity.context.current_node_id);
        if (!node || page.kind !== 'nodes' || page.page.next_cursor !== null
            || page.page.previous_cursor !== null || !page.identity.context.selected_branch_id
            || page.identity.context.path !== 'browser-smoke.txt' || node.tags.includes(p.label))
            throw new Error('tag fixture needs a complete real writable current file node');
        p.base = page;
        p.snapshots.push(page);
        return node.node_id;
    },
    verify(index) {
        const write = p.writes[index], page = p.reads.at(-1)?.body;
        if (!write?.delivered || !page || p.unexpected.length || p.writes.length !== index + 1)
            return false;
        const before = p.snapshots[index], identity = before.identity, context = identity.context;
        const source = before.nodes.find(node => node.node_id === context.current_node_id);
        const expected = {targets: [{source_node_id: source.node_id,
            branch_id: context.selected_branch_id, expected_node_id: source.node_id}],
            add: index === 0 ? [p.label] : [], remove: index === 0 ? [] : [p.label],
            repository_id: context.repository_id, path: context.path};
        const {operation_id, ...body} = write.body;
        // Compare named fields, not property insertion order.
        if (Object.keys(body).length !== Object.keys(expected).length
            || Object.entries(expected).some(([key, value]) => !same(body[key], value))
            || !/^op_[0-9a-f]{32}$/.test(operation_id)
            || write.path !== `/api/file-manager/files/${identity.file_id}/tags`
            || write.status !== 200 || write.receipt?.state !== 'completed'
            || write.receipt.operation_kind !== 'file.tags.update'
            || write.receipt.operation_id !== operation_id || write.receipt.error !== null
            || write.receipt.result?.file_id !== identity.file_id
            || write.receipt.result?.content_versions_created !== '0')
            throw new Error('tag write did not preserve the reviewed identity or complete receipt');
        if (page.identity.file_id !== identity.file_id
            || page.identity.context.repository_id !== context.repository_id
            || page.identity.context.path !== context.path
            || page.identity.context.selected_branch_id !== context.selected_branch_id
            || page.identity.context.history_tree_id !== context.history_tree_id)
            throw new Error('tag refresh changed the selected file identity');
        const node = page.nodes.find(node => node.node_id === page.identity.context.current_node_id);
        if (!node || node.node_id === source.node_id) return false;
        const expectedTags = index === 0 ? [...source.tags, p.label].sort()
            : source.tags.filter(label => label !== p.label);
        if (page.nodes.length !== before.nodes.length + 1 || node.kind !== 'tags'
            || node.version_id !== source.version_id || node.size !== source.size
            || node.content_hash !== source.content_hash || !same(node.tags, expectedTags)
            || !page.edges.some(edge => edge.node_id === node.node_id
                && edge.parent_node_id === source.node_id && edge.endpoint === 'in_window'))
            throw new Error('tag save did not create exactly one child retaining the original body');
        const results = write.receipt.result.nodes;
        if (!Array.isArray(results) || results.length !== 1
            || results[0].node_id !== node.node_id || results[0].source_node_id !== source.node_id
            || results[0].branch_id !== context.selected_branch_id
            || results[0].no_change !== false || results[0].advanced !== true
            || !same(results[0].tags, node.tags))
            throw new Error('tag child does not match its real publication receipt');
        for (const old of before.nodes) {
            const retained = page.nodes.find(node => node.node_id === old.node_id);
            if (!retained) throw new Error('tag save removed an earlier node');
            for (const key of Object.keys(old).filter(key => key !== 'current')) {
                if (!same(old[key], retained[key])) throw new Error('tag save changed an earlier node');
            }
        }
        if (page.page.previous_cursor !== null || page.page.next_cursor !== null)
            throw new Error('small tag fixture unexpectedly needs another history page');
        p.snapshots[index + 1] = page;
        return node.node_id;
    },
    reread(index, after) {
        if (p.reads.length <= after) return false;
        const page = p.reads.at(-1)?.body, saved = p.snapshots[index + 1];
        if (!page || !saved) return false;
        // Stable publication/context and complete graph must survive navigation.
        // A new read may have a different request-local query hash or limit.
        if (page.identity.file_id !== saved.identity.file_id
            || page.identity.history_root !== saved.identity.history_root
            || !same(page.identity.context, saved.identity.context)
            || !same(page.nodes, saved.nodes) || !same(page.edges, saved.edges)
            || page.page.previous_cursor !== null || page.page.next_cursor !== null)
            throw new Error('fresh history did not retain the saved tag publication');
        return page.identity.context.current_node_id;
    }
};
window.fetch = async function(...args) {
    const input = args[0];
    const request = new Request(input instanceof Request ? input.clone() : input, args[1]);
    const url = new URL(request.url, location.href), own = url.origin === location.origin;
    const tags = own && request.method === 'POST' && /^\/api\/file-manager\/files\/[^/]+\/tags$/.test(url.pathname);
    const history = own && request.method === 'GET' && /^\/api\/file-manager\/files\/[^/]+\/history$/.test(url.pathname);
    const write = tags ? {path: url.pathname, body: await request.clone().json(), delivered: false} : null;
    if (write) p.writes.push(write);
    else if (own && url.pathname.startsWith('/api/') && !['GET', 'HEAD'].includes(request.method))
        p.unexpected.push({method: request.method, path: url.pathname});
    const response = await Reflect.apply(original, this, args);
    if (history) {
        const body = await response.clone().json();
        if (response.ok && body.kind === 'nodes') p.reads.push({body});
    }
    if (write) {
        write.status = response.status;
        write.receipt = await response.clone().json();
        if (!p.closed) await new Promise(resolve => { write.release = resolve; });
        write.delivered = true;
    }
    return response;
};
return true;

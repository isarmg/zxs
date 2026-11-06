// Faults are applied only after observing the real matching service response.
// No success receipt, history node, authentication header or private UI state
// is manufactured. A retry receives its original real Response unchanged.
if (window.__zxsTagUnknown) throw new Error('Tag recovery probe already installed');
const original = window.fetch;
const canonical = value => Array.isArray(value) ? value.map(canonical)
    : value !== null && typeof value === 'object'
        ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;
const same = (a, b) => JSON.stringify(canonical(a)) === JSON.stringify(canonical(b));
const p = window.__zxsTagUnknown = {
    label: '恢复原请求', marker: 'Browser smoke tag recovery fault',
    reads: [], writes: [], outcomes: [], unexpected: [], stages: [], base: null, closed: false,
    capture() {
        const page = p.reads.at(-1);
        const source = page?.nodes?.find(node => node.node_id === page.identity.context.current_node_id);
        if (!source || page.kind !== 'nodes' || page.page.next_cursor !== null
            || page.page.previous_cursor !== null || !page.identity.context.selected_branch_id
            || page.identity.context.path !== 'browser-smoke.txt' || source.tags.includes(p.label))
            throw new Error('tag recovery requires a complete real writable current history');
        p.base = page;
        return source.node_id;
    },
    published(write) {
        const receipt = write.receipt;
        const context = p.base?.identity.context;
        if (!context) return false;
        const {operation_id, ...body} = write.body;
        const expected = {repository_id: context.repository_id, path: context.path,
            targets: [{branch_id: context.selected_branch_id,
                source_node_id: context.current_node_id, expected_node_id: context.current_node_id}],
            add: [p.label], remove: []};
        return write.method === 'POST'
            && write.url === `${location.origin}/api/file-manager/files/${p.base.identity.file_id}/tags`
            && /^op_[0-9a-f]{32}$/.test(operation_id) && same(body, expected)
            && write.status === 200 && receipt?.operation_id === operation_id
            && receipt.operation_kind === 'file.tags.update' && receipt.state === 'completed'
            && receipt.commit_state === 'PUBLISHED' && receipt.error === null
            && receipt.protection_at_publish === 'NOT_APPLICABLE'
            && /^[0-9a-f]{64}$/.test(receipt.request_digest)
            && /^[1-9][0-9]*$/.test(receipt.publication_sequence)
            && receipt.result?.file_id === p.base?.identity.file_id
            && receipt.result?.content_versions_created === '0';
    },
    uncertain() {
        const first = p.writes[0];
        if (first?.status !== undefined && !first.lost)
            throw new Error(`real first tag response was not a matching publication: HTTP ${first.status}`);
        return p.writes.length === 1 && first?.lost && p.outcomes.filter(read => read.hidden404).length >= 2;
    },
    replay() {
        if (p.writes.length > 2 || p.unexpected.length) throw new Error('unexpected write during tag recovery');
        const [first, retry] = p.writes;
        if (!retry?.receipt) return false;
        if (!first.lost || first.method !== retry.method || first.url !== retry.url
            || first.text !== retry.text || !same(first.body, retry.body))
            throw new Error('retry changed the frozen method, URL, operation ID or complete body');
        if (!p.published(retry) || !same(first.receipt, retry.receipt))
            throw new Error('retry did not return the same real published operation receipt');
        if (p.outcomes.length !== retry.outcomesAtStart)
            throw new Error('outcome polling overlapped the tag retry');
        return true;
    },
    verify() {
        if (!p.replay() || !p.writes[1].delivered) return false;
        const page = p.reads.at(-1), before = p.base;
        if (!page || page === before) return false;
        const source = before.nodes.find(node => node.node_id === before.identity.context.current_node_id);
        const result = p.writes[1].receipt.result.nodes;
        if (!Array.isArray(result) || result.length !== 1) throw new Error('retry has no single tag child receipt');
        const id = result[0].node_id, node = page.nodes.find(node => node.node_id === id);
        if (!node) return false;
        if (page.identity.file_id !== before.identity.file_id
            || page.identity.context.repository_id !== before.identity.context.repository_id
            || page.identity.context.path !== before.identity.context.path
            || page.identity.context.history_tree_id !== before.identity.context.history_tree_id
            || page.identity.context.selected_branch_id !== before.identity.context.selected_branch_id
            || page.identity.context.current_node_id !== id
            || page.page.next_cursor !== null || page.page.previous_cursor !== null)
            throw new Error('recovered history does not belong to the frozen file context');
        if (page.nodes.length !== before.nodes.length + 1 || node.kind !== 'tags'
            || node.version_id !== source.version_id || node.size !== source.size
            || node.content_hash !== source.content_hash || !same(node.tags, [...source.tags, p.label].sort())
            || result[0].source_node_id !== source.node_id || result[0].branch_id !== before.identity.context.selected_branch_id
            || result[0].no_change !== false || result[0].advanced !== true || !same(result[0].tags, node.tags)
            || !page.edges.some(edge => edge.node_id === id && edge.parent_node_id === source.node_id && edge.endpoint === 'in_window'))
            throw new Error('replay created another child or changed the original body');
        for (const old of before.nodes) {
            const retained = page.nodes.find(node => node.node_id === old.node_id);
            if (!retained || Object.keys(old).filter(key => key !== 'current').some(key => !same(old[key], retained[key])))
                throw new Error('recovery changed an earlier node');
        }
        if (before.edges.some(edge => !page.edges.some(retained => same(edge, retained))))
            throw new Error('recovery removed an earlier parent edge');
        return id;
    },
    release() { for (const write of p.writes) write.release?.(); },
    snapshot() {
        return {scope: 'real published POST loses delivery; matching real outcomes are hidden by synthetic 404; exact original POST retry',
            base: p.base, reads: p.reads, outcomes: p.outcomes, stages: p.stages,
            writes: p.writes.map(({release, ...write}) => write), unexpected: p.unexpected};
    },
    restore() { p.closed = true; window.fetch = original; p.release(); delete window.__zxsTagUnknown; }
};
const observedJson = async (response, record) => {
    try { return await response.clone().json(); }
    catch { record.invalidJson = true; return null; }
};
window.fetch = async function(...args) {
    const input = args[0];
    const request = new Request(input instanceof Request ? input.clone() : input, args[1]);
    const url = new URL(request.url, location.href), own = url.origin === location.origin;
    const tags = own && request.method === 'POST' && /^\/api\/file-manager\/files\/[^/]+\/tags$/.test(url.pathname);
    const history = own && request.method === 'GET' && /^\/api\/file-manager\/files\/[^/]+\/history$/.test(url.pathname);
    const first = p.writes[0];
    const outcome = own && request.method === 'GET' && first?.lost
        && url.pathname === `/api/operations/${first.body.operation_id}/outcome`;
    let write = null, read = null;
    if (tags) {
        const text = await request.clone().text();
        write = {method: request.method, url: url.href, text, body: JSON.parse(text),
            delivered: false, lost: false, outcomesAtStart: p.outcomes.length};
        p.writes.push(write);
    } else if (outcome) {
        read = {status: null, hidden404: false}; p.outcomes.push(read);
    } else if (own && url.pathname.startsWith('/api/') && !['GET', 'HEAD'].includes(request.method)) {
        p.unexpected.push({method: request.method, path: url.pathname});
    }
    const response = await Reflect.apply(original, this, args);
    if (history && response.ok) {
        const body = await observedJson(response, {});
        if (body?.kind === 'nodes') p.reads.push(body);
    }
    if (write) {
        write.status = response.status;
        write.receipt = await observedJson(response, write);
        if (!p.closed && p.writes[0] === write && p.published(write)) {
            write.lost = true;
            throw new TypeError(`${p.marker}: published POST response delivery lost`);
        }
        if (!p.closed && p.writes[1] === write && p.published(write))
            await new Promise(resolve => { write.release = resolve; });
        write.delivered = true;
    }
    if (read) {
        read.status = response.status;
        read.receipt = await observedJson(response, read);
        // Preserve actual 429, authentication errors and malformed responses.
        // A synthetic 404 is allowed only over this exact completed operation.
        if (!p.closed && read.status === 200 && same(read.receipt, first.receipt)) {
            const headers = new Headers(response.headers); headers.delete('content-length');
            headers.set('content-type', 'application/json'); read.hidden404 = true;
            return new Response(JSON.stringify({error:{code:'NOT_FOUND',message:`${p.marker}: outcome temporarily hidden`,retryable:false}}), {status:404,headers});
        }
    }
    return response;
};
return true;

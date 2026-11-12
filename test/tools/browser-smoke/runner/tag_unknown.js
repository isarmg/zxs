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
    reads: [], tagReads: [], baseTags: null, writes: [], outcomes: [], unexpected: [], stages: [], base: null, closed: false,
    capture() {
        const page = p.reads.at(-1), tags = p.tagReads.at(-1)?.body;
        const source = page?.nodes?.find(node => node.node_id === page.identity.context.current_node_id);
        if (!source || page.kind !== 'nodes' || page.page.next_cursor !== null
            || page.page.previous_cursor !== null || !page.identity.context.selected_branch_id
            || page.identity.context.path !== 'browser-smoke.txt'
            || tags?.file_id !== page.identity.file_id
            || tags.context?.repository_id !== page.identity.context.repository_id
            || tags.context?.path !== page.identity.context.path || !Array.isArray(tags.tags)
            || !/^(0|[1-9][0-9]*)$/.test(tags.generation) || tags.tags.includes(p.label))
            throw new Error('tag recovery requires complete real writable history and container tags');
        p.base = page;
        p.baseTags = tags;
        return source.node_id;
    },
    published(write) {
        const receipt = write.receipt;
        const context = p.base?.identity.context, tags = p.baseTags;
        if (!context || !tags) return false;
        const {operation_id, ...body} = write.body;
        const wanted = [...tags.tags, p.label].sort();
        const expected = {repository_id: context.repository_id, path: context.path,
            expected_generation: tags.generation, tags: wanted};
        const command = {file_id: p.base.identity.file_id,
            context: {repository_id: context.repository_id, path: context.path},
            expected_generation: tags.generation, tags: wanted};
        const replacement = {request: command, tags: wanted,
            generation: (BigInt(tags.generation) + 1n).toString(), changed: true};
        return write.method === 'POST'
            && write.url === `${location.origin}/api/file-manager/files/${p.base.identity.file_id}/tags`
            && /^op_[0-9a-f]{32}$/.test(operation_id) && same(body, expected)
            && write.status === 200 && receipt?.operation_id === operation_id
            && receipt.operation_kind === 'file.container-tags.replace' && receipt.state === 'completed'
            && receipt.commit_state === 'PUBLISHED' && receipt.error === null
            && receipt.protection_at_publish === 'NOT_APPLICABLE'
            && /^[0-9a-f]{64}$/.test(receipt.request_digest)
            && /^[1-9][0-9]*$/.test(receipt.publication_sequence)
            && same(receipt.result, replacement);
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
        const page = p.reads.at(-1), before = p.base, tags = p.tagReads.at(-1)?.body;
        const retry = p.writes[1], result = retry.receipt.result;
        if (!page || p.reads.length <= retry.historyReadsAtStart
            || p.tagReads.length <= retry.tagReadsAtStart) return false;
        if (page.identity.file_id !== before.identity.file_id
            || !same(page.identity.context, before.identity.context)
            || page.page.next_cursor !== null || page.page.previous_cursor !== null)
            throw new Error('recovered history does not belong to the frozen file context');
        if (!same(page.nodes, before.nodes) || !same(page.edges, before.edges))
            throw new Error('recovery changed the graph or original body');
        const expected = {file_id: result.request.file_id, context: result.request.context,
            tags: result.tags, generation: result.generation};
        if (!same(tags, expected))
            throw new Error('recovery changed the container CAS result');
        return page.identity.context.current_node_id;
    },
    release() { for (const write of p.writes) write.release?.(); },
    snapshot() {
        return {scope: 'real published POST loses delivery; matching real outcomes are hidden by synthetic 404; exact original POST retry',
            base: p.base, baseTags: p.baseTags, tagReads: p.tagReads, reads: p.reads, outcomes: p.outcomes, stages: p.stages,
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
    const tagRead = own && request.method === 'GET'
        && /^\/api\/file-manager\/files\/[^/]+\/tags$/.test(url.pathname);
    const history = own && request.method === 'GET' && /^\/api\/file-manager\/files\/[^/]+\/history$/.test(url.pathname);
    const first = p.writes[0];
    const outcome = own && request.method === 'GET' && first?.lost
        && url.pathname === `/api/operations/${first.body.operation_id}/outcome`;
    let write = null, read = null;
    if (tags) {
        const text = await request.clone().text();
        write = {method: request.method, url: url.href, text, body: JSON.parse(text),
            delivered: false, lost: false, outcomesAtStart: p.outcomes.length,
            historyReadsAtStart: p.reads.length, tagReadsAtStart: p.tagReads.length};
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
    if (tagRead && response.ok) {
        const body = await observedJson(response, {});
        if (body?.file_id && body.context
            && url.searchParams.get('repository_id') === body.context.repository_id
            && url.searchParams.get('path') === body.context.path
            && url.pathname === `/api/file-manager/files/${body.file_id}/tags`)
            p.tagReads.push({body, url: url.href});
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

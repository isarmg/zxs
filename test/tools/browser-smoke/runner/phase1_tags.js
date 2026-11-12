// Observe real requests and responses, without recording authentication headers
// or replacing business data. A held real POST receipt tests duplicate clicks.
if (window.__zxsPhase1Tags) throw new Error('Tag probe already installed');
const original = window.fetch;
const canonical = value => Array.isArray(value) ? value.map(canonical)
    : value !== null && typeof value === 'object'
        ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;
const same = (a, b) => JSON.stringify(canonical(a)) === JSON.stringify(canonical(b));
const p = window.__zxsPhase1Tags = {
    reads: [], tagReads: [], tagSnapshots: [], verifiedTagReads: [], writes: [], unexpected: [], base: null, snapshots: [], closed: false,
    label: '一期已审核',
    release() { for (const write of p.writes) write.release?.(); },
    snapshot() {
        return {scope: 'observed real tag request, receipt and history; no authentication headers',
            base: p.base, snapshots: p.snapshots, tagSnapshots: p.tagSnapshots,
            tagReads: p.tagReads, readCount: p.reads.length,
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
        const page = p.reads.at(-1)?.body, tags = p.tagReads.at(-1)?.body;
        const node = page?.nodes?.find(node => node.node_id === page.identity.context.current_node_id);
        if (!node || page.kind !== 'nodes' || page.page.next_cursor !== null
            || page.page.previous_cursor !== null || !page.identity.context.selected_branch_id
            || page.identity.context.path !== 'browser-smoke.txt' || !p.bound(tags, page)
            || tags.tags.includes(p.label))
            throw new Error('tag fixture needs complete real writable history and container tags');
        p.base = page;
        p.snapshots.push(page);
        p.tagSnapshots.push(tags);
        return node.node_id;
    },
    bound(tags, page) {
        return tags?.file_id === page?.identity.file_id
            && tags.context?.repository_id === page?.identity.context.repository_id
            && tags.context?.path === page?.identity.context.path
            && Array.isArray(tags.tags) && /^(0|[1-9][0-9]*)$/.test(tags.generation);
    },
    verify(index) {
        const write = p.writes[index], page = p.reads.at(-1)?.body;
        const tags = p.tagReads.at(-1)?.body;
        if (!write?.delivered || !page || !tags || p.unexpected.length
            || p.writes.length !== index + 1) return false;
        const before = p.snapshots[index], prior = p.tagSnapshots[index];
        const identity = before.identity, context = identity.context;
        const expectedTags = index === 0 ? [...prior.tags, p.label].sort()
            : prior.tags.filter(label => label !== p.label);
        const expected = {repository_id: context.repository_id, path: context.path,
            expected_generation: prior.generation, tags: expectedTags};
        const {operation_id, ...body} = write.body;
        const command = {file_id: identity.file_id,
            context: {repository_id: context.repository_id, path: context.path},
            expected_generation: prior.generation, tags: expectedTags};
        const replacement = {request: command, tags: expectedTags,
            generation: (BigInt(prior.generation) + 1n).toString(), changed: true};
        if (!same(body, expected) || !/^op_[0-9a-f]{32}$/.test(operation_id)
            || write.path !== `/api/file-manager/files/${identity.file_id}/tags`
            || write.status !== 200 || write.receipt?.state !== 'completed'
            || write.receipt.operation_kind !== 'file.container-tags.replace'
            || write.receipt.operation_id !== operation_id || write.receipt.error !== null
            || write.receipt.commit_state !== 'PUBLISHED'
            || write.receipt.protection_at_publish !== 'NOT_APPLICABLE'
            || !/^[0-9a-f]{64}$/.test(write.receipt.request_digest)
            || !/^[1-9][0-9]*$/.test(write.receipt.publication_sequence)
            || !same(write.receipt.result, replacement))
            throw new Error('tag write did not preserve the reviewed CAS or complete receipt');
        if (p.reads.length <= write.historyReadsAtStart
            || p.tagReads.length <= write.tagReadsAtStart) return false;
        if (page.identity.file_id !== identity.file_id || !same(page.identity.context, context)
            || !same(page.nodes, before.nodes) || !same(page.edges, before.edges)
            || page.page.previous_cursor !== null || page.page.next_cursor !== null)
            throw new Error('tag save changed the selected identity, graph or original body');
        if (!p.bound(tags, page) || !same(tags.tags, expectedTags)
            || tags.generation !== replacement.generation)
            throw new Error('fresh tag read does not match the container replacement');
        p.snapshots[index + 1] = page;
        p.tagSnapshots[index + 1] = tags;
        p.verifiedTagReads[index] ??= p.tagReads.length;
        return page.identity.context.current_node_id;
    },
    reread(index, after) {
        if (p.reads.length <= after || p.tagReads.length <= p.verifiedTagReads[index]) return false;
        const page = p.reads.at(-1)?.body, saved = p.snapshots[index + 1];
        const tags = p.tagReads.at(-1)?.body;
        if (!page || !saved) return false;
        if (page.identity.file_id !== saved.identity.file_id
            || !same(page.identity.context, saved.identity.context)
            || !same(page.nodes, saved.nodes) || !same(page.edges, saved.edges)
            || page.page.previous_cursor !== null || page.page.next_cursor !== null
            || !same(tags, p.tagSnapshots[index + 1]))
            throw new Error('fresh history or tags did not retain the container publication');
        return page.identity.context.current_node_id;
    },
    versionSelection(index, afterTags) {
        if (p.tagReads.length <= afterTags) return false;
        const page = p.reads.at(-1)?.body, tags = p.tagReads.at(-1)?.body;
        if (!p.bound(tags, page) || !same(tags, p.tagSnapshots[index + 1]))
            throw new Error('version selection changed container tags or generation');
        return true;
    }

};
window.fetch = async function(...args) {
    const input = args[0];
    const request = new Request(input instanceof Request ? input.clone() : input, args[1]);
    const url = new URL(request.url, location.href), own = url.origin === location.origin;
    const tags = own && request.method === 'POST' && /^\/api\/file-manager\/files\/[^/]+\/tags$/.test(url.pathname);
    const tagRead = own && request.method === 'GET'
        && /^\/api\/file-manager\/files\/[^/]+\/tags$/.test(url.pathname);
    const history = own && request.method === 'GET' && /^\/api\/file-manager\/files\/[^/]+\/history$/.test(url.pathname);
    const write = tags ? {path: url.pathname, body: await request.clone().json(), delivered: false,
        historyReadsAtStart: p.reads.length, tagReadsAtStart: p.tagReads.length} : null;
    if (write) p.writes.push(write);
    else if (own && url.pathname.startsWith('/api/') && !['GET', 'HEAD'].includes(request.method))
        p.unexpected.push({method: request.method, path: url.pathname});
    const response = await Reflect.apply(original, this, args);
    if (history) {
        const body = await response.clone().json();
        if (response.ok && body.kind === 'nodes') p.reads.push({body});
    }
    if (tagRead && response.ok) {
        const body = await response.clone().json();
        if (body.file_id && body.context
            && url.searchParams.get('repository_id') === body.context.repository_id
            && url.searchParams.get('path') === body.context.path
            && url.pathname === `/api/file-manager/files/${body.file_id}/tags`)
            p.tagReads.push({body, url: url.href});
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

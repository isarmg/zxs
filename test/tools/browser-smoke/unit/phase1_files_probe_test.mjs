// Host models validate the maintained observers/assertions, not Chrome or WASM.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import {runInNewContext} from 'node:vm';

const origin = 'http://127.0.0.1:2345';
const sources = Object.fromEntries(['disk_read', 'tags'].map(name => [name,
    readFileSync(new URL(`../runner/phase1_${name}.js`, import.meta.url), 'utf8')]));
const copy = value => JSON.parse(JSON.stringify(value));
const tick = () => new Promise(resolve => setImmediate(resolve));
async function until(condition) {
    for (let i = 0; i < 100; i++) { if (condition()) return; await tick(); }
    assert.fail('observer did not reach its expected gate');
}
function fixture(kind, responder = () => ({fixture: 'actual response'})) {
    const calls = [], responses = [];
    const original = async (...args) => {
        calls.push(args);
        const value = await responder(...args);
        const response = value instanceof Response ? value : new Response(JSON.stringify(value), {
            headers: {'content-type': 'application/json', 'x-real': 'preserved'}
        });
        responses.push(response);
        return response;
    };
    const window = {fetch: original};
    const context = {window, Request, URL, location: {origin, href: `${origin}/`}};
    const install = () => runInNewContext(`(function(){${sources[kind]}})()`, context);
    assert.equal(install(), true);
    return {window, calls, responses, original, install,
        state: window[kind === 'tags' ? '__zxsPhase1Tags' : '__zxsPhase1DiskRead']};
}

test('disk failure happens only after the real response; retry preserves bytes and headers', async () => {
    const f = fixture('disk_read', () => ({disks: [{slot_id: 'P01', online: true, writable: true}]}));
    const request = new Request(`${origin}/api/file-manager/disks`);
    const first = f.window.fetch(request);
    await until(() => f.state.reads[0]?.release);
    assert.equal(f.calls[0][0], request);
    assert.equal(f.state.reads[0].status, 200);
    assert.equal(f.state.reads[0].delivered, false);
    const rejected = assert.rejects(first, /phase-one disk response delivery interrupted/);
    f.state.release(0, true);
    await rejected;
    const next = f.window.fetch(request);
    await until(() => f.state.reads[1]?.release);
    f.state.release(1);
    const response = await next;
    assert.equal(response, f.responses[1]);
    assert.equal(response.bodyUsed, false);
    assert.equal(response.headers.get('x-real'), 'preserved');
    assert.deepEqual(await response.json(), {disks: [{slot_id: 'P01', online: true, writable: true}]});
    assert.equal(f.calls.length, 2);
    f.state.restore();
});

test('health gate holds an original response and cleanup releases it without replacement', async () => {
    const f = fixture('disk_read', () => ({storage: {state: 'ready', recovering: false}}));
    let delivered = false;
    const pending = f.window.fetch(`${origin}/api/management/status`).then(value => { delivered = true; return value; });
    await until(() => f.state.statusHeld);
    assert.equal(delivered, false);
    assert.throws(f.install, /already installed/);
    f.state.restore();
    assert.equal(await pending, f.responses[0]);
    assert.equal(f.window.fetch, f.original);
    assert.equal(f.window.__zxsPhase1DiskRead, undefined);
});

test('disk observer preserves real 429 and bypasses other origins and non-GET requests', async () => {
    const f = fixture('disk_read', () => new Response('{"error":{"code":"RESOURCE_LIMIT_EXCEEDED"}}', {status: 429}));
    for (const request of [new Request(`${origin}/api/file-manager/disks`, {method: 'POST'}),
        new Request('https://other.example/api/file-manager/disks')]) {
        const response = await f.window.fetch(request);
        assert.equal(response, f.responses.at(-1));
        assert.equal(response.status, 429);
    }
    assert.equal(f.state.reads.length, 0);
    const pending = f.window.fetch(`${origin}/api/file-manager/disks`);
    await until(() => f.state.reads[0]?.release);
    f.state.release(0);
    const response = await pending;
    assert.equal(response.status, 429);
    assert.equal(response, f.responses.at(-1));
    assert.equal(f.calls.length, 3);
    f.state.restore();
});

test('cleanup before or after a disk response arrives cannot strand the original request', async () => {
    let release;
    const gate = new Promise(resolve => release = resolve);
    const f = fixture('disk_read', async () => { await gate; return {disks: []}; });
    const pending = f.window.fetch(`${origin}/api/file-manager/disks`);
    f.state.restore();
    release();
    assert.equal(await pending, f.responses[0]);
    const g = fixture('disk_read');
    const held = g.window.fetch(`${origin}/api/file-manager/disks`);
    await until(() => g.state.reads[0]?.release);
    g.state.restore();
    assert.equal(await held, g.responses[0]);
});

const id = (prefix, number) => `${prefix}_${number.toString(16).padStart(32, '0')}`;
function page() {
    return {kind: 'nodes', identity: {file_id: id('file', 1), history_root: 'a'.repeat(64),
        context: {repository_id: id('repo', 1), history_tree_id: id('history', 1),
            path: 'browser-smoke.txt', selected_branch_id: id('br', 1), current_node_id: id('ver', 1)}},
        page: {previous_cursor: null, next_cursor: null}, edges: [], nodes: [{node_id: id('ver', 1),
            version_id: id('ver', 1), current: true, kind: 'content', tags: [], size: '7',
            content_hash: 'b'.repeat(64), branch_id: id('br', 1), note: 'retained', parents: {state: 'complete'}}]};
}
function publication(p, index) {
    const before = p.snapshots[index], context = before.identity.context;
    const old = before.nodes.find(node => node.node_id === context.current_node_id);
    const after = copy(before), next = {...copy(old), node_id: id('fmeta', index + 2),
        kind: 'tags', tags: index === 0 ? [p.label] : []};
    for (const node of after.nodes) node.current = false;
    after.nodes.unshift(next);
    after.identity.history_root = String(index + 2).repeat(64);
    after.identity.context.current_node_id = next.node_id;
    after.edges.push({node_id: next.node_id, parent_node_id: old.node_id, endpoint: 'in_window'});
    const operation = id('op', index + 1);
    const body = {operation_id: operation, add: index === 0 ? [p.label] : [],
        remove: index === 0 ? [] : [p.label], repository_id: context.repository_id, path: context.path,
        targets: [{branch_id: context.selected_branch_id, expected_node_id: old.node_id, source_node_id: old.node_id}]};
    // This is the public operation_wire shape, not the persisted OperationResult.kind.
    const receipt = {operation_id: operation, operation_kind: 'file.tags.update', state: 'completed', error: null,
        commit_state: 'PUBLISHED', protection_at_publish: 'NOT_APPLICABLE',
        request_digest: 'd'.repeat(64), publication_sequence: String(index + 2),
        result: {file_id: before.identity.file_id, content_versions_created: '0', nodes: [{node_id: next.node_id,
            source_node_id: old.node_id, branch_id: context.selected_branch_id, no_change: false, advanced: true, tags: next.tags}]}};
    p.writes.push({path: `/api/file-manager/files/${before.identity.file_id}/tags`, body, status: 200, receipt, delivered: true});
    p.reads.push({body: after});
    return {before, after, body, receipt, next};
}
function tagged() {
    const f = fixture('tags');
    f.state.reads.push({body: page()});
    assert.equal(f.state.capture(), id('ver', 1));
    const mutation = publication(f.state, 0);
    return {...f, ...mutation};
}

test('tag observer preserves request, original response and full receipt, with no credentials in its record', async () => {
    const f = fixture('tags', () => page());
    const request = new Request(`${origin}/api/file-manager/files/${id('file', 1)}/history`, {headers: {authorization: 'Bearer test-only-secret'}});
    const response = await f.window.fetch(request);
    assert.equal(response, f.responses[0]);
    assert.equal(f.calls[0][0], request);
    assert.equal(response.bodyUsed, false);
    assert.deepEqual(copy(f.state.reads[0].body), page());
    const post = new Request(`${origin}/api/file-manager/files/${id('file', 1)}/tags`, {method: 'POST', body: '{"add":["一期已审核"]}'});
    const pending = f.window.fetch(post);
    await until(() => f.state.writes[0]?.release);
    assert.equal(post.bodyUsed, false);
    assert.equal(f.calls[1][0], post);
    assert.equal(f.state.writes[0].delivered, false);
    assert.equal(JSON.stringify(f.state).includes('test-only-secret'), false);
    f.state.release();
    assert.equal(await pending, f.responses[1]);
    assert.equal(f.state.writes[0].delivered, true);
    f.state.restore();
});

test('tag proof accepts add then remove, old current flags changing, and fresh identical rereads', () => {
    const f = tagged();
    assert.equal(f.state.verify(0), f.next.node_id);
    const after = f.state.reads.length;
    assert.equal(f.state.reread(0, after), false);
    f.state.reads.push({body: copy(f.after)});
    assert.equal(f.state.reread(0, after), f.next.node_id);
    const removed = publication(f.state, 1);
    assert.equal(f.state.verify(1), removed.next.node_id);
    assert.deepEqual(copy(removed.next.tags), []);
    assert.equal(f.state.snapshots[1].nodes[0].tags[0], f.state.label);
    f.state.restore();
});

test('tag proof binds the public operation_kind and rejects internal or unrelated receipt kinds', () => {
    const valid = tagged();
    assert.equal(Object.hasOwn(valid.receipt, 'kind'), false);
    assert.equal(valid.state.verify(0), valid.next.node_id);
    valid.state.restore();
    for (const mutate of [
        receipt => { receipt.kind = receipt.operation_kind; delete receipt.operation_kind; },
        receipt => { receipt.operation_kind = 'directory.namespace.batch'; },
    ]) {
        const f = tagged();
        mutate(f.receipt);
        assert.throws(() => f.state.verify(0), /complete receipt/);
        f.state.restore();
    }
});

test('tag failure evidence preserves observed data without gate callbacks or credentials', async () => {
    const f = fixture('tags', () => page());
    const request = new Request(`${origin}/api/file-manager/files/${id('file', 1)}/tags`, {
        method: 'POST', headers: {authorization: 'Bearer test-only-secret'}, body: '{"add":["一期已审核"]}'
    });
    const pending = f.window.fetch(request);
    await until(() => f.state.writes[0]?.release);
    const snapshot = f.state.snapshot();
    assert.equal(snapshot.writes.length, 1);
    assert.equal(snapshot.writes[0].status, 200);
    assert.equal(snapshot.writes[0].delivered, false);
    assert.deepEqual(copy(snapshot.writes[0].body), {add: ['一期已审核']});
    assert.deepEqual(copy(snapshot.writes[0].receipt), page());
    assert.equal(Object.hasOwn(snapshot.writes[0], 'release'), false);
    assert.equal(JSON.stringify(snapshot).includes('test-only-secret'), false);
    assert.doesNotThrow(() => structuredClone(snapshot));
    f.state.restore();
    assert.equal(await pending, f.responses[0]);
    assert.equal(f.window.fetch, f.original);
    assert.equal(f.window.__zxsPhase1Tags, undefined);
    assert.equal(snapshot.writes[0].delivered, false, 'saved failure evidence is not changed by cleanup');
});

test('tag proof rejects wrong receipt, changed file/body/old node, missing edge and extra writes', () => {
    const mutations = [
        f => { f.receipt.operation_id = id('op', 99); },
        f => { f.receipt.state = 'running'; },
        f => { f.receipt.result.content_versions_created = '1'; },
        f => { f.receipt.result.nodes[0].node_id = id('fmeta', 99); },
        f => { f.after.identity.file_id = id('file', 99); },
        f => { f.after.identity.context.repository_id = id('repo', 99); },
        f => { f.after.identity.context.history_tree_id = id('history', 99); },
        f => { f.after.nodes[0].content_hash = 'c'.repeat(64); },
        f => { f.after.nodes[1].note = 'changed'; },
        f => { f.after.edges = []; },
    ];
    for (const mutate of mutations) {
        const f = tagged();
        mutate(f);
        assert.throws(() => f.state.verify(0), /tag/);
        f.state.restore();
    }
    const f = tagged();
    f.state.writes.push(copy(f.state.writes[0]));
    assert.equal(f.state.verify(0), false);
    f.state.restore();
});

test('tag capture and reread reject unreviewed or altered history, and cleanup releases held receipts', async () => {
    const f = fixture('tags');
    const invalid = page();
    invalid.identity.context.selected_branch_id = null;
    f.state.reads.push({body: invalid});
    assert.throws(() => f.state.capture(), /complete real writable/);
    assert.throws(f.install, /already installed/);
    const post = f.window.fetch(`${origin}/api/file-manager/files/${id('file', 1)}/tags`, {method: 'POST', body: '{}'});
    await until(() => f.state.writes[0]?.release);
    f.state.restore();
    assert.equal(await post, f.responses[0]);
    assert.equal(f.window.fetch, f.original);
    const g = tagged();
    g.state.verify(0);
    const after = g.state.reads.length, changed = copy(g.after);
    changed.nodes[0].tags = [];
    g.state.reads.push({body: changed});
    assert.throws(() => g.state.reread(0, after), /fresh history/);
    g.state.restore();
});

// Host fixtures exercise the observer, not production WASM or actual Chrome.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import {runInNewContext} from 'node:vm';

const source = readFileSync(new URL('../runner/tag_unknown.js', import.meta.url), 'utf8');
const origin = 'http://127.0.0.1:2345';
const copy = value => JSON.parse(JSON.stringify(value));
const id = (prefix, value) => `${prefix}_${value.toString(16).padStart(32, '0')}`;
const tick = () => new Promise(resolve => setImmediate(resolve));
async function until(condition) {
    for (let i = 0; i < 100; i++) { if (condition()) return; await tick(); }
    assert.fail('tag observer did not reach its expected gate');
}
function history() {
    return {kind: 'nodes', identity: {file_id: id('file', 1), history_root: 'a'.repeat(64),
        context: {repository_id: id('repo', 1), history_tree_id: id('history', 1),
            path: 'browser-smoke.txt', selected_branch_id: id('br', 1), current_node_id: id('ver', 1)}},
        page: {previous_cursor: null, next_cursor: null}, edges: [], nodes: [{node_id: id('ver', 1),
            version_id: id('ver', 1), current: true, kind: 'content', tags: [], size: '7',
            content_hash: 'b'.repeat(64), branch_id: id('br', 1), note: 'retained', parents: {state: 'complete'}}]};
}
function fixture() {
    const base = history(), after = copy(base), operation = id('op', 1), label = '恢复原请求';
    const context = base.identity.context, old = base.nodes[0];
    const next = {...copy(old), node_id: id('fmeta', 2), kind: 'tags', tags: [label]};
    after.nodes[0].current = false;
    after.nodes.unshift(next);
    after.identity.history_root = '2'.repeat(64);
    after.identity.context.current_node_id = next.node_id;
    after.edges.push({node_id: next.node_id, parent_node_id: old.node_id, endpoint: 'in_window'});
    const body = {operation_id: operation, repository_id: context.repository_id, path: context.path,
        targets: [{branch_id: context.selected_branch_id, source_node_id: old.node_id, expected_node_id: old.node_id}],
        add: [label], remove: []};
    const receipt = {operation_id: operation, operation_kind: 'file.tags.update', state: 'completed', error: null,
        commit_state: 'PUBLISHED', protection_at_publish: 'NOT_APPLICABLE', request_digest: 'd'.repeat(64),
        publication_sequence: '2', result: {file_id: base.identity.file_id, content_versions_created: '0',
            nodes: [{node_id: next.node_id, source_node_id: old.node_id, branch_id: context.selected_branch_id,
                no_change: false, advanced: true, tags: next.tags}]}};
    const calls = [], responses = [];
    const fixture = {base, after, next, body, receipt, history: base, post: null, outcome: null,
        beforeResponse: null, calls, responses,
        tagsUrl: `${origin}/api/file-manager/files/${base.identity.file_id}/tags`,
        historyUrl: `${origin}/api/file-manager/files/${base.identity.file_id}/history`,
        outcomeUrl: `${origin}/api/operations/${operation}/outcome`};
    const original = async (...args) => {
        calls.push(args);
        if (fixture.beforeResponse) await fixture.beforeResponse();
        const request = args[0], url = new URL(typeof request === 'string' ? request : request.url);
        const method = request.method || args[1]?.method || 'GET';
        const value = url.pathname.endsWith('/history') ? fixture.history
            : method === 'POST' ? fixture.post ?? fixture.receipt : fixture.outcome ?? fixture.receipt;
        const response = value instanceof Response ? value : new Response(JSON.stringify(value), {
            headers: {'content-type': 'application/json', 'x-original': 'unchanged'}
        });
        responses.push(response);
        return response;
    };
    const window = {fetch: original};
    const contextJs = {window, Request, Response, Headers, URL, location: {origin, href: `${origin}/`}};
    const install = () => runInNewContext(`(function(){${source}})()`, contextJs);
    assert.equal(install(), true);
    return Object.assign(fixture, {window, original, install, state: window.__zxsTagUnknown});
}
function request(f, body = f.body, url = f.tagsUrl) {
    return new Request(url, {method: 'POST', body: JSON.stringify(body),
        headers: {authorization: 'Bearer test-only-secret', 'content-type': 'application/json'}});
}
async function lose(f) {
    await f.window.fetch(f.historyUrl);
    assert.equal(f.state.capture(), f.base.nodes[0].node_id);
    const req = request(f);
    await assert.rejects(f.window.fetch(req), /published POST response delivery lost/);
    assert.equal(f.calls.at(-1)[0], req);
    assert.equal(req.bodyUsed, false);
    assert.equal(f.responses.at(-1).bodyUsed, false);
    assert.equal(f.state.writes[0].lost, true);
}
async function hiddenOutcomes(f) {
    for (let i = 0; i < 2; i++) {
        const response = await f.window.fetch(f.outcomeUrl);
        assert.equal(response.status, 404);
        assert.notEqual(response, f.responses.at(-1));
        assert.equal(f.responses.at(-1).status, 200);
        assert.equal((await response.json()).error.code, 'NOT_FOUND');
    }
    assert.equal(f.state.uncertain(), true);
}
async function recovered(f) {
    await lose(f);
    await hiddenOutcomes(f);
    const pending = f.window.fetch(request(f));
    await until(() => f.state.writes[1]?.release);
    assert.equal(f.state.replay(), true);
    assert.equal(f.state.writes[1].delivered, false);
    f.state.release();
    assert.equal(await pending, f.responses.at(-1));
    f.history = f.after;
    await f.window.fetch(f.historyUrl);
    assert.equal(f.state.verify(), f.next.node_id);
}

test('published delivery loss, two hidden outcomes and exact retry preserve one original receipt and body', async () => {
    const f = fixture();
    await recovered(f);
    assert.equal(f.state.writes.length, 2);
    assert.equal(f.state.outcomes.length, 2);
    assert.deepEqual(copy(f.state.writes[0].body), f.body);
    assert.deepEqual(copy(f.state.writes[1].receipt), f.receipt);
    assert.equal(f.state.writes[0].text, f.state.writes[1].text);
    assert.equal(f.responses.at(-2).headers.get('x-original'), 'unchanged');
    f.state.restore();
    assert.equal(f.window.fetch, f.original);
    assert.equal(f.window.__zxsTagUnknown, undefined);
});

test('only a completed matching real publication can lose delivery; wrong context and kinds pass through', async () => {
    for (const mutate of [
        f => { f.receipt.commit_state = 'UNPUBLISHED'; },
        f => { f.receipt.state = 'running'; },
        f => { f.receipt.operation_id = id('op', 9); },
        f => { f.receipt.kind = f.receipt.operation_kind; delete f.receipt.operation_kind; },
        f => { f.receipt.operation_kind = 'directory.namespace.batch'; },
        f => { f.receipt.result.file_id = id('file', 9); },
        f => { f.receipt.result.content_versions_created = '1'; },
        f => { f.body.path = 'another.txt'; },
        f => { f.body.targets[0].expected_node_id = id('ver', 9); },
        f => { f.body.extra = 'unreviewed'; },
    ]) {
        const f = fixture(); mutate(f);
        await f.window.fetch(f.historyUrl); f.state.capture();
        const response = await f.window.fetch(request(f));
        assert.equal(response, f.responses.at(-1));
        assert.equal(f.state.writes[0].lost, false);
        assert.throws(() => f.state.uncertain(), /not a matching publication/);
        f.state.restore();
    }
});

test('real 429, authentication errors and malformed bodies pass through without success or 404 substitution', async () => {
    for (const status of [401, 429, 500]) {
        const f = fixture();
        await f.window.fetch(f.historyUrl); f.state.capture();
        const bytes = JSON.stringify({error: {code: status === 429 ? 'RESOURCE_LIMIT_EXCEEDED' : 'HTTP_ERROR', message: 'real fixture error'}});
        f.post = new Response(bytes, {status, headers: {'x-original': 'error'}});
        const result = await f.window.fetch(request(f));
        assert.equal(result, f.post);
        assert.equal(await result.text(), bytes);
        assert.equal(f.calls.length, 2, 'no retry by probe');
        f.state.restore();
        const g = fixture(); await lose(g);
        g.outcome = new Response(bytes, {status});
        assert.equal(await g.window.fetch(g.outcomeUrl), g.outcome);
        assert.equal(g.state.outcomes[0].hidden404, false);
        g.state.restore();
    }
    const f = fixture(); await lose(f);
    f.outcome = new Response('not json', {status: 200});
    assert.equal(await f.window.fetch(f.outcomeUrl), f.outcome);
    assert.equal(f.state.outcomes[0].invalidJson, true);
    assert.equal(f.state.outcomes[0].hidden404, false);
    f.state.restore();
});

test('outcome hiding requires the exact lost receipt, not another operation or an unproven response', async () => {
    const f = fixture();
    assert.equal(await f.window.fetch(f.outcomeUrl), f.responses.at(-1));
    assert.equal(f.state.outcomes.length, 0);
    await lose(f);
    f.outcome = {...copy(f.receipt), publication_sequence: '3'};
    assert.equal(await f.window.fetch(f.outcomeUrl), f.responses.at(-1));
    assert.equal(f.state.outcomes[0].hidden404, false);
    assert.equal(await f.window.fetch(f.outcomeUrl.replace(id('op', 1), id('op', 2))), f.responses.at(-1));
    assert.equal(f.state.outcomes.length, 1);
    f.state.restore();
});

test('replay proof rejects changed method, URL, operation ID, complete body or encoded bytes', async () => {
    for (const mutate of [
        write => { write.method = 'PUT'; }, write => { write.url += '?another=scope'; },
        write => { write.body.operation_id = id('op', 2); },
        write => { write.body.remove = ['unreviewed']; }, write => { write.text += ' '; },
        write => { write.body.targets[0].branch_id = id('br', 2); },
    ]) {
        const f = fixture(); await recovered(f);
        mutate(f.state.writes[1]);
        assert.throws(() => f.state.replay(), /frozen method, URL, operation ID or complete body/);
        f.state.restore();
    }
});

test('a concurrent outcome or additional mutation cannot be reported as an exclusive retry', async () => {
    const f = fixture(); await lose(f);
    const pending = f.window.fetch(request(f));
    await until(() => f.state.writes[1]?.release);
    await f.window.fetch(f.outcomeUrl);
    assert.throws(() => f.state.replay(), /polling overlapped/);
    f.state.restore(); await pending;
    const g = fixture(); await recovered(g);
    await g.window.fetch(`${origin}/api/directories`, {method: 'POST', body: '{}'});
    assert.throws(() => g.state.replay(), /unexpected write/);
    g.state.restore();
    const h = fixture(); await recovered(h);
    h.state.writes.push(copy(h.state.writes[0]));
    assert.throws(() => h.state.replay(), /unexpected write/);
    h.state.restore();
});

test('fresh history proof rejects duplicate child, changed body, missing parent, altered old node and wrong identity', async () => {
    for (const mutate of [
        page => { page.nodes.push(copy(page.nodes[0])); },
        page => { page.nodes[0].content_hash = 'c'.repeat(64); },
        page => { page.nodes[0].version_id = id('ver', 99); },
        page => { page.nodes[1].note = 'lost'; },
        page => { page.edges = []; },
        page => { page.identity.context.repository_id = id('repo', 99); },
        page => { page.identity.context.history_tree_id = id('history', 99); },
        page => { page.identity.context.selected_branch_id = id('br', 99); },
        page => { page.page.next_cursor = 'more'; },
    ]) {
        const f = fixture(); await recovered(f);
        mutate(f.state.reads.at(-1));
        assert.throws(() => f.state.verify(), /recovered history|replay created|recovery changed/);
        f.state.restore();
    }
});

test('cleanup before first response or during retry hold cannot leave a request stranded', async () => {
    const f = fixture(); await f.window.fetch(f.historyUrl); f.state.capture();
    let release;
    f.beforeResponse = () => new Promise(resolve => { release = resolve; });
    const first = f.window.fetch(request(f));
    await until(() => !!release);
    f.state.restore(); release();
    assert.equal(await first, f.responses.at(-1));
    assert.equal(f.state.writes[0].lost, false);
    const g = fixture(); await lose(g);
    const retry = g.window.fetch(request(g));
    await until(() => g.state.writes[1]?.release);
    g.state.restore();
    assert.equal(await retry, g.responses.at(-1));
    assert.equal(g.window.fetch, g.original);
});

test('diagnostics retain request and receipt but not credentials or release callbacks', async () => {
    const f = fixture(); await lose(f); await hiddenOutcomes(f);
    const retry = f.window.fetch(request(f));
    await until(() => f.state.writes[1]?.release);
    const saved = f.state.snapshot();
    assert.doesNotThrow(() => structuredClone(saved));
    assert.equal(JSON.stringify(saved).includes('test-only-secret'), false);
    assert.equal(JSON.stringify(saved).includes('authorization'), false);
    assert.equal(Object.hasOwn(saved.writes[1], 'release'), false);
    assert.equal(saved.writes[1].delivered, false);
    assert.deepEqual(copy(saved.writes[1].receipt), f.receipt);
    f.state.restore(); await retry;
    assert.equal(saved.writes[1].delivered, false, 'cleanup does not rewrite saved evidence');
});

test('capture rejects incomplete or foreign history and duplicate installation', async () => {
    for (const mutate of [
        page => { page.page.next_cursor = 'more'; },
        page => { page.identity.context.selected_branch_id = null; },
        page => { page.identity.context.path = 'another.txt'; },
        page => { page.nodes[0].tags = ['恢复原请求']; },
    ]) {
        const f = fixture(); mutate(f.history);
        await f.window.fetch(f.historyUrl);
        assert.throws(() => f.state.capture(), /complete real writable/);
        assert.throws(f.install, /already installed/);
        f.state.restore();
    }
});

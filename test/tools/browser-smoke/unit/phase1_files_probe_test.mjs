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
function fixture(kind, responder = () => ({fixture: 'actual response'}), document) {
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
    const context = {window, Request, URL, document, location: {origin, href: `${origin}/`}};
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

const diskRecord = (level, online, issue = null) => ({name: `disk-${level}`, slot_id: `slot-${level}`,
    level, online, writable: level === 1, available_bytes: issue ? null : '0', observation_failure: issue});
const diskRow = (disk, levelLabel, mode, observation, space = '0 B') => ({slot: disk.slot_id,
    name: disk.name, disabled: !disk.online, online: String(disk.online), level: disk.level,
    levelLabel, mode, observation, space});

test('disk table observer reads all six current columns instead of the old access column', () => {
    const disk = diskRecord(3, false);
    const row = diskRow(disk, 'Level 3 · Backup', 'Read only', 'Offline');
    const document = {querySelectorAll(selector) {
        assert.equal(selector, '.zxs-disk-table tbody tr');
        return [{dataset: {online: 'false', diskLevel: '3'}, cells:
            ['', row.levelLabel, row.mode, row.observation, row.space, 'Open'].map(textContent => ({textContent})),
            querySelector(selector) {
                assert.equal(selector, '.zxs-disk-entry');
                return {dataset: {slot: disk.slot_id}, textContent: disk.name, disabled: true};
            }}];
    }};
    const f = fixture('disk_read', undefined, document);
    assert.deepEqual(copy(f.state.rows()), [row]);
    assert.equal(f.state.matches(f.state.rows(), [disk]), true);
    const old = {...row, access: row.levelLabel, space: row.mode};
    delete old.mode; delete old.observation; delete old.levelLabel;
    assert.equal(f.state.matches([old], [disk]), false);
    f.state.restore();
});

test('disk proof preserves real order, higher levels, zero and unavailable capacity', () => {
    const f = fixture('disk_read');
    const disks = [diskRecord(3, false, {stage: 'seal'}), diskRecord(1, true),
        diskRecord(2, true, {stage: 'capacity'}), diskRecord(7, false, {stage: 'capacity'})];
    const rows = [diskRow(disks[0], 'Level 3 · Backup', 'Read only', 'Unavailable (identity observation failed)', 'Unavailable'),
        diskRow(disks[1], '第 1 级 · 主盘', '读写目录', '在线'),
        diskRow(disks[2], '层级 2 · 备份盘', '只读', '在线（容量不可用）', '不可用'),
        diskRow(disks[3], 'Level 7 · Backup', 'Read only', 'Unavailable (capacity observation failed)', 'Unavailable')];
    assert.equal(f.state.matches(rows, disks), true);
    assert.equal(f.state.matches([...rows].reverse(), disks), false);
    assert.equal(f.state.matches(rows.slice(1), disks), false);
    const wrong = copy(rows); wrong[0].space = '0 B';
    assert.equal(f.state.matches(wrong, disks), false);
    const zeroHidden = copy(rows); zeroHidden[1].space = '不可用';
    assert.equal(f.state.matches(zeroHidden, disks), false);
    f.state.restore();
});

test('disk proof rejects collapsed modes, false online state, and missing real level', () => {
    const f = fixture('disk_read'), disk = diskRecord(9, false);
    const row = diskRow(disk, 'Level 9 · Backup', 'Read only', 'Offline');
    for (const mutation of [{level: 1}, {levelLabel: 'Level 1 · Primary'}, {mode: 'Offline'},
        {mode: 'Read/write directory'}, {observation: 'Online'}, {disabled: false},
        {online: 'true'}, {name: 'another'}, {slot: 'another'}, {level: undefined}]) {
        assert.equal(f.state.matches([{...row, ...mutation}], [disk]), false);
    }
    const before = copy(row);
    assert.equal(f.state.matches([row], [disk]), true);
    assert.deepEqual(row, before);
    assert.equal(f.calls.length, 0, 'observing rows cannot send any request');
    f.state.restore();
});

test('disk proof checks nonzero byte values with native binary units and ties-to-even rounding', () => {
    const f = fixture('disk_read'), disk = diskRecord(2, true);
    const row = diskRow(disk, 'Level 2 · Backup', 'Read only', 'Online');
    for (const [bytes, display] of [
        ['42', '42 B'], ['1023', '1023 B'], ['1024', '1.0 KiB'],
        ['1280', '1.2 KiB'], ['1792', '1.8 KiB'], ['4096', '4.0 KiB'],
        ['1048576', '1.0 MiB'], ['9007199254740993', '8.0 PiB'],
        ['18446744073709551615', '16.0 EiB'],
    ]) {
        disk.available_bytes = bytes;
        assert.equal(f.state.matches([{...row, space: display}], [disk]), true, bytes);
    }
    disk.available_bytes = '4096';
    for (const space of ['Unavailable', '不可用', '0 B', '1.0 KiB', '4096 B', '']) {
        assert.equal(f.state.matches([{...row, space}], [disk]), false, space);
    }
    disk.available_bytes = '1280';
    assert.equal(f.state.matches([{...row, space: '1.3 KiB'}], [disk]), false);
    f.state.restore();
});

const id = (prefix, number) => `${prefix}_${number.toString(16).padStart(32, '0')}`;
function page() {
    return {kind: 'nodes', identity: {file_id: id('file', 1), history_root: 'a'.repeat(64),
        context: {repository_id: id('repo', 1), history_tree_id: id('history', 1),
            path: 'browser-smoke.txt', selected_branch_id: id('br', 1), current_node_id: id('ver', 1)}},
        page: {previous_cursor: null, next_cursor: null}, edges: [], nodes: [{node_id: id('ver', 1),
            version_id: id('ver', 1), current: true, kind: 'content', size: '7',
            content_hash: 'b'.repeat(64), branch_id: id('br', 1), note: 'retained', parents: {state: 'complete'}}]};
}
function labels() {
    return {file_id: id('file', 1),
        context: {repository_id: id('repo', 1), path: 'browser-smoke.txt'}, tags: [], generation: '0'};
}
function publication(p, index) {
    const before = p.snapshots[index], prior = p.tagSnapshots[index];
    const context = before.identity.context;
    const after = copy(before), next = after.nodes[0];
    after.identity.history_root = String(index + 2).repeat(64);
    const tags = index === 0 ? [...prior.tags, p.label].sort() : prior.tags.filter(t => t !== p.label);
    const operation = id('op', index + 1);
    const body = {operation_id: operation, tags, expected_generation: prior.generation,
        repository_id: context.repository_id, path: context.path};
    const command = {file_id: before.identity.file_id, context: copy(prior.context),
        expected_generation: prior.generation, tags};
    const current = {...copy(prior), tags, generation: (BigInt(prior.generation) + 1n).toString()};
    const receipt = {operation_id: operation, operation_kind: 'file.container-tags.replace',
        state: 'completed', error: null, commit_state: 'PUBLISHED',
        protection_at_publish: 'NOT_APPLICABLE', request_digest: 'd'.repeat(64),
        publication_sequence: String(index + 2), result: {request: command, tags,
            generation: current.generation, changed: true}};
    p.writes.push({path: `/api/file-manager/files/${before.identity.file_id}/tags`, body,
        status: 200, receipt, delivered: true, historyReadsAtStart: p.reads.length,
        tagReadsAtStart: p.tagReads.length});
    p.reads.push({body: after});
    p.tagReads.push({body: current});
    return {before, after, body, receipt, next, current};
}
function tagged() {
    const f = fixture('tags');
    f.state.reads.push({body: page()});
    f.state.tagReads.push({body: labels()});
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
    const post = new Request(`${origin}/api/file-manager/files/${id('file', 1)}/tags`, {method: 'POST', body: '{"expected_generation":"0","tags":["一期已审核"]}'});
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

test('tag proof accepts replacement then removal without graph changes and fresh independent rereads', () => {
    const f = tagged();
    assert.equal(f.state.verify(0), f.next.node_id);
    const after = f.state.reads.length;
    assert.equal(f.state.reread(0, after), false);
    f.state.reads.push({body: copy(f.after)});
    f.state.tagReads.push({body: copy(f.current)});
    assert.equal(f.state.reread(0, after), f.next.node_id);
    const removed = publication(f.state, 1);
    assert.equal(f.state.verify(1), removed.next.node_id);
    assert.deepEqual(copy(removed.current.tags), []);
    assert.equal(f.state.tagSnapshots[1].tags[0], f.state.label);
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
        method: 'POST', headers: {authorization: 'Bearer test-only-secret'}, body: '{"expected_generation":"0","tags":["一期已审核"]}'
    });
    const pending = f.window.fetch(request);
    await until(() => f.state.writes[0]?.release);
    const snapshot = f.state.snapshot();
    assert.equal(snapshot.writes.length, 1);
    assert.equal(snapshot.writes[0].status, 200);
    assert.equal(snapshot.writes[0].delivered, false);
    assert.deepEqual(copy(snapshot.writes[0].body), {expected_generation: '0', tags: ['一期已审核']});
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

test('tag proof rejects wrong receipt, changed file/body/old node, changed edges and extra writes', () => {
    const mutations = [
        f => { f.receipt.operation_id = id('op', 99); },
        f => { f.receipt.state = 'running'; },
        f => { f.receipt.result.changed = false; },
        f => { f.receipt.result.request.expected_generation = '9'; },
        f => { f.after.identity.file_id = id('file', 99); },
        f => { f.after.identity.context.repository_id = id('repo', 99); },
        f => { f.after.identity.context.history_tree_id = id('history', 99); },
        f => { f.after.nodes[0].content_hash = 'c'.repeat(64); },
        f => { f.after.nodes[0].note = 'changed'; },
        f => { f.after.edges.push({node_id: id('ver', 1), parent_node_id: id('ver', 2)}); },
        f => { f.current.generation = '8'; },
        f => { f.current.tags = []; },
        f => { f.after.nodes.push(copy(f.next)); },
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
    changed.nodes[0].content_hash = "c".repeat(64);
    g.state.reads.push({body: changed});
    g.state.tagReads.push({body: copy(g.current)});
    assert.throws(() => g.state.reread(0, after), /fresh history/);
    g.state.restore();
});

test('fresh container reads retain tags and generation across version selection', () => {
    const f = tagged();
    f.state.verify(0);
    const after = f.state.tagReads.length;
    assert.equal(f.state.versionSelection(0, after), false);
    f.after.identity.context.current_node_id = id('ver', 2);
    f.after.identity.context.selected_branch_id = id('br', 2);
    f.state.tagReads.push({body: copy(f.current)});
    assert.equal(f.state.versionSelection(0, after), true);
    f.state.tagReads.at(-1).body.tags = [];
    assert.throws(() => f.state.versionSelection(0, after), /version selection/);
    f.state.restore();
});

test('tag read observer binds the real GET path and query to the returned context', async () => {
    const f = fixture('tags', () => labels());
    const route = `${origin}/api/file-manager/files/${id('file', 1)}/tags`;
    await f.window.fetch(`${route}?repository_id=${id('repo', 2)}&path=browser-smoke.txt`);
    assert.equal(f.state.tagReads.length, 0);
    const response = await f.window.fetch(`${route}?repository_id=${id('repo', 1)}&path=browser-smoke.txt`);
    assert.equal(response, f.responses.at(-1));
    assert.deepEqual(copy(f.state.tagReads[0].body), labels());
    f.state.restore();
});

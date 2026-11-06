// Host fixtures test the maintained observer/oracle, not real Chrome or backup I/O.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import {runInNewContext} from 'node:vm';

const source = readFileSync(new URL('../runner/backup_browse.js', import.meta.url), 'utf8');
const origin = 'http://127.0.0.1:2345';
const copy = value => JSON.parse(JSON.stringify(value));
// Model the runner's JSON Value roundtrip for these ASCII-key wire fixtures.
// Object keys are sorted; array positions and scalar types are unchanged.
function rustJsonRoundtrip(value) {
    if (value === null || typeof value !== 'object') return value;
    if (Array.isArray(value)) return value.map(rustJsonRoundtrip);
    return Object.fromEntries(Object.keys(value).sort().map(key => [key, rustJsonRoundtrip(value[key])]));
}
const disks = ['P01', 'B01', 'C01'].map((slot_id, index) => ({
    slot_id, name: `real-${slot_id}`, level: index + 1, online: true, writable: index === 0
}));
const labels = ['New folder', 'Move', 'Copy', 'Rename', 'Delete', 'Create folder branch', 'Primary placement', 'Backup settings'];
const text = value => ({textContent: value});
const button = (label, disabled = false) => ({...text(label), disabled,
    getAttribute: name => name === 'aria-label' ? null : undefined});
// Complete current-owned-file shape from ListingEntryResponse and
// ContentDispositionResponse, in the observed wire field order. Values remain
// host fixtures, not captured I/O or real user identities.
const base = {entry_id: 'entry-one', name: 'browser-smoke.txt',
    path: 'browser-fixtures/browser-smoke.txt', kind: 'file', size: '34',
    file_id: 'file-one', version_id: 'version-two', materialized: true, inherited: false,
    own_version_id: 'version-two', source_version_id: null,
    content_state: 'AVAILABLE', visible_content_state: 'AVAILABLE',
    repository_id: 'repository-one', repository_path: 'browser-smoke.txt', container: true,
    selected_branch_id: 'branch-main', note: '', node_id: 'node-two', read_only: false};
function file(path, backup = false) {
    return {...base, path, ...(backup ? {backup: true, read_only: true, historical_only: false} : {})};
}
function directory(path, backup = false) {
    return {entry_id: `directory-${path}`, name: path.split('/').at(-1), path,
        kind: 'directory', size: '0', file_id: '', version_id: '',
        ...(backup ? {backup: true, read_only: true} : {})};
}
function page(slot, path, entries) {
    const backup = slot !== 'P01', repository = !backup && path === 'browser-fixtures';
    return {slot_id: slot, path, ...(backup ? {backup: true} : {}), writable: !backup,
        entries, selected_entry: null, context: repository ? {repository_id: 'repository-one',
            repository_path: '', branch_id: 'branch-main', base_head: 'commit-two'} : null,
        directory_branches: repository ? [{directory_branch_id: 'directory-branch',
            repository_id: 'repository-one', directory_id: 'directory-one', name: 'main',
            parent_branch_id: null, head_tree_id: 'tree-two', commit_id: 'directory-commit-two',
            path: '', materialized: true, current: true, sequence: '2', file_count: '1'}] : [],
        directory_branches_supported: repository, default_branch_scope: repository,
        directory_file_count: String(entries.filter(entry => entry.kind === 'file').length),
        total_entries: String(entries.length), snapshot: 'real-snapshot', previous_cursor: null, next_cursor: null};
}
function view(body) {
    return {ready: true, breadcrumbs: ['All disks', `real-${body.slot_id}`, ...body.path.split('/').filter(Boolean)],
        rows: body.entries.map(entry => ({name: entry.name, type: entry.kind === 'file' ? 'File' : 'Folder',
            disabled: false, checked: false, checkboxDisabled: body.backup === true})),
        readonlyNotice: true, readonlyControls: true};
}
function dom() {
    const state = {mode: 'root', body: null, details: null, overrides: {}};
    const document = {
        querySelectorAll(selector) {
            if (selector === '.zxs-disk-table tbody tr') return state.mode !== 'root' ? [] : disks.map(disk => ({
                dataset: {diskLevel: String(disk.level), online: String(disk.online)},
                cells: [text(''), text(''), text(disk.writable ? 'Read/write directory' : 'Read only')],
                querySelector: () => ({...button(disk.name, !disk.online), dataset: {slot: disk.slot_id}})
            }));
            const current = state.body ? {...view(state.body), ...state.overrides} : null;
            if (selector === '.zxs-breadcrumb button') return state.mode === 'directory' ? current.breadcrumbs.map(text) : [];
            if (selector === '.zxs-file-toolbar button') return labels.map(label => button(label, current?.readonlyControls));
            throw new Error(`unexpected document list selector ${selector}`);
        },
        querySelector(selector) {
            if (selector === '.zxs-breadcrumb') return state.mode === 'directory' ? {} : null;
            if (selector === '.zxs-file-manager') return state.mode === 'detail' ? null : {
                getAttribute: () => state.overrides.ready === false ? 'true' : 'false',
                querySelectorAll: () => state.overrides.readonlyNotice === false ? [] : [text('This disk directory is read only.')]
            };
            if (selector === '[data-zxs-set-directory-default]') return {disabled: state.overrides.readonlyControls !== false};
            if (selector === '.zxs-file-table:not(.zxs-disk-table)') {
                if (state.mode !== 'directory') return null;
                const current = {...view(state.body), ...state.overrides};
                return {
                    querySelector: () => ({disabled: current.readonlyControls}),
                    querySelectorAll: () => current.rows.map(row => ({
                        cells: [text(''), text(row.type)], querySelector(selector) {
                            if (selector === '.zxs-file-name') return button(row.name, row.disabled);
                            if (selector === '.zxs-entry-checkbox') return {disabled: row.checkboxDisabled, checked: row.checked};
                            throw new Error(`unexpected row selector ${selector}`);
                        }
                    }))
                };
            }
            if (selector === '.zxs-entry-content') {
                if (state.mode !== 'detail') return null;
                const selected = state.details;
                return {
                    querySelectorAll(selector) {
                        if (selector === 'button') return state.overrides.controls || [];
                        if (selector === '[data-node-id]') return [{dataset: {nodeId: selected.file.node_id,
                            bodyVersionId: state.overrides.version || selected.file.version_id}}];
                        throw new Error(`unexpected detail list selector ${selector}`);
                    },
                    querySelector(selector) {
                        if (selector === '.zxs-file-heading h2') return text(state.overrides.name || selected.file.name);
                        if (selector === '.zxs-file-properties dd') return text(state.overrides.location || `${selected.slot}/${selected.file.path}`);
                        if (selector === '.zxs-entry-version-history') return {getAttribute: () => state.overrides.busy ? 'true' : 'false'};
                        if (selector === '[data-zxs-set-file-default]') return {disabled: state.overrides.defaultDisabled !== false};
                        throw new Error(`unexpected detail selector ${selector}`);
                    }
                };
            }
            throw new Error(`unexpected document selector ${selector}`);
        }
    };
    return {state, document};
}
function fixture(responder = () => ({disks})) {
    const calls = [], responses = [], {state: render, document} = dom();
    const original = async function(...args) {
        calls.push({receiver: this, args});
        const result = await responder(...args);
        const response = result instanceof Response ? result : new Response(JSON.stringify(result), {
            headers: {'content-type': 'application/json', 'x-original': 'retained'}
        });
        responses.push(response);
        return response;
    };
    const window = {fetch: original};
    const context = {window, Request, URL, TextDecoder, document, location: {origin, href: `${origin}/`}};
    const install = () => runInNewContext(`(function(){${source}})()`, context);
    assert.equal(install(), true);
    const p = window.__zxsBackupBrowse;
    p.start();
    return {p, window, original, calls, responses, install, render};
}
const endpoint = (slot, path, extra = '') => `${origin}/api/file-manager/disks/${slot}/entries?path=${encodeURIComponent(path)}&query=&limit=200${extra}`;
async function load(f, body, extra = '') {
    const after = f.p.mark();
    f.render.mode = 'directory'; f.render.body = body;
    await f.window.fetch(endpoint(body.slot_id, body.path, extra));
    return after;
}
const historyBody = (file, kind = 'nodes') => ({kind, identity: {file_id: file.file_id, history_root: 'root-one',
    context: {repository_id: file.repository_id, path: file.repository_path}}, nodes: [{node_id: file.node_id, version_id: file.version_id}]});

// This is a pass-through, not the synthetic disk retry or Unknown fault probe.
test('observation preserves exact Request, receiver, real Response, bytes and headers', async () => {
    const body = page('B01', '', [directory('P01', true)]);
    const f = fixture(() => body), request = new Request(endpoint('B01', ''));
    const receiver = {owner: 'fetch caller'};
    const response = await f.window.fetch.call(receiver, request);
    assert.equal(f.calls[0].receiver, receiver);
    assert.equal(f.calls[0].args[0], request);
    assert.equal(response, f.responses[0]);
    assert.equal(response.bodyUsed, false);
    assert.equal(response.headers.get('x-original'), 'retained');
    assert.deepEqual(await response.json(), body);
    assert.equal(f.p.requests[0].raw, JSON.stringify(body));
    assert.equal(f.p.requests[0].complete, true);
    f.p.restore();
});

test('real HTTP failures and original network errors remain failures without manufactured data', async () => {
    const f = fixture(() => new Response('{"error":{"code":"UNAUTHENTICATED"}}', {status: 401}));
    const response = await f.window.fetch(endpoint('B01', ''));
    assert.equal(response, f.responses[0]); assert.equal(response.status, 401);
    assert.throws(() => f.p.page('B01', '', 0), /HTTP 401/);
    f.p.restore();
    const error = new TypeError('original network rejection');
    const g = fixture(() => { throw error; });
    await assert.rejects(g.window.fetch(endpoint('C01', '')), actual => actual === error);
    assert.throws(() => g.p.mark(), /original fetch rejected/);
    g.p.restore();
});

test('invalid JSON and recording byte budgets preserve the original response but fail proof', async () => {
    for (const bytes of ['{invalid', 'x'.repeat(1024 * 1024 + 1)]) {
        const f = fixture(() => new Response(bytes));
        const response = await f.window.fetch(endpoint('B01', ''));
        assert.equal(response, f.responses[0]); assert.equal(response.bodyUsed, false);
        assert.equal(await response.text(), bytes);
        assert.throws(() => f.p.page('B01', '', 0), /observation|budget/);
        f.p.restore();
    }
});

test('guard blocks own mutation/content before sending without claiming server rejection', async () => {
    for (const [path, method] of [['/api/file-manager/files/file-one/tags', 'POST'],
        ['/api/file-manager/disks/B01/entries', 'DELETE'], ['/api/content/version-two', 'GET'],
        ['/api/versions/version-two/download', 'HEAD']]) {
        const f = fixture();
        await assert.rejects(f.window.fetch(`${origin}${path}`, {method}), /before sending/);
        assert.equal(f.calls.length, 0); assert.equal(f.p.blocked.length, 1);
        assert.throws(() => f.p.finish(), /forbidden request/);
        f.p.restore();
    }
});

test('other origins and unrelated real GETs are unchanged and not captured with credentials', async () => {
    const f = fixture(() => ({value: 1}));
    for (const request of [new Request('https://other.example/api/anything', {method: 'POST', body: 'secret-body'}),
        new URL(`${origin}/api/management/status`), new Request(`${origin}/api/activity`, {headers: {authorization: 'secret-header'}})]) {
        const response = await f.window.fetch(request);
        assert.equal(response, f.responses.at(-1));
    }
    assert.equal(f.p.requests.length, 0); assert.equal(f.p.blocked.length, 0);
    assert.doesNotMatch(JSON.stringify(f.p.snapshot()), /secret-body|secret-header|authorization/);
    f.p.restore();
});

test('cleanup rejects overlapping ownership and does not strand a pending original read', async () => {
    let release;
    const pending = new Promise(resolve => { release = resolve; });
    const f = fixture(async () => { await pending; return {disks}; });
    assert.throws(f.install, /already installed/);
    const read = f.window.fetch(`${origin}/api/file-manager/disks`);
    f.p.restore(); release();
    assert.equal(await read, f.responses[0]);
    assert.equal(f.window.fetch, f.original);
    assert.equal(f.window.__zxsBackupBrowse, undefined);
    const g = fixture(); const other = () => {};
    g.window.fetch = other;
    assert.throws(() => g.p.restore(), /replaced/);
    assert.equal(g.window.fetch, other, 'cleanup may not clobber another observer');
});

test('fresh exact slot/path/query and one-click one-read are required', async () => {
    const body = page('B01', '', [directory('P01', true)]), f = fixture(() => body);
    const after = await load(f, body);
    assert.equal(f.p.pageReady('B01', '', after), true);
    assert.equal(f.p.pageReady('C01', '', after), false);
    assert.equal(f.p.pageReady('B01', 'P01', after), false);
    assert.equal(f.p.pageReady('B01', '', after + 1), false, 'old response cannot satisfy a later click');
    await f.window.fetch(endpoint('B01', ''));
    assert.throws(() => f.p.pageReady('B01', '', after), /duplicate reads/);
    f.p.restore();
    for (const extra of ['&cursor=old', '&path=', '&directory_branch_id=foreign', '&query=hidden']) {
        const g = fixture(() => body); await load(g, body, extra);
        assert.throws(() => g.p.page('B01', '', 0), /exact complete/); g.p.restore();
    }
});

test('incomplete pages, wrong slots/paths, previews and writable backup context fail', async () => {
    const baseline = page('B01', '', [directory('P01', true)]);
    for (const delta of [{slot_id: 'C01'}, {path: 'P01'}, {writable: true}, {backup: false},
        {preview: false}, {next_cursor: 'cursor'}, {selected_entry: {}}, {snapshot: ''},
        {total_entries: '2'}, {context: {}}, {directory_branches_supported: true},
        {default_branch_scope: true}, {directory_branches: [{}]}]) {
        const body = {...baseline, ...delta}, f = fixture(() => body);
        await f.window.fetch(endpoint('B01', ''));
        assert.throws(() => f.p.page('B01', '', 0), /exact complete|mutable repository/);
        f.p.restore();
    }
});

test('wrong or duplicate children and missing read-only flags cannot satisfy a backup page', async () => {
    for (const entries of [[directory('P01', true), directory('P01', true)],
        [{...directory('P01', true), path: 'foreign/P01'}],
        [{...directory('P01', true), read_only: false}],
        [{...directory('P01', true), backup: false}], [directory('../P01', true)]]) {
        const body = page('C01', '', entries), f = fixture(() => body);
        await load(f, body);
        assert.throws(() => f.p.page('C01', '', 0), /duplicate, foreign child or writable/);
        f.p.restore();
    }
});

test('actual DOM selectors capture breadcrumb/row kinds and disabled navigation controls', async () => {
    const body = page('C01', 'P01', [directory('P01/browser-fixtures', true)]), f = fixture(() => body);
    const after = await load(f, body);
    assert.equal(f.p.pageReady('C01', 'P01', after), true);
    assert.equal(f.p.step('C01', 'P01', after).path, 'P01/browser-fixtures');
    assert.deepEqual(copy(f.p.pages[0].view), view(body));
    for (const overrides of [{ready: false}, {breadcrumbs: ['All disks', 'real-B01', 'P01']},
        {readonlyNotice: false}, {readonlyControls: false}, {rows: []},
        {rows: [{...view(body).rows[0], checkboxDisabled: false}]},
        {rows: [{...view(body).rows[0], checked: true}]},
        {rows: [{...view(body).rows[0], type: 'File'}]}]) {
        f.render.overrides = overrides;
        assert.equal(f.p.pageReady('C01', 'P01', after), false, JSON.stringify(overrides));
    }
    f.p.restore();
});

test('prefix planning follows actual entries for current or nested layouts without guessing policy', () => {
    const f = fixture();
    for (const prefix of ['P01', 'B01/P01']) {
        const pieces = prefix.split('/'); let path = '';
        for (const name of pieces) {
            const actual = directory(path ? `${path}/${name}` : name, true);
            assert.equal(f.p.next(page('C01', path, [actual])).path, actual.path);
            path = actual.path;
        }
        const actual = directory(`${path}/browser-fixtures`, true);
        assert.equal(f.p.next(page('C01', path, [directory(`${path}/other`, true), actual])).path, actual.path);
    }
    for (const entries of [[], [directory('one', true), directory('two', true)], [file('browser-smoke.txt', true)]]) {
        assert.throws(() => f.p.next(page('C01', '', entries)), /missing or ambiguous/);
    }
    f.p.restore();
});

test('same-name files require the actual primary repository/file/node/version and unchanged return entry', () => {
    const f = fixture(), primary = file('browser-fixtures/browser-smoke.txt');
    f.p.next(page('P01', 'browser-fixtures', [primary]));
    for (const key of ['file_id', 'repository_id', 'version_id', 'node_id', 'size']) {
        const target = {...file('P01/browser-fixtures/browser-smoke.txt', true), [key]: 'different'};
        assert.throws(() => f.p.next(page('C01', 'P01/browser-fixtures', [target])), /differs/);
    }
    const target = file('P01/browser-fixtures/browser-smoke.txt', true);
    assert.deepEqual(copy(f.p.next(page('C01', 'P01/browser-fixtures', [target]))), target);
    assert.throws(() => f.p.next(page('P01', 'browser-fixtures', [{...primary, note: 'changed'}])), /changed the primary/);
    f.p.restore();
});

test('backup detail requires a fresh matching real history and read-only exact slot/path render', async () => {
    const selected = file('P01/browser-fixtures/browser-smoke.txt', true);
    const f = fixture(input => historyBody(selected, String(input).endsWith('/branches') ? 'branches' : 'nodes'));
    f.render.mode = 'detail'; f.render.details = {slot: 'C01', file: selected};
    assert.equal(f.p.detailReady('C01', selected, 0), false);
    await f.window.fetch(`${origin}/api/file-manager/files/${selected.file_id}/history`);
    assert.equal(f.p.detailReady('C01', selected, 0), false, 'nodes alone do not prove settled file details');
    await f.window.fetch(`${origin}/api/file-manager/files/${selected.file_id}/history/branches`);
    assert.equal(f.p.detailReady('C01', selected, 0), true);
    assert.equal(f.p.detailReady('C01', selected, 1), false);
    assert.equal(f.p.detailReady('B01', selected, 0), false);
    for (const overrides of [{name: 'other.txt'}, {location: `B01/${selected.path}`},
        {busy: true}, {version: 'wrong-body-version'}, {defaultDisabled: false},
        {controls: [button('Edit tags')]}, {controls: [button('Delete')]}]) {
        f.render.overrides = overrides;
        assert.equal(f.p.detailReady('C01', selected, 0), false, JSON.stringify(overrides));
    }
    f.render.overrides = {};
    assert.equal(f.p.recordDetail('C01', selected, 0), true);
    assert.throws(() => f.p.recordDetail('C01', selected, 0), /visited twice/);
    f.p.restore();
});

test('disk list return cannot be stale, wrong-level, wrong-name or missing one real tier', async () => {
    let body = {disks}; const f = fixture(() => body);
    await f.window.fetch(`${origin}/api/file-manager/disks`);
    assert.equal(f.p.rootReady(0), true); assert.equal(f.p.rootReady(1), false);
    for (const actual of [disks.slice(0, 2), disks.map(disk => ({...disk, level: 1})),
        disks.map(disk => ({...disk, name: 'wrong'})), disks.map(disk => ({...disk, writable: true}))]) {
        body = {disks: actual}; const after = f.p.mark();
        await f.window.fetch(`${origin}/api/file-manager/disks`);
        if (actual.length !== 3) assert.throws(() => f.p.rootReady(after), /complete real disk list/);
        else assert.equal(f.p.rootReady(after), false);
    }
    f.p.restore();
});

test('bounded observation refuses a 65th matching request before sending', async () => {
    const f = fixture();
    for (let i = 0; i < 64; i++) await f.window.fetch(`${origin}/api/file-manager/disks`);
    await assert.rejects(f.window.fetch(`${origin}/api/file-manager/disks`), /budget/);
    assert.equal(f.calls.length, 64); assert.throws(() => f.p.finish(), /budget/);
    f.p.restore();
});

test('completion survives Rust-sorted file keys and still requires both backups, fresh returns and primary recheck', async () => {
    let response;
    const f = fixture(() => response);
    const primary = file('browser-fixtures/browser-smoke.txt');
    async function root() {
        f.render.mode = 'root'; response = {disks}; const after = f.p.mark();
        await f.window.fetch(`${origin}/api/file-manager/disks`); f.p.recordRoot(after);
    }
    async function visitRoot(slot) {
        response = page(slot, '', [directory('browser-fixtures', slot !== 'P01')]);
        const after = await load(f, response); f.p.step(slot, '', after);
    }
    async function primaryRead() {
        await visitRoot('P01');
        response = page('P01', 'browser-fixtures', [primary]);
        const after = await load(f, response); f.p.step('P01', response.path, after);
        await root();
    }
    await primaryRead();
    for (const slot of ['B01', 'C01']) {
        await visitRoot(slot);
        const selected = file('browser-fixtures/browser-smoke.txt', true);
        const returned = copy(rustJsonRoundtrip(selected));
        assert.deepEqual(returned, selected);
        assert.notDeepEqual(Object.keys(returned), Object.keys(selected));
        response = page(slot, 'browser-fixtures', [selected]);
        const after = await load(f, response); f.p.step(slot, response.path, after);
        f.render.mode = 'detail'; f.render.details = {slot, file: selected};
        response = historyBody(selected); const historyAfter = f.p.mark();
        await f.window.fetch(`${origin}/api/file-manager/files/${selected.file_id}/history`);
        response = historyBody(selected, 'branches');
        await f.window.fetch(`${origin}/api/file-manager/files/${selected.file_id}/history/branches`);
        f.p.recordDetail(slot, returned, historyAfter); await root();
        assert.throws(() => f.p.finish(), /both real levels and restore/);
    }
    await primaryRead(); assert.equal(f.p.finish(), true);
    const saved = f.p.details.pop(); assert.throws(() => f.p.finish(), /both real levels/); f.p.details.push(saved);
    assert.equal(f.p.finish(), true);
    assert.equal(f.p.snapshot().blocked.length, 0);
    assert.ok(f.p.snapshot().requests.every(row => row.raw && row.complete));
    f.p.restore();
});

// Isolate the same() helper without exposing it in the installed browser probe.
// The existing full navigation case above also exercises it through real hooks.
test('JSON equality ignores nested object key order but preserves keys, scalar types and array order', () => {
    assert.match(source, /return true;\s*$/);
    const same = runInNewContext(`(function(){${source.replace(/return true;\s*$/, 'return same;')}})()`,
        {window: {fetch() { throw new Error('comparison must not fetch'); }}});
    const value = {identity: {node: 'node-two', file: 'file-one'},
        history: [{parents: ['node-one', 'node-two'], selected: true}, null, '34']};
    assert.equal(same(value, rustJsonRoundtrip(value)), true);
    assert.equal(same(rustJsonRoundtrip(value), value), true);
    const reversedKeys = Object.fromEntries(Object.entries(value).reverse());
    assert.equal(same(value, reversedKeys), true);
    for (const [left, right] of [
        [{x: '34'}, {x: 34}], [{x: true}, {x: 1}], [{x: null}, {x: false}],
        [{x: null}, {}], [{x: null}, {x: null, extra: null}],
        [{left: null}, {right: null}], [[1, 2], [2, 1]], [[1], [1, null]],
        [[1, 2], {'0': 1, '1': 2}], [null, {}],
        [value, {...value, history: [null, value.history[0], '34']}],
        [value, {...value, history: [{parents: ['node-two', 'node-one'], selected: true}, null, '34']}],
        [value, {...value, identity: {...value.identity, node: 'another-node'}}]
    ]) {
        assert.equal(same(left, right), false, JSON.stringify([left, right]));
        assert.equal(same(right, left), false, JSON.stringify([right, left]));
    }
});

async function backupReturnFixture() {
    let response;
    const f = fixture(() => response);
    f.p.next(page('P01', 'browser-fixtures', [file('browser-fixtures/browser-smoke.txt')]));
    const selected = file('P01/browser-fixtures/browser-smoke.txt', true);
    for (const [path, entry] of [['', directory('P01', true)],
        ['P01', directory('P01/browser-fixtures', true)], ['P01/browser-fixtures', selected]]) {
        response = page('B01', path, [entry]);
        const after = await load(f, response); f.p.step('B01', path, after);
    }
    f.render.mode = 'detail'; f.render.details = {slot: 'B01', file: selected};
    const historyAfter = f.p.mark();
    response = historyBody(selected);
    await f.window.fetch(`${origin}/api/file-manager/files/${selected.file_id}/history`);
    response = historyBody(selected, 'branches');
    await f.window.fetch(`${origin}/api/file-manager/files/${selected.file_id}/history/branches`);
    f.p.recordDetail('B01', copy(rustJsonRoundtrip(selected)), historyAfter);
    f.render.mode = 'root'; response = {disks}; const after = f.p.mark();
    await f.window.fetch(`${origin}/api/file-manager/disks`);
    return {f, after, selected, historyAfter};
}

test('backup return compares every file field after JSON roundtrip without accepting omissions or permission changes', async () => {
    const {f, after, selected} = await backupReturnFixture();
    const original = f.p.details[0].file;
    const cases = [];
    for (const key of Object.keys(selected)) {
        const changed = copy(rustJsonRoundtrip(selected));
        changed[key] = typeof changed[key] === 'string' ? `${changed[key]}-different`
            : typeof changed[key] === 'boolean' ? !changed[key] : 'not-null';
        const missing = copy(rustJsonRoundtrip(selected)); delete missing[key];
        cases.push([`changed ${key}`, changed], [`missing ${key}`, missing]);
    }
    cases.push(['extra field', {...original, extra: null}], ['numeric size', {...original, size: 34}],
        ['numeric permission', {...original, read_only: 1}], ['string permission', {...original, read_only: 'true'}]);
    for (const [name, candidate] of cases) {
        f.p.details[0].file = candidate;
        assert.throws(() => f.p.recordRoot(after), /exact selected file detail/, name);
        assert.equal(f.p.returns.length, 0);
    }
    f.p.details[0].file = original;
    assert.equal(f.p.recordRoot(after), true);
    f.p.restore();
});

test('order-independent file comparison does not weaken backup slot or history timing', async () => {
    const {f, after, historyAfter} = await backupReturnFixture();
    const detail = f.p.details[0];
    for (const change of [{slot: 'C01'}, {historyAfter: historyAfter - 1}, {historyAfter: after + 1}]) {
        f.p.details[0] = {...detail, ...change};
        assert.throws(() => f.p.recordRoot(after), /exact selected file detail/);
        assert.equal(f.p.returns.length, 0);
    }
    f.p.restore();
});


test('a detail or same final filename alone cannot prove disk-root navigation', async () => {
    let response = page('P01', 'browser-fixtures', [file('browser-fixtures/browser-smoke.txt')]);
    const f = fixture(() => response);
    const after = await load(f, response); f.p.step('P01', 'browser-fixtures', after);
    f.render.mode = 'root'; response = {disks}; const rootAfter = f.p.mark();
    await f.window.fetch(`${origin}/api/file-manager/disks`);
    assert.throws(() => f.p.recordRoot(rootAfter), /continuous actual root-to-file/);
    f.p.restore();
});

test('aggregate response recording cannot grow beyond four MiB', async () => {
    const raw = JSON.stringify({padding: 'x'.repeat(900000)});
    const f = fixture(() => new Response(raw));
    for (let i = 0; i < 5; i++) {
        const response = await f.window.fetch(`${origin}/api/file-manager/disks`);
        assert.equal(await response.text(), raw);
    }
    assert.equal(f.p.overflow, true); assert.throws(() => f.p.finish(), /budget/);
    f.p.restore();
});


test('backup detail rejects another file/repository/root in the auxiliary history response', async () => {
    const selected = file('P01/browser-fixtures/browser-smoke.txt', true);
    for (const delta of [{file_id: 'another-file'}, {history_root: 'another-root'},
        {context: {repository_id: 'another-repository', path: selected.repository_path}},
        {context: {repository_id: selected.repository_id, path: 'another-alias.txt'}}]) {
        const f = fixture(input => {
            const body = historyBody(selected, String(input).endsWith('/branches') ? 'branches' : 'nodes');
            if (body.kind === 'branches') Object.assign(body.identity, delta);
            return body;
        });
        f.render.mode = 'detail'; f.render.details = {slot: 'B01', file: selected};
        await f.window.fetch(`${origin}/api/file-manager/files/${selected.file_id}/history`);
        await f.window.fetch(`${origin}/api/file-manager/files/${selected.file_id}/history/branches`);
        assert.equal(f.p.detailReady('B01', selected, 0), false);
        f.p.restore();
    }
});


test('host positives use complete current-owned-file and virtual-directory wire shapes', () => {
    const primary = file('browser-fixtures/browser-smoke.txt');
    assert.deepEqual(Object.keys(primary).sort(), ['entry_id', 'name', 'path', 'kind', 'size', 'file_id',
        'version_id', 'materialized', 'inherited', 'own_version_id', 'source_version_id',
        'content_state', 'visible_content_state', 'repository_id', 'repository_path', 'container',
        'selected_branch_id', 'note', 'node_id', 'read_only'].sort());
    assert.equal(primary.container, true); assert.equal(primary.read_only, false);
    assert.equal(primary.own_version_id, primary.version_id); assert.equal(primary.materialized, true);
    assert.equal(primary.source_version_id, null); assert.equal(primary.inherited, false);
    assert.equal(primary.content_state, primary.visible_content_state);
    const backup = file('P01/browser-fixtures/browser-smoke.txt', true);
    assert.deepEqual(Object.keys(backup).sort(), [...Object.keys(primary), 'backup', 'historical_only'].sort());
    assert.equal(backup.backup, true); assert.equal(backup.read_only, true); assert.equal(backup.historical_only, false);
    assert.deepEqual(Object.keys(directory('P01', true)).sort(), ['entry_id', 'name', 'path', 'kind',
        'size', 'file_id', 'version_id', 'backup', 'read_only'].sort());
    assert.equal(page('P01', '', []).context, null);
    assert.deepEqual(Object.keys(page('P01', 'browser-fixtures', [primary]).context).sort(),
        ['repository_id', 'repository_path', 'branch_id', 'base_head'].sort());
});

test('selected backup must be the same current container, not another entry, repository path or historical-only row', () => {
    const f = fixture(), primary = file('browser-fixtures/browser-smoke.txt');
    f.p.next(page('P01', 'browser-fixtures', [primary]));
    for (const delta of [{entry_id: 'other-entry'}, {entry_id: undefined},
        {container: false}, {container: undefined}, {repository_path: 'other.txt'},
        {repository_path: null}, {repository_path: undefined}, {historical_only: true},
        {historical_only: undefined}]) {
        const selected = {...file('P01/browser-fixtures/browser-smoke.txt', true), ...delta};
        assert.throws(() => f.p.next(page('C01', 'P01/browser-fixtures', [selected])),
            /current file container|identified real file|differs/, JSON.stringify(delta));
    }
    const selected = file('P01/browser-fixtures/browser-smoke.txt', true);
    assert.deepEqual(copy(f.p.next(page('C01', 'P01/browser-fixtures', [selected]))), selected);
    f.p.restore();
});

test('backup prefix directories require an entry identity and remain virtual, without repository/file context', async () => {
    for (const delta of [{entry_id: ''}, {entry_id: undefined}, {repository_id: 'repository-one'},
        {repository_path: 'browser-smoke.txt'}, {container: false}, {selected_branch_id: null}]) {
        const body = page('B01', '', [{...directory('P01', true), ...delta}]);
        const f = fixture(() => body); await load(f, body);
        assert.throws(() => f.p.page('B01', '', 0), /identity|virtual backup directory/);
        f.p.restore();
    }
});

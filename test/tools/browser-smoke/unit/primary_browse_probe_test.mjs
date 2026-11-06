import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';

const source = readFileSync(new URL('../runner/management/primary_browse.js', import.meta.url), 'utf8');
const install = new Function('window', 'location', source);
const origin = 'https://fixture.invalid';
const disk = {slot_id: 'D01', name: 'selected-storage', level: 1, online: true, writable: true};
const root = {slot_id: 'D01', path: '', writable: true, entries: [], total_entries: '0',
    directory_file_count: '0', snapshot: 'real-fixture-snapshot', previous_cursor: null, next_cursor: null};
const rootPath = '/api/file-manager/disks/D01/entries?path=&query=&limit=200';
function fixture(original) {
    const window = {fetch: original};
    install(window, {origin, href: origin + '/'});
    return {window, original, state: window.__zxsPrimaryBrowse,
        read: path => window.fetch(new Request(origin + path))};
}
function response(body, status = 200) {
    return new Response(JSON.stringify(body), {status, headers: {'X-Fixture': 'retained'}});
}
let cases = 0;
{
    const actual = response({disks: [disk]});
    let receiver;
    const f = fixture(function () { receiver = this; return actual; });
    const observed = await f.read('/api/file-manager/disks');
    assert.equal(receiver, f.window);
    assert.equal(observed, actual);
    assert.equal(observed.headers.get('X-Fixture'), 'retained');
    assert.equal(await observed.text(), JSON.stringify({disks: [disk]}));
    assert.equal(f.state.diskReady('selected-storage'), true);
    assert.equal(f.state.diskReady('different'), false);
    f.state.restore();
    assert.equal(f.window.fetch, f.original);
    assert.equal(f.window.__zxsPrimaryBrowse, undefined);
    cases++;
}
{
    const actual = response(root);
    const f = fixture(() => actual);
    assert.equal(await f.read(rootPath), actual);
    assert.equal(await actual.text(), JSON.stringify(root));
    assert.equal(f.state.emptyRootReady(), true);
    f.state.restore(); cases++;
}
for (const change of [{slot_id: 'D02'}, {level: 2}, {online: false}, {writable: false}, {name: 'other'}]) {
    const f = fixture(() => response({disks: [{...disk, ...change}]}));
    await f.read('/api/file-manager/disks');
    assert.equal(f.state.diskReady('selected-storage'), false);
    f.state.restore();
}
cases++;
for (const change of [{slot_id: 'D02'}, {path: 'nested'}, {writable: false}, {backup: true},
    {preview: true}, {entries: [{}]}, {total_entries: '1'}, {directory_file_count: '1'},
    {snapshot: ''}, {next_cursor: 'next'}, {previous_cursor: 'previous'}]) {
    const f = fixture(() => response({...root, ...change}));
    await f.read(rootPath);
    assert.equal(f.state.emptyRootReady(), false);
    f.state.restore();
}
cases++;
for (const query of ['path=nested&query=', 'path=&query=searched', 'path=&query=&cursor=next',
    'path=&query=&directory_branch_id=branch&expected_tree_id=tree', 'query=', 'path=&path=&query=']) {
    const f = fixture(() => response(root));
    await f.read('/api/file-manager/disks/D01/entries?' + query);
    assert.equal(f.state.emptyRootReady(), false);
    f.state.restore();
}
cases++;
{
    const f = fixture(() => response(root));
    await f.window.fetch(new URL('https://other.invalid' + rootPath));
    await f.read('/api/file-manager/disks/D02/entries?path=&query=');
    await f.window.fetch(origin + rootPath, {method: 'HEAD'});
    assert.equal(f.state.entryReads.length, 0);
    await f.window.fetch(new Request(origin + rootPath), {method: 'POST', body: 'not retained'});
    assert.deepEqual(f.state.mutations, [{method: 'POST', path: '/api/file-manager/disks/D01/entries'}]);
    assert.equal(JSON.stringify(f.state).includes('not retained'), false);
    f.state.restore(); cases++;
}
{
    const actual = response({error: {code: 'STORAGE_UNAVAILABLE'}}, 503);
    const f = fixture(() => actual);
    assert.equal(await f.read('/api/file-manager/disks'), actual);
    assert.equal(f.state.diskReady('selected-storage'), false);
    assert.equal(actual.status, 503);
    f.state.restore(); cases++;
}
{
    const actual = new Response('{incomplete');
    const f = fixture(() => actual);
    assert.equal(await f.read(rootPath), actual);
    assert.equal(await actual.text(), '{incomplete');
    assert.equal(typeof f.state.entryReads[0].parseError, 'string');
    assert.equal(f.state.emptyRootReady(), false);
    f.state.restore(); cases++;
}
{
    const failure = new TypeError('real upstream failure');
    const f = fixture(() => Promise.reject(failure));
    await assert.rejects(f.read(rootPath), error => error === failure);
    assert.equal(f.state.entryReads.length, 0);
    f.state.restore(); cases++;
}
{
    let release;
    const actual = response(root);
    const f = fixture(() => new Promise(resolve => { release = resolve; }));
    const pending = f.read(rootPath);
    f.state.restore();
    release(actual);
    assert.equal(await pending, actual);
    assert.equal(f.state.entryReads.length, 0);
    assert.equal(f.window.__zxsPrimaryBrowse, undefined);
    cases++;
}
{
    const f = fixture(() => response({disks: [disk]}));
    for (let index = 0; index < 17; index++) await f.read('/api/file-manager/disks');
    assert.equal(f.state.diskReads.length, 16);
    assert.equal(f.state.overflow, true);
    assert.equal(f.state.diskReady('selected-storage'), false);
    f.state.restore(); cases++;
}
{
    const f = fixture(() => response({disks: [disk]}));
    const observer = f.window.fetch;
    assert.throws(() => install(f.window, {origin, href: origin + '/'}), /already installed/);
    assert.equal(f.window.fetch, observer);
    f.state.restore(); cases++;
}
console.log(`PASS ${cases} primary browse observer cases; not Chrome or WASM acceptance`);

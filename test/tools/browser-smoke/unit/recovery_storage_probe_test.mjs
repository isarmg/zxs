// Observer model only: no production WASM, browser storage implementation or server runs here.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import {runInNewContext} from 'node:vm';

const rust = readFileSync(new URL('../runner/naming.rs', import.meta.url), 'utf8');
const source = rust.match(/browser\.script\(r#"([\s\S]*?)"#\)\?/)[1];
const origin = 'http://127.0.0.1:2345';
const operation = `op_${'a'.repeat(32)}`;
const key = `zxs:folder-create:admin:${operation}`;
const bytes = JSON.stringify({operation, principal: 'admin', path: 'folder'});
const unrelatedKey = 'zxs:browser-smoke:recovery-unrelated';
const copy = value => JSON.parse(JSON.stringify(value));

function fixture(entries = []) {
    const values = new Map(entries), calls = [];
    const localStorage = {
        get length() { return values.size; },
        key(index) { return [...values.keys()][index] ?? null; },
        getItem(key) { return values.get(key) ?? null; },
        setItem(key, value) { values.set(key, value); },
        removeItem(key) { values.delete(key); },
    };
    const original = async (...args) => {
        calls.push(args);
        return new Response('original');
    };
    const window = {fetch: original};
    const context = {window, localStorage, URL, Response, location: {href: `${origin}/`}};
    const install = () => runInNewContext(`(function(){${source}})()`, context);
    return {install, window, localStorage, values, calls, original};
}
function request(path = 'namespace', body = {operation_id: operation}) {
    return new Request(`${origin}/api/file-manager/disks/P01/${path}`, {
        method: 'POST', body: JSON.stringify(body),
    });
}

for (const path of ['namespace', 'directory-branches']) {
    test(`${path} observes the original stored identity before returning its rejection`, async () => {
        const f = fixture([[key, bytes]]);
        assert.equal(f.install(), true);
        const response = await f.window.fetch(request(path));
        assert.equal(response.status, 409);
        assert.deepEqual(copy(f.window.__zxsNaming.recovery), [{key, bytes, operation}]);
        assert.equal(f.localStorage.getItem(key), bytes);
        assert.equal(f.calls.length, 0);
        f.window.__zxsNaming.restore();
        assert.equal(f.window.fetch, f.original);
        assert.equal(f.localStorage.getItem(unrelatedKey), null);
        assert.equal(f.localStorage.getItem(key), bytes);
    });
}

test('batch requests keep their existing independent rejection path', async () => {
    const f = fixture();
    f.install();
    assert.equal((await f.window.fetch(request('namespace-batch'))).status, 409);
    assert.equal(f.window.__zxsNaming.writes.length, 1);
    assert.equal(f.window.__zxsNaming.recovery.length, 0);
});

test('ordinary reads retain original fetch arguments and response', async () => {
    const f = fixture();
    f.install();
    const input = new Request(`${origin}/api/file-manager/disks/P01/entries`);
    assert.equal(await (await f.window.fetch(input)).text(), 'original');
    assert.equal(f.calls[0][0], input);
    assert.equal(f.window.__zxsNaming.writes.length, 0);
});

for (const [name, entries, message] of [
    ['missing record', [], /one retained original/],
    ['ambiguous accounts', [[key, bytes], [`zxs:folder-create:other:${operation}`, bytes]], /one retained original/],
    ['changed operation', [[key, JSON.stringify({operation: 'other', principal: 'admin'})]], /identity/],
    ['empty principal', [[key, JSON.stringify({operation, principal: ''})]], /identity/],
    ['non-text principal', [[key, JSON.stringify({operation, principal: 8})]], /identity/],
    ['malformed record', [[key, '{']], /JSON|property/],
]) {
    test(`${name} cannot count as a captured original`, async () => {
        const f = fixture(entries);
        f.install();
        await assert.rejects(f.window.fetch(request()), message);
        assert.equal(f.window.__zxsNaming.recovery.length, 0);
        assert.equal(f.window.__zxsNaming.writes.length, 0);
        for (const [key, bytes] of entries) assert.equal(f.localStorage.getItem(key), bytes);
    });
}

test('probe never overwrites a previously occupied test key', () => {
    const f = fixture([[unrelatedKey, 'original']]);
    assert.throws(f.install, /already occupied/);
    assert.equal(f.localStorage.getItem(unrelatedKey), 'original');
    assert.equal(f.window.fetch, f.original);
});

test('cleanup exposes changed unrelated data without deleting it', () => {
    const f = fixture();
    f.install();
    f.localStorage.setItem(unrelatedKey, 'changed');
    assert.throws(() => f.window.__zxsNaming.restore(), /Unrelated recovery record changed/);
    assert.equal(f.localStorage.getItem(unrelatedKey), 'changed');
    assert.equal(f.window.fetch, f.original);
});

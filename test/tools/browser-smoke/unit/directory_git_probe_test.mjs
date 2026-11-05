// Observer model only: no Chrome, DOM, production WASM, or server runs here.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import {runInNewContext} from 'node:vm';

const rust = readFileSync(new URL('../runner/directory_git.rs', import.meta.url), 'utf8');
const source = rust.match(/const OBSERVE: &str = r#"([\s\S]*?)"#;/)?.[1];
assert.ok(source, 'the model executes the actual maintained observer');
const origin = 'http://127.0.0.1:2345';
function fixture() {
    const calls = [], responses = [];
    const original = async (...args) => {
        calls.push(args);
        const response = new Response(JSON.stringify({fixture: calls.length}), {
            status: 200, headers: {'Content-Type': 'application/json', 'x-fixture': 'preserved'},
        });
        responses.push(response);
        return response;
    };
    const window = {fetch: original};
    const context = {window, Request, URL, location: {href: `${origin}/`, origin}};
    const install = () => runInNewContext(`(function(){${source}})()`, context);
    assert.equal(install(), true);
    return {calls, responses, original, window, install, state: window.__zxsDirectoryGit};
}
const copy = value => JSON.parse(JSON.stringify(value));

test('observer retains the same GET Response, headers, body and fetch arguments', async () => {
    const f = fixture();
    const request = new Request(`${origin}/api/repositories/repo_1/directory-git?path=`);
    const response = await f.window.fetch(request);
    assert.equal(f.calls[0][0], request);
    assert.equal(response, f.responses[0]);
    assert.equal(response.bodyUsed, false);
    assert.equal(response.headers.get('x-fixture'), 'preserved');
    assert.deepEqual(await response.json(), {fixture: 1});
    assert.deepEqual(copy(f.state.reads), [{status: 200, body: {fixture: 1}}]);
    assert.equal(f.state.writes.length, 0);
});

test('all current Git mutations are observed without changing their bodies or responses', async () => {
    const f = fixture();
    for (const suffix of ['directory-git/initialize', 'directory-git/stage', 'directory-git/children/stage', 'directory-branches/dbr_1/commits']) {
        const path = `/api/repositories/repo_1/${suffix}`;
        const body = {expected_index_generation: '9007199254740993', operation_id: 'op_fixture'};
        const request = new Request(`${origin}${path}`, {method: 'POST', body: JSON.stringify(body)});
        const response = await f.window.fetch(request);
        assert.equal(response, f.responses.at(-1));
        assert.equal(request.bodyUsed, false);
        assert.deepEqual(copy(f.state.writes.at(-1)), {path, body});
        assert.deepEqual(copy(f.state.responses.at(-1)), {status: 200, body: {fixture: f.calls.length}});
    }
    assert.equal(f.state.unexpected.length, 0);
});

test('observer handles string URL and init arguments and records unplanned API writes', async () => {
    const f = fixture();
    await f.window.fetch(`${origin}/api/repositories/repo_1/directory-git/stage`, {method: 'POST', body: '{}'});
    assert.equal(f.state.writes.length, 1);
    await f.window.fetch(`${origin}/api/repositories/repo_1/default-branch`, {method: 'PUT', body: '{}'});
    assert.deepEqual(copy(f.state.unexpected), [{method: 'PUT', path: '/api/repositories/repo_1/default-branch'}]);
});

test('retired routes and old entry query are rejected before sending any request', async () => {
    const f = fixture();
    for (const path of [
        '/api/repositories/repo_1/directory-scopes',
        '/api/file-manager/disks/P01/directory-select',
        '/api/file-manager/disks/P01/entries?entry=old',
        '/api/file-manager/disks/P01/entries?path=folder&entry=old',
    ]) {
        await assert.rejects(f.window.fetch(`${origin}${path}`), /Retired directory request/);
    }
    assert.equal(f.calls.length, 0);
    await f.window.fetch(`${origin}/api/file-manager/disks/P01/entries?path=folder&selected_path=folder/file&selected_entry_id=entry_1`);
    await f.window.fetch('https://other.example/api/repositories/repo_1/directory-scopes');
    assert.equal(f.calls.length, 2);
    assert.equal(f.state.reads.length, 0);
});

test('duplicate installation fails and cleanup restores the original fetch', () => {
    const f = fixture();
    assert.throws(f.install, /already installed/);
    f.state.restore();
    assert.equal(f.window.fetch, f.original);
    assert.equal(f.window.__zxsDirectoryGit, undefined);
});

import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';

const source = readFileSync(new URL('../runner/admin_read_queue.js', import.meta.url), 'utf8');
const install = new Function('window', 'location', 'requestAnimationFrame', source);
const origin = 'https://fixture.invalid';
function fixture(original = async () => new Response('{"real":"bytes"}', {
    status: 200, headers: {'Content-Type': 'application/json', 'X-Fixture': 'kept'}
})) {
    const window = {fetch: original};
    const frames = [];
    install(window, {href: origin, origin}, callback => frames.push(callback));
    return {window, original, probe: window.__zxsAdminRefresh, frames,
        read: path => window.fetch(new Request(origin + path))};
}
let cases = 0;
{
    const f = fixture();
    const response = await f.read('/api/admin/disks');
    assert.equal(response.status, 200);
    assert.equal(response.headers.get('X-Fixture'), 'kept');
    let done = false;
    const body = response.text().then(value => { done = true; return value; });
    await Promise.resolve(); await Promise.resolve();
    assert.equal(done, false, 'headers arrive before the intentionally held real body');
    assert.equal(f.probe.active, 1);
    f.frames.shift()(); f.frames.shift()();
    assert.equal(f.probe.framesPassed, true);
    f.probe.release();
    assert.equal(await body, '{"real":"bytes"}');
    assert.equal(f.probe.active, 0);
    assert.equal(f.probe.peak, 1);
    f.probe.restore(); cases++;
}
{
    const f = fixture();
    const held = await f.read('/api/admin/disks');
    const concurrent = await f.read('/api/admin/disks/registrations');
    assert.equal(f.probe.peak, 2, 'probe detects the unfixed overlapping fetches');
    assert.notEqual(f.probe.paths.length, f.probe.atHold);
    f.probe.release();
    await Promise.all([held.text(), concurrent.text()]);
    assert.equal(f.probe.active, 0);
    f.probe.restore(); cases++;
}
{
    const f = fixture(async () => new Response('unchanged 429 bytes', {status: 429}));
    const response = await f.read('/api/admin/storage/discovered-disks');
    assert.equal(response.status, 429);
    assert.equal(await response.text(), 'unchanged 429 bytes');
    assert.deepEqual(f.probe.failures, [{path: '/api/admin/storage/discovered-disks', status: 429}]);
    assert.equal(f.probe.paths.length, 1, 'probe does not retry or hide failure');
    f.probe.restore(); cases++;
}
{
    const f = fixture();
    await (await f.window.fetch(new Request(origin + '/api/admin/disks/exit', {method: 'POST'}))).text();
    await (await f.read('/api/management/status')).text();
    assert.equal(f.probe.paths.length, 0);
    f.probe.restore(); cases++;
}
{
    const cause = new TypeError('real fetch rejection');
    const f = fixture(async () => { throw cause; });
    await assert.rejects(f.read('/api/admin/disks/exit'), error => error === cause);
    assert.equal(f.probe.active, 0);
    f.probe.restore(); cases++;
}
{
    const f = fixture();
    const response = await f.read('/api/admin/disks');
    const body = response.text();
    f.probe.restore();
    assert.equal(await body, '{"real":"bytes"}');
    assert.equal(f.window.fetch, f.original);
    assert.equal(f.window.__zxsAdminRefresh, undefined);
    assert.equal(f.probe.active, 0);
    cases++;
}
{
    const cause = new DOMException('real response body failure', 'NetworkError');
    const f = fixture(async () => new Response(new ReadableStream({start(controller) {
        controller.error(cause);
    }})));
    const response = await f.read('/api/admin/disks/registrations');
    await assert.rejects(response.text(), error => error === cause);
    assert.equal(f.probe.active, 0);
    f.probe.restore(); cases++;
}
{
    const f = fixture();
    const response = await f.read('/api/admin/disks');
    const reader = response.body.getReader();
    const pending = reader.read();
    await reader.cancel('fixture cancellation');
    assert.equal((await pending).done, true);
    assert.equal(f.probe.active, 0);
    f.probe.restore();
    await Promise.resolve(); await Promise.resolve();
    assert.equal(f.probe.active, 0, 'late released gate cannot double-release occupancy');
    cases++;
}
{
    const lifecycle = readFileSync(new URL('../runner/management_queue_lifecycle.js', import.meta.url), 'utf8');
    const callbacks = new Map();
    let timer = 0;
    const window = {
        Request,
        fetch: async () => new Response('{"seed":true}', {status: 200}),
        setTimeout(callback) { const id = ++timer; callbacks.set(id, callback); return id; },
        clearTimeout(id) { callbacks.delete(id); }
    };
    const original = {...window};
    const frames = [];
    new Function('window', 'location', 'requestAnimationFrame', lifecycle)(window, {href: origin, origin}, callback => frames.push(callback));
    const make = (path, headers = {}) => {
        const controller = new AbortController();
        const request = new window.Request(origin + path, {signal: controller.signal, headers});
        window.setTimeout(() => controller.abort(), 120000);
        return {request, signal: controller.signal};
    };
    const active = make('/api/admin/storage/discovered-disks', {Authorization: 'Bearer fixture-old'});
    const response = window.fetch(active.request);
    const queued = make('/api/admin/disks/readmission');
    // A newer unrelated timer must not become the queued read's deadline.
    const unrelated = make('/api/system/status');
    await (await window.fetch(unrelated.request)).text();
    const probe = window.__zxsManagementQueue;
    assert.equal(probe.held, true);
    probe.timeoutQueued();
    assert.equal(queued.signal.aborted, true);
    assert.equal(active.signal.aborted, false);
    assert.equal(unrelated.signal.aborted, false);
    assert.equal(probe.timedOut.started, false);
    assert.throws(() => probe.timeoutQueued(), /one exact queued/);
    probe.phase = 'new';
    const fresh = make('/api/file-manager/disks', {Authorization: 'Bearer fixture-new'});
    await (await window.fetch(fresh.request)).text();
    assert.equal(probe.newCredentialCaptured, true);
    assert.equal(probe.newCredentialDistinct, true);
    assert.equal(JSON.stringify(probe).includes('fixture-new'), false);
    probe.lateUnauthorized = true;
    probe.release();
    const old = await response;
    assert.equal(old.status, 401);
    assert.equal((await old.json()).error.code, 'UNAUTHORIZED');
    frames.shift()(); frames.shift()();
    assert.equal(probe.framesPassed, true);
    const retained = make('/api/admin/disks/registrations', {Authorization: 'Bearer fixture-new'});
    await (await window.fetch(retained.request)).text();
    assert.equal(probe.credentialChanged, false);
    const publicRead = make('/api/setup/status');
    await (await window.fetch(publicRead.request)).text();
    assert.equal(probe.credentialChanged, false, 'public setup legitimately sends no Bearer');
    // HTTP 200 alone must not hide a missing Bearer after an old 401.
    const cookieOnly = make('/api/admin/disks/registrations');
    await (await window.fetch(cookieOnly.request)).text();
    assert.equal(probe.credentialChanged, true);
    await (await window.fetch(queued.request)).text();
    assert.equal(probe.lateOldStarts, 1, 'probe detects a cancelled old request dispatched late');
    probe.mode = 'rate-limit';
    const limited = await window.fetch(make('/api/admin/storage/discovered-disks').request);
    assert.equal(limited.status, 429);
    assert.equal((await limited.json()).error.code, 'RESOURCE_LIMIT_EXCEEDED');
    assert.equal(probe.rateLimitCalls, 1);
    assert.throws(() => window.setTimeout(() => {}, 120000), /no exact Request owner/);
    probe.restore();
    assert.equal(window.fetch, original.fetch);
    assert.equal(window.Request, original.Request);
    assert.equal(window.setTimeout, original.setTimeout);
    assert.equal(window.clearTimeout, original.clearTimeout);
    cases++;
}
console.log(`PASS ${cases} management queue probe cases (probe validation, not production WASM)`);

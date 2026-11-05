// Host-object tests of the real browser delivery probe. This is not a browser,
// a real service, a disk write, or import success evidence.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {setImmediate} from 'node:timers/promises';
const source = readFileSync(new URL('../runner/foreign_review/delivery.js', import.meta.url), 'utf8');
const install = new Function('window', 'Request', 'URL', 'location', source);
const origin = 'http://127.0.0.1:43123';
const location = {href: `${origin}/`, origin};
const route = part => `/api/admin/storage/foreign/${part}`;
let checks = 0;
function fixture(responder = async () => new Response('{"execution":"unavailable"}', {status: 200})) {
  const calls = [];
  const original = async (...args) => { calls.push(args); return responder(...args); };
  const window = {fetch: original};
  assert.equal(install(window, Request, URL, location), true);
  return {window, original, calls, probe: window.__zxsForeignReview};
}
async function until(predicate) {
  const deadline = Date.now() + 5000;
  while (!predicate()) { assert.ok(Date.now() < deadline, 'delivery probe did not reach expected state'); await setImmediate(); }
}
async function test(name, action) { await action(); checks++; console.log(`PASS ${name}`); }
await test('live Request and actual response remain unconsumed', async () => {
  const body = {preview: {request: {operation_id: 'original-id'}}, acknowledge_plan_only: true};
  const response = new Response(JSON.stringify({validation: 'required', execution: 'unavailable'}));
  const f = fixture(async request => { assert.equal(request.bodyUsed, false); assert.deepEqual(await request.clone().json(), body); return response; });
  const request = new Request(origin + route('plans/save'), {method: 'POST', body: JSON.stringify(body)});
  const result = await f.window.fetch(request);
  assert.equal(result, response); assert.equal(request.bodyUsed, false); assert.equal(response.bodyUsed, false);
  assert.equal(f.calls[0][0], request); assert.deepEqual(f.probe.records[0].body, body);
  assert.equal((await result.json()).execution, 'unavailable');
});
await test('Request init override is observed without replacing the forwarded object', async () => {
  const f = fixture();
  const request = new Request(origin + route('plans/save'), {method: 'POST', body: '{"old":true}'});
  const init = {method: 'POST', body: '{"new":true}'};
  await f.window.fetch(request, init);
  assert.deepEqual(f.probe.records[0].body, {new: true}); assert.equal(request.bodyUsed, false);
  assert.equal(f.calls[0][0], request); assert.equal(f.calls[0][1], init);
});
await test('relative URL works and status keeps the exact operation query', async () => {
  const f = fixture(); await f.window.fetch(route('plans/status') + '?operation_id=op_exact');
  assert.equal(f.probe.records[0].kind, 'status'); assert.equal(f.probe.records[0].query, '?operation_id=op_exact');
  assert.equal(f.probe.records[0].body, null);
});
await test('cross-origin and unrelated routes are forwarded and never faulted', async () => {
  const f = fixture(); f.probe.loseNextSave = true;
  await f.window.fetch('https://other.example/api/admin/storage/foreign/plans/save', {method: 'POST'});
  await f.window.fetch('/api/management/status');
  assert.equal(f.calls.length, 2); assert.deepEqual(f.probe.records, []); assert.equal(f.probe.loseNextSave, true);
});
await test('a failed save stays a real failure and cannot become a lost success', async () => {
  const response = new Response('{"code":"PlanStale"}', {status: 409});
  const f = fixture(async () => response); f.probe.loseNextSave = true;
  assert.equal(await f.window.fetch(route('plans/save'), {method: 'POST'}), response);
  assert.equal(f.probe.records[0].status, 409); assert.equal(f.probe.records[0].delivery, 'delivered');
  assert.equal(f.probe.loseNextSave, true);
});
await test('only an actual successful save delivery can be lost', async () => {
  let writes = 0;
  const f = fixture(async () => { writes++; return new Response('{"validation":"required","execution":"unavailable"}'); });
  f.probe.loseNextSave = true;
  await assert.rejects(f.window.fetch(route('plans/save'), {method: 'POST', body: '{"operation":"one"}'}), TypeError);
  assert.equal(writes, 1); assert.equal(f.probe.records[0].status, 200);
  assert.equal(f.probe.records[0].delivery, 'lost_after_actual_success');
  assert.equal(f.probe.records[0].response.execution, 'unavailable'); assert.equal(f.probe.loseNextSave, false);
});
await test('retry observation preserves original operation and complete request', async () => {
  const f = fixture(); const body = JSON.stringify({preview: {request: {operation_id: 'same', names: {one: 'first', two: 'second'}}}, acknowledge_plan_only: true});
  f.probe.loseNextSave = true;
  await assert.rejects(f.window.fetch(route('plans/save'), {method: 'POST', body}), TypeError);
  await f.window.fetch(route('plans/save'), {method: 'POST', body});
  assert.deepEqual(f.probe.records[0].body, f.probe.records[1].body); assert.equal(f.calls.length, 2);
});
await test('held original preview resolves only when explicitly released', async () => {
  const response = new Response('{"review_digest":"original"}');
  const f = fixture(async () => response); f.probe.holdNext = 'review_preview';
  let settled = false;
  const result = f.window.fetch(route('plans/preview'), {method: 'POST'}).then(value => { settled = true; return value; });
  await until(() => f.probe.pendingIds().length === 1);
  assert.equal(settled, false); assert.deepEqual(f.probe.pendingIds(), [0]);
  f.probe.release(0); assert.equal(await result, response); assert.equal(f.probe.records[0].delivery, 'released');
  assert.throws(() => f.probe.release(0), /no held/);
});
await test('cleanup releases all held original responses and restores ownership', async () => {
  const f = fixture(); f.probe.holdNext = 'save';
  const first = f.window.fetch(route('plans/save'), {method: 'POST'});
  await until(() => f.probe.pendingIds().length === 1);
  f.probe.holdNext = 'status'; const second = f.window.fetch(route('plans/status'));
  await until(() => f.probe.pendingIds().length === 2);
  assert.deepEqual(f.probe.pendingIds(), [0, 1]); f.probe.restore();
  assert.equal(f.window.fetch, f.original); assert.deepEqual(f.probe.pendingIds(), []);
  assert.equal((await first).status, 200); assert.equal((await second).status, 200); f.probe.restore();
});
await test('cleanup never overwrites another fetch owner', async () => {
  const f = fixture(); const replacement = async () => new Response('{}'); f.window.fetch = replacement;
  assert.throws(() => f.probe.restore(), /another owner/); assert.equal(f.window.fetch, replacement);
});
await test('transport failure identity is preserved', async () => {
  const error = new TypeError('host transport failed'); const f = fixture(async () => { throw error; });
  await assert.rejects(f.window.fetch(route('plans/preview'), {method: 'POST'}), value => value === error);
  assert.equal(f.probe.records[0].delivery, 'transport_failed'); assert.equal(f.probe.records[0].status, null);
});
await test('malformed observation never manufactures a corrected body', async () => {
  const response = new Response('not JSON', {status: 500}); const f = fixture(async () => response);
  assert.equal(await f.window.fetch(route('plans/save'), {method: 'POST', body: 'bad JSON'}), response);
  assert.equal(f.probe.records[0].requestParseFailed, true); assert.equal(f.probe.records[0].responseParseFailed, true);
  assert.equal(await response.text(), 'not JSON');
});
await test('future writer and execute routes are recorded as forbidden but not simulated', async () => {
  const f = fixture(async () => new Response('{"code":"FormatUnsupported"}', {status: 400}));
  for (const path of ['plans/execute', 'execution-intents/save', 'executions/commit']) {
    assert.equal((await f.window.fetch(route(path), {method: 'POST'})).status, 400);
  }
  assert.equal(f.calls.length, 3); assert.ok(f.probe.records.every(row => row.kind === 'forbidden'));
});
await test('concurrent body observations reserve distinct original response IDs', async () => {
  const f = fixture();
  const first = f.window.fetch(route('plans/preview'), {method:'POST',body:'{"first":true}'});
  const second = f.window.fetch(route('plans/preview'), {method:'POST',body:'{"second":true}'});
  await Promise.all([first, second]);
  assert.deepEqual(f.probe.records.map(record => record.id), [0, 1]);
  assert.deepEqual(f.probe.records.map(record => record.body), [{first:true},{second:true}]);
  assert.equal(f.calls.length, 2);
});
await test('duplicate probe installation leaves the current owner unchanged', async () => {
  const f = fixture(); const owner = f.window.fetch;
  assert.throws(() => install(f.window, Request, URL, location), /already installed/); assert.equal(f.window.fetch, owner);
});
console.log(`PASS ${checks} foreign review delivery host cases; no browser or import execution performed`);

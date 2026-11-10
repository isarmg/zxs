// Production account handlers, with transport/DOM doubles and no host commands.
import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import vm from "node:vm";
const source = readFileSync("helper/web/service_account.js", "utf8");
const html = readFileSync("helper/web/index.html", "utf8");
const key = "zxs-helper:local-service-account", digest = "a".repeat(64);
const input = {service_user:"zxs", service_group:"zxs"};
const sambaKey = "zxs-helper:local-samba-unix-account", sambaDigest = "b".repeat(64);
const sambaInput = {service_user:"zxs-smb", service_group:"zxs-smb"};
function harness(saved = null, sambaSaved = null) {
  const elements = new Map([...html.matchAll(/id="([^"]+)"/g)].map(([, id]) => [id, {
    value:"", disabled:false, checked:false, handlers:new Map(),
    addEventListener(type, fn) { this.handlers.set(type, fn); },
  }]));
  const element = id => { assert.ok(elements.has(id), id); return elements.get(id); };
  element("service-account-user").value = "zxs";
  element("service-account-group").value = "zxs";
  element("samba-unix-account-user").value = "zxs-smb";
  element("samba-unix-account-group").value = "zxs-smb";
  const storage = new Map(), calls = [], errors = [], confirmations = [], listeners = new Map();
  if (saved !== null) storage.set(key, saved);
  if (sambaSaved !== null) storage.set(sambaKey, sambaSaved);
  let respond = () => { throw new Error("unexpected request"); };
  const context = vm.createContext({
    byId:element, busy:false, stopping:false, csrf:"token", TextEncoder,
    document:{addEventListener(name, fn) {
      const handlers = listeners.get(name) ?? []; handlers.push(fn); listeners.set(name, handlers);
    }},
    window:{addEventListener() {}, confirm(text) { confirmations.push(text); return true; }},
    sessionStorage:{getItem:name => storage.get(name) ?? null, setItem(name, value) { storage.set(name, value); }},
    error(text) { errors.push(text); },
    setBusy(value) {
      context.busy = value;
      for (const fn of listeners.get("zxs-helper-busy-changed") ?? []) fn();
    },
    async post(path, body) {
      const request = JSON.parse(JSON.stringify(body)); calls.push({path, request});
      return {json:async () => await respond(path, request)};
    },
  });
  vm.runInContext(source, context);
  return {element, storage, calls, errors, confirmations, context,
    answer(fn) { respond = fn; },
    async emit(name, type = "click", prefix = "service-account") {
      await element(prefix + "-" + name).handlers.get(type)({preventDefault() {}});
    },
  };
}
const missing = {plan:{input}, request_digest:digest, requires_creation:true};
const flow = harness();
assert.equal(flow.calls.length, 0);
flow.answer(() => missing);
await flow.emit("preview");
assert.equal(flow.element("service-account-prepare").disabled, false);
await flow.emit("prepare");
assert.equal(flow.calls.length, 1); // Acknowledgment is separate from preview.
flow.element("service-account-ack").checked = true;
flow.answer(() => { throw new Error("lost response"); });
await flow.emit("prepare");
assert.equal(flow.calls.length, 2);
assert.equal(flow.calls[1].path, "/api/initialization/account-prepare");
assert.equal(flow.calls[1].request.confirmation, "prepare-local-service-account");
assert.equal(flow.calls[1].request.external_administration_stopped, true);
assert.deepEqual(JSON.parse(flow.storage.get(key)), {input, request_digest:digest});
assert.match(flow.errors.at(-1), /lost response/);
await flow.emit("prepare");
assert.equal(flow.calls.length, 2); // Unknown outcome never automatically replays.

const restored = harness(flow.storage.get(key));
assert.equal(restored.calls.length, 0);
assert.equal(restored.element("service-account-user").disabled, true);
restored.answer(() => ({...missing, requires_creation:false}));
await restored.emit("preview");
restored.element("service-account-ack").checked = true;
restored.answer(() => ({state:"local_service_identity_prepared", request_digest:digest}));
await restored.emit("prepare");
assert.equal(restored.calls.length, 2);
assert.equal(restored.element("service-account-ack").checked, false);
assert.match(restored.confirmations[0], /只核对原有 UID\/GID/);

const changed = harness();
changed.answer(() => missing);
await changed.emit("preview");
changed.element("service-account-user").value = "other";
await changed.emit("user", "input");
await changed.emit("prepare");
assert.equal(changed.calls.length, 1);
const stale = harness();
stale.answer(() => missing);
await stale.emit("preview");
stale.element("service-account-ack").checked = true;
stale.storage.set(key, "changed outside this handler");
await stale.emit("prepare");
assert.equal(stale.calls.length, 1);
assert.equal(stale.storage.get(key), "changed outside this handler");

const mismatch = harness(JSON.stringify({input, request_digest:digest}));
mismatch.answer(() => ({...missing, request_digest:"b".repeat(64)}));
await mismatch.emit("preview");
assert.equal(mismatch.element("service-account-prepare").disabled, true);
assert.match(mismatch.errors.at(-1), /预览与保留的原请求不同/);
console.log("service-account explicit preparation and recovery handlers passed");

// A completed service-account record must not block the distinct Samba identity.
const both = harness(restored.storage.get(key));
const serviceOriginal = both.storage.get(key);
const sambaMissing = {plan:{input:sambaInput}, request_digest:sambaDigest, requires_creation:true};
assert.equal(both.element("service-account-user").disabled, true);
assert.equal(both.element("samba-unix-account-user").disabled, false);
both.answer(() => sambaMissing);
await both.emit("preview", "click", "samba-unix-account");
both.element("samba-unix-account-ack").checked = true;
both.answer((path, request) => {
  assert.equal(both.element("service-account-preview").disabled, true);
  assert.equal(both.element("samba-unix-account-preview").disabled, true);
  assert.equal(path, "/api/initialization/account-prepare");
  assert.deepEqual(request.input, sambaInput);
  assert.equal(both.storage.get(key), serviceOriginal);
  assert.deepEqual(JSON.parse(both.storage.get(sambaKey)), {input:sambaInput, request_digest:sambaDigest});
  throw new Error("Samba identity reply lost");
});
await both.emit("prepare", "click", "samba-unix-account");
assert.equal(both.calls.length, 2);
assert.match(both.errors.at(-1), /Samba identity reply lost/);
assert.match(both.confirmations[0], /独立 Samba Unix 账户/);
assert.equal(both.storage.get(key), serviceOriginal);
await both.emit("prepare", "click", "samba-unix-account");
assert.equal(both.calls.length, 2);
const bothRestored = harness(both.storage.get(key), both.storage.get(sambaKey));
assert.equal(bothRestored.calls.length, 0);
for (const prefix of ["service-account", "samba-unix-account"]) {
  assert.equal(bothRestored.element(prefix + "-user").disabled, true);
}
bothRestored.answer(() => ({...sambaMissing, requires_creation:false}));
await bothRestored.emit("preview", "click", "samba-unix-account");
bothRestored.element("samba-unix-account-ack").checked = true;
bothRestored.answer(() => ({state:"local_service_identity_prepared", request_digest:sambaDigest}));
await bothRestored.emit("prepare", "click", "samba-unix-account");
assert.equal(bothRestored.storage.get(key), serviceOriginal);
assert.equal(bothRestored.element("samba-unix-account-ack").checked, false);
assert.deepEqual(bothRestored.calls.map(call => call.request.input ?? call.request), [sambaInput, sambaInput]);
console.log("independent Samba Unix identity uses shared account handlers without overwriting service identity");

// Production account handlers, with transport/DOM doubles and no host commands.
import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import vm from "node:vm";
const source = readFileSync("helper/web/service_account.js", "utf8");
const html = readFileSync("helper/web/index.html", "utf8");
const key = "zxs-helper:local-service-account", digest = "a".repeat(64);
const input = {service_user:"zxs", service_group:"zxs"};
function harness(saved = null) {
  const elements = new Map([...html.matchAll(/id="([^"]+)"/g)].map(([, id]) => [id, {
    value:"", disabled:false, checked:false, handlers:new Map(),
    addEventListener(type, fn) { this.handlers.set(type, fn); },
  }]));
  const element = id => { assert.ok(elements.has(id), id); return elements.get(id); };
  element("service-account-user").value = "zxs";
  element("service-account-group").value = "zxs";
  const storage = new Map(), calls = [], errors = [], confirmations = [], listeners = new Map();
  if (saved !== null) storage.set(key, saved);
  let respond = () => { throw new Error("unexpected request"); };
  const context = vm.createContext({
    byId:element, busy:false, stopping:false, csrf:"token", TextEncoder,
    document:{addEventListener(name, fn) { listeners.set(name, fn); }},
    window:{addEventListener() {}, confirm(text) { confirmations.push(text); return true; }},
    sessionStorage:{getItem:name => storage.get(name) ?? null, setItem(name, value) { storage.set(name, value); }},
    error(text) { errors.push(text); },
    setBusy(value) { context.busy = value; listeners.get("zxs-helper-busy-changed")?.(); },
    async post(path, body) {
      const request = JSON.parse(JSON.stringify(body)); calls.push({path, request});
      return {json:async () => await respond(path, request)};
    },
  });
  vm.runInContext(source, context);
  return {element, storage, calls, errors, confirmations, context,
    answer(fn) { respond = fn; },
    async emit(name, type = "click") { await element("service-account-" + name).handlers.get(type)({preventDefault() {}}); },
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

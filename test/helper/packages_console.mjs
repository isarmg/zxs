// Actual package card handlers, never invoking host APT/systemd or account tools.
import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import vm from "node:vm";
const source = readFileSync("helper/web/packages.js", "utf8");
const html = readFileSync("helper/web/index.html", "utf8");
const key = "zxs-helper:system-packages", digest = "a".repeat(64);
const planned = {state:"installable", request_digest:digest, plan:{simulation:"Inst samba (target Debian [amd64])\n"}};
function harness(saved = null) {
  const elements = new Map([...html.matchAll(/id="([^"]+)"/g)].map(([, id]) => [id, {
    checked:false, disabled:false, textContent:"", handlers:new Map(),
    addEventListener(name, fn) { this.handlers.set(name, fn); },
  }]));
  const byId = id => { assert.ok(elements.has(id), id); return elements.get(id); };
  const storage = new Map(), calls = [], errors = [], confirmations = [], listeners = new Map();
  if (saved !== null) storage.set(key, saved);
  let answer = () => { throw new Error("unexpected request"); };
  const context = vm.createContext({
    byId, busy:false, stopping:false, csrf:"token",
    sessionStorage:{getItem:k => storage.get(k) ?? null, setItem(k, v) { storage.set(k, v); }},
    document:{addEventListener(name, fn) { listeners.set(name, fn); }},
    window:{confirm(text) { confirmations.push(text); return true; }, addEventListener() {}},
    error(text) { errors.push(text); },
    setBusy(value) { context.busy = value; listeners.get("zxs-helper-busy-changed")?.(); },
    async post(path, body) {
      const request = JSON.parse(JSON.stringify(body)); calls.push({path, request});
      return {json:async () => await answer(path, request)};
    },
  });
  vm.runInContext(source, context);
  return {calls, errors, confirmations, storage, context,
    element:name => byId("packages-" + name), answer(fn) { answer = fn; },
    async click(name) { await byId("packages-" + name).handlers.get("click")(); },
  };
}
const flow = harness();
assert.equal(flow.calls.length, 0);
await flow.click("refresh");
assert.equal(flow.calls.length, 0);
flow.element("ack").checked = true;
flow.answer(() => "configured_apt_indexes_refreshed");
await flow.click("refresh");
assert.equal(flow.calls[0].request.confirmation, "refresh-system-package-indexes");
assert.equal(flow.storage.has(key), false);
flow.answer(() => planned);
await flow.click("preview");
assert.equal(flow.element("install").disabled, false);
flow.element("ack").checked = true;
flow.answer((path, request) => {
  assert.equal(path, "/api/initialization/packages-install");
  assert.equal(request.request_digest, digest);
  assert.equal(flow.storage.get(key), digest);
  assert.equal(flow.element("preview").disabled, true);
  throw new Error("connection lost");
});
await flow.click("install");
assert.match(flow.errors.at(-1), /connection lost/);
assert.match(flow.confirmations[1], /Inst samba/);
await flow.click("install");
assert.equal(flow.calls.length, 3); // Original request blocks any automatic/repeated APT call.
const restored = harness(flow.storage.get(key));
assert.equal(restored.calls.length, 0);
assert.equal(restored.element("refresh").disabled, true);
restored.answer(() => ({state:"original_pending", request_digest:digest, packages_installed:false}));
await restored.click("preview");
assert.equal(restored.element("finish").disabled, true);
restored.answer(() => ({state:"original_pending", request_digest:digest, packages_installed:true}));
await restored.click("preview");
assert.equal(restored.element("install").disabled, true);
assert.equal(restored.element("finish").disabled, false);
restored.element("ack").checked = true;
restored.answer(() => ({state:"completed", request_digest:digest, package_version:"target"}));
await restored.click("finish");
assert.equal(restored.calls.at(-1).path, "/api/initialization/packages-finish");
assert.equal(restored.calls.at(-1).request.confirmation, "finish-original-system-packages");
assert.equal(restored.storage.get(key), digest);
const discovered = harness();
discovered.answer(() => ({state:"original_pending", request_digest:digest, packages_installed:true}));
await discovered.click("preview");
assert.equal(discovered.element("refresh").disabled, true);
assert.equal(discovered.element("install").disabled, true);
for (const value of [
  {state:"unexpected"},
  {state:"installable", request_digest:digest, plan:{}},
  {state:"original_pending", request_digest:digest, packages_installed:"true"},
  {state:"completed", request_digest:"b".repeat(64)},
]) {
  const invalid = harness(digest); invalid.answer(() => value);
  await invalid.click("preview");
  assert.equal(invalid.element("install").disabled, true);
  assert.equal(invalid.element("finish").disabled, true);
}
const changed = harness(); changed.answer(() => planned);
await changed.click("preview"); changed.element("ack").checked = true;
changed.storage.set(key, "changed by another handler");
await changed.click("install");
assert.equal(changed.calls.length, 1);
assert.equal(changed.storage.get(key), "changed by another handler");
console.log("system-package explicit index/install/original-finish handlers passed");

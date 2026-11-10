// Actual activation handlers with DOM/transport doubles, not a systemd run.
import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import vm from "node:vm";
const html = readFileSync("helper/web/index.html", "utf8");
const consoleSource = readFileSync("helper/web/console.js", "utf8");
const source = consoleSource.slice(consoleSource.indexOf("function resetActivationPreview("),
  consoleSource.indexOf("function resetDeployment("));
const prefix = "zxs-helper:service-activation:", digest = "a".repeat(64);
const original = {input:{deployment:{service_user:"zxs", service_group:"zxs",
  listen:"0.0.0.0:7400", smb:null}, enable_on_boot:true}, request_digest:digest};
const accepted = () => ({state:"helper_exit_then_service_start", request_digest:digest,
  web_port:7400, helper_shutdown_requested:true, services_started:false, service_readiness_verified:false});
function harness(retained = null) {
  const elements = new Map([...html.matchAll(/id="([^"]+)"/g)].map(([, id]) => [id, {
    value:"", hidden:true, checked:false, disabled:false, handlers:new Map(),
    addEventListener(type, handler) { this.handlers.set(type, handler); },
    replaceChildren() {}, append() {}, reportValidity() { return true; },
  }]));
  const element = id => { assert.ok(elements.has(id), id); return elements.get(id); };
  for (const [id, value] of [["service-user", "zxs"], ["service-group", "zxs"], ["service-listen", "0.0.0.0:7400"]]) {
    element(id).value = value;
  }
  element("activation-enable-on-boot").checked = true;
  const storage = new Map(), calls = [], confirmations = [], errors = [];
  if (retained !== null) storage.set(prefix + digest, JSON.stringify(retained));
  let answer = () => { throw new Error("unexpected request"); };
  const context = vm.createContext({byId:element, busy:false, stopping:false, csrf:"csrf", URL,
    activationPreview:null, activationOriginal:null, activationObserved:false, activationPrefix:prefix,
    window:{location:{href:"http://host.example:7500/helper?view=activation#start"},
      confirm(text) { confirmations.push(text); return true; }},
    document:{createElement() { return {}; }},
    localStorage:{get length() { return storage.size; }, key(index) { return [...storage.keys()][index]; },
      getItem:key => storage.get(key) ?? null, setItem(key, value) { storage.set(key, value); }},
    error(text) { errors.push(text); },
    setBusy(value) { context.busy = value; },
    async post(path, body) {
      const request = JSON.parse(JSON.stringify(body)); calls.push({path, body:request});
      return {json:async () => answer(path, request)};
    },
  });
  vm.runInContext(source, context);
  return {element, storage, calls, confirmations, errors, context,
    respond(fn) { answer = fn; },
    async click(name) { await element("activation-" + name).handlers.get("click")(); },
  };
}
const flow = harness();
flow.respond((path, body) => {
  if (path.endsWith("activation-preview")) return {state:"offline_ready_for_handoff", request_digest:digest};
  assert.equal(path, "/api/deployment/activate");
  assert.deepEqual(JSON.parse(flow.storage.get(prefix + digest)), original);
  assert.deepEqual(body, {...original, confirmation:"exit-helper-and-start-installed-zxs-services",
    external_administration_stopped:true});
  return accepted();
});
await flow.click("preview");
flow.element("activation-external-stopped").checked = true;
await flow.click("start");
assert.equal(flow.context.stopping, true);
assert.equal(flow.element("activation-status").hidden, true);
assert.equal(flow.element("activation-retry").hidden, true);
assert.equal(flow.element("activation-open").hidden, false);
assert.equal(flow.element("activation-web").href, "http://host.example:7400/");
assert.match(flow.element("connection").textContent, /尚未确认服务可用/);
assert.match(flow.confirmations[0], /退出前.*开机启动/);
for (const action of ["preview", "start", "status", "retry"]) await flow.click(action);
assert.equal(flow.calls.length, 2);

const unknown = harness();
unknown.respond(path => {
  if (path.endsWith("activation-preview")) return {state:"offline_ready_for_handoff", request_digest:digest};
  throw new Error("response lost during exit");
});
await unknown.click("preview");
unknown.element("activation-external-stopped").checked = true;
await unknown.click("start");
assert.equal(unknown.context.stopping, false);
assert.deepEqual(JSON.parse(unknown.storage.get(prefix + digest)), original);
assert.equal(unknown.element("activation-retry").hidden, true);
assert.equal(unknown.element("activation-open").hidden, true);
assert.match(unknown.errors.at(-1), /再次使用离线 helper 前须停止 ZXS/);
await unknown.click("retry");
assert.equal(unknown.calls.length, 2);

for (const result of [{state:"services_active_and_management_ready", request_digest:digest},
  {state:"offline_ready_for_handoff", request_digest:"b".repeat(64)}, null]) {
  const restored = harness(original);
  assert.equal(restored.calls.length, 0);
  restored.respond(() => { if (result === null) throw new Error("ZXS is still running"); return result; });
  await restored.click("status");
  restored.element("activation-external-stopped").checked = true;
  await restored.click("retry");
  assert.equal(restored.calls.length, 1);
  assert.equal(restored.element("activation-open").hidden, true);
}
const restored = harness(original);
restored.respond((path, body) => {
  assert.equal(body.request_digest, digest);
  assert.deepEqual(body.input, original.input);
  return path.endsWith("activation-status") ? {state:"offline_ready_for_handoff", request_digest:digest} : accepted();
});
await restored.click("status");
assert.equal(restored.element("activation-retry").hidden, false);
restored.element("activation-external-stopped").checked = true;
await restored.click("retry");
assert.equal(restored.context.stopping, true);
assert.equal(restored.calls.length, 2);
console.log("activation console preserves the original, hands off after helper exit and never polls readiness");

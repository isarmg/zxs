// Actual first-install handlers with DOM/transport doubles, not browser QA.
import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import vm from "node:vm";

const key = "zxs-helper:first-install-tab";
const directoryKey = key + ":directories";
const digest = "a".repeat(64);
const html = readFileSync("helper/web/index.html", "utf8");
const source = readFileSync("helper/web/first_install.js", "utf8");
const initial = () => ({format:"zxs-helper-first-install-tab", revision:1, input:{
  deployment:{service_user:"zxs", service_group:"zxs", listen:"0.0.0.0:7400", smb:null},
  administrator:"admin", roots:["/zxs/data/disks/CON"], enable_on_boot:false,
}, steps:Array.from({length:6}, () => ({request_digest:digest, plan:null, issued:true, completed:true}))});

function harness(record = initial(), original = null) {
  const elements = new Map([...html.matchAll(/id="([^"]+)"/g)].map(([, id]) => [id, {
    value:"", hidden:true, checked:false, disabled:false, elements:[], textContent:"", handlers:new Map(),
    addEventListener(type, handler) { this.handlers.set(type, handler); },
    querySelectorAll() { return []; },
    dispatchEvent(event) { this.handlers.get(event.type)?.(event); },
    scrollIntoView() { this.scrolled = true; },
  }]));
  const element = id => { assert.ok(elements.has(id), `missing HTML id: ${id}`); return elements.get(id); };
  element("first-install-form").elements = [element("first-install-roots"), element("first-install-user")];
  const storage = new Map([[key, JSON.stringify(record)]]);
  if (original !== null) storage.set(directoryKey, JSON.stringify(original));
  const calls = [], confirmations = [], errors = [];
  const listeners = new Map();
  let answer = () => { throw new Error("unexpected transport call"); };
  const context = vm.createContext({
    byId:element, busy:false, stopping:false, csrf:"token", TextEncoder, structuredClone,
    Event:class { constructor(type) { this.type = type; } },
    window:{addEventListener() {}, confirm(text) { confirmations.push(text); return true; }},
    document:{addEventListener(type, listener) { listeners.set(type, listener); }},
    sessionStorage:{getItem:name => storage.get(name) ?? null, setItem(name, value) { storage.set(name, value); }},
    setBusy(value) { context.busy = value; listeners.get("zxs-helper-busy-changed")?.(); },
    error(text) { errors.push(text); },
    async post(path, body) {
      const request = JSON.parse(JSON.stringify(body));
      calls.push({path, body:request});
      return {json:async () => await answer(path, request)};
    },
  });
  vm.runInContext(source, context);
  return {element, storage, calls, confirmations, errors, context,
    record:() => JSON.parse(storage.get(key)),
    respond(fn) { answer = fn; },
    async click(name) { await element("first-install-" + name).handlers.get("click")({preventDefault() {}}); },
  };
}

// A rejected native preview must not strand already completed installation.
const flow = harness();
const original = flow.record();
assert.equal(flow.calls.length, 0);
assert.equal(flow.element("first-install-roots").disabled, false);
assert.equal(flow.element("first-install-user").disabled, true);
flow.respond(() => { throw new Error("invalid portable basename"); });
await flow.click("inspect");
assert.equal(flow.storage.has(directoryKey), false);
assert.match(flow.errors.at(-1), /invalid portable basename/);
flow.element("first-install-roots").value = "/zxs/data/disks/资料\n/zxs/data/disks/Archive";
const callsBeforeCorrection = flow.calls.length;
await flow.click("roots-correct");
assert.equal(flow.calls.length, callsBeforeCorrection);
assert.equal(flow.confirmations.length, 1);
assert.deepEqual(flow.record(), {...original, input:{...original.input,
  roots:["/zxs/data/disks/资料", "/zxs/data/disks/Archive"]}});

let prepared = false;
flow.respond((path, body) => {
  switch (path) {
    case "/api/initialization/storage-directories-preview":
      assert.deepEqual(body.roots, flow.record().input.roots);
      return {input:body, request_digest:digest,
        directories:body.roots.map(() => ({state:prepared ? "empty_prepared" : "missing"}))};
    case "/api/initialization/storage-directories-prepare":
      assert.deepEqual(body.input.roots, flow.record().input.roots);
      prepared = true;
      return {request_digest:digest, state:"original_empty_storage_directories_durably_prepared"};
    case "/api/initialization/storage-preview":
      return {request_digest:digest, plan:{roots:body.roots}};
    case "/api/initialization/storage-create":
      return {request_digest:digest, format:"zxs-helper-first-storage-batch-complete",
        storage_activation_completed:true};
    default: throw new Error(`unexpected endpoint: ${path}`);
  }
});
await flow.click("inspect");
assert.equal(flow.element("first-install-roots-correct").hidden, true);
assert.equal(flow.element("first-install-roots").disabled, true);
assert.equal(flow.element("first-install-commit").hidden, false);
const frozen = flow.storage.get(key);
flow.element("first-install-roots").value = "/zxs/data/disks/replacement";
await flow.click("roots-correct");
assert.equal(flow.storage.get(key), frozen);
assert.match(flow.errors.at(-1), /原请求已经保留/);
flow.element("first-install-ack").checked = true;
await flow.click("commit");
assert.equal(flow.record().steps.length, 7);
assert.deepEqual(flow.record().steps.slice(0, 6), original.steps);
assert.equal(flow.record().steps[6].issued, false);
flow.element("first-install-ack").checked = true;
await flow.click("commit");
assert.equal(flow.record().steps[6].completed, true);
assert.match(flow.element("first-install-progress").textContent, /8\/8/);

// Reload and imported storage plans never reopen directory selection.
const directoryOriginal = {input:{service_user:"zxs", roots:original.input.roots}, request_digest:digest};
const restored = harness(original, directoryOriginal);
assert.equal(restored.element("first-install-roots-correct").hidden, true);
assert.equal(restored.element("first-install-roots").disabled, true);
assert.equal(restored.calls.length, 0);
const selected = structuredClone(original);
selected.steps.push({request_digest:digest, plan:{roots:original.input.roots}, issued:false, completed:false});
const selectedFlow = harness(selected);
await selectedFlow.click("roots-correct");
assert.deepEqual(selectedFlow.record(), selected);
assert.equal(selectedFlow.confirmations.length, 0);

// Storage changes between rendering and clicking are not overwritten.
const stale = harness();
stale.storage.set(directoryKey, JSON.stringify(directoryOriginal));
stale.element("first-install-roots").value = "/zxs/data/disks/replacement";
await stale.click("roots-correct");
assert.deepEqual(stale.record(), original);
assert.equal(stale.confirmations.length, 0);
const changed = harness();
changed.storage.set(key, "externally changed");
await changed.click("roots-correct");
assert.equal(changed.storage.get(key), "externally changed");
assert.match(changed.errors.at(-1), /原请求已改变/);
assert.equal(changed.calls.length, 0);

// Earlier stages, busy work and an unavailable helper cannot change inputs.
const earlier = structuredClone(original);
earlier.steps.pop();
const earlierFlow = harness(earlier);
assert.equal(earlierFlow.element("first-install-roots-correct").hidden, true);
await earlierFlow.click("roots-correct");
assert.deepEqual(earlierFlow.record(), earlier);
for (const [field, value] of [["busy", true], ["stopping", true], ["csrf", ""]]) {
  const locked = harness();
  locked.context[field] = value;
  locked.element("first-install-roots").value = "/zxs/data/disks/replacement";
  await locked.click("roots-correct");
  assert.deepEqual(locked.record(), original);
  assert.equal(locked.confirmations.length, 0);
}
console.log("first-install directory correction handlers passed");

// The original first-install input selects two separate Unix identities.
for (const step of [2, 3, 4]) {
  const accountRecord = initial();
  accountRecord.steps.length = step;
  accountRecord.input.deployment.smb = {account:"zxs-smb", peer_uid:0};
  const account = harness(accountRecord);
  assert.equal(account.element("first-install-samba-account-settings").hidden, false);
  await account.click("samba-account-settings");
  assert.equal(account.element("samba-unix-account-user").value, "zxs-smb");
  assert.equal(account.element("samba-unix-account-group").value, "zxs-smb");
  assert.equal(account.element("samba-unix-account").scrolled, true);
  assert.equal(account.calls.length, 0);
  assert.deepEqual(account.record(), accountRecord);
}
const collisionRecord = initial();
collisionRecord.steps.length = 2;
collisionRecord.input.deployment.smb = {account:"zxs", peer_uid:0};
const collision = harness(collisionRecord);
await collision.click("samba-account-settings");
assert.match(collision.errors.at(-1), /必须独立/);
assert.equal(collision.calls.length, 0);
const lockedRecord = initial();
lockedRecord.steps.length = 2;
lockedRecord.input.deployment.smb = {account:"zxs-smb", peer_uid:0};
const lockedAccount = harness(lockedRecord);
lockedAccount.element("samba-unix-account-user").disabled = true;
lockedAccount.element("samba-unix-account-user").value = "retained-other";
await lockedAccount.click("samba-account-settings");
assert.equal(lockedAccount.element("samba-unix-account-user").value, "retained-other");
assert.match(lockedAccount.errors.at(-1), /另一原请求/);
assert.deepEqual(lockedAccount.record(), lockedRecord);
const disabledSamba = harness();
assert.equal(disabledSamba.element("first-install-samba-account-settings").hidden, true);
await disabledSamba.click("samba-account-settings");
assert.equal(disabledSamba.calls.length, 0);
console.log("first-install selects the independent Samba Unix account without sending or changing frozen input");

for (const samba of [false, true]) {
  const record = initial(); record.steps.length = 1;
  record.input.deployment.smb = samba ? {account:"zxs-smb", peer_uid:0} : null;
  const dependencies = harness(record);
  assert.equal(dependencies.element("first-install-package-settings").hidden, false);
  await dependencies.click("package-settings");
  assert.equal(dependencies.element("packages-scope").value, samba ? "samba" : "systemd");
  assert.equal(dependencies.element("packages").scrolled, true);
  assert.deepEqual(dependencies.record(), record);
  assert.equal(dependencies.calls.length, 0);
}
console.log("first-install selects only the dependencies requested by the frozen setup");

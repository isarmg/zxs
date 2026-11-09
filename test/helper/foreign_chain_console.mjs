// Actual helper handlers with bounded DOM/transport doubles, not browser QA.
import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import vm from "node:vm";

const html = readFileSync("helper/web/index.html", "utf8");
const elements = new Map([...html.matchAll(/id="([^"]+)"/g)].map(([, id]) => [id, {
  id, value:"", hidden:true, checked:false, disabled:false, elements:[], dataset:{}, textContent:"",
  handlers:new Map(), addEventListener(type, handler) {
    this.handlers.set(type, [...(this.handlers.get(type) || []), handler]);
  }
}]));
const element = id => { assert.ok(elements.has(id), `missing actual HTML id: ${id}`); return elements.get(id); };
const calls = [], downloads = [], storage = new Map();
let answer, storageFailure = false;
const operation = "op_11111111111111111111111111111111";
// A refresh restores only a selector, not approval or a queued write.
storage.set("zxs-helper:foreign-chain-original", JSON.stringify({service_user:"zxs",
  operation_id:operation,request_digest:"a".repeat(64),external_writers_stopped:true}));
const context = vm.createContext({
  document:{getElementById:element, body:{append() {}}, createElement:() => ({
    click() { downloads.push(this.download); }, remove() {}
  })},
  window:{addEventListener() {},confirm:() => true}, confirm:() => true,
  Blob, TextEncoder, URL:{createObjectURL:() => "blob:test",revokeObjectURL() {}},
  setTimeout:() => 0,
  localStorage:{getItem:key => storage.get(key) ?? null,setItem(key, value) {
    if (storageFailure) throw new Error("storage unavailable"); storage.set(key,value);
  }},
  async fetch(path, options) {
    if (path === "/api/status") return {ok:true,json:async () => ({csrf:"token",stopping:false,
      name:"helper",version:"test",busy:false,program_installation:{legal_materials:null,embedded:null}})};
    calls.push({path,body:JSON.parse(options.body)});
    return {ok:true,json:async () => await answer(path)};
  }
});
vm.runInContext(readFileSync("helper/web/console.js","utf8"),context);
await new Promise(resolve => setImmediate(resolve));
assert.equal(calls.length,0);
assert.equal(element("foreign-chain-operation").value,operation);
assert.equal(element("foreign-chain-writers").checked,false);
async function emit(id, type = "click") {
  for (const handler of element(id).handlers.get(type) || []) await handler({preventDefault() {}});
}
const original = {plan:{binding:{operation_id:operation}},request_digest:"a".repeat(64),batch_material_prepared:true};
element("foreign-chain-user").value = "zxs";
element("foreign-chain-operation").value = operation;
element("foreign-chain-digest").value = original.request_digest;
element("foreign-chain-writers").checked = true;
answer = async () => original;
await emit("foreign-chain-original");
assert.equal(element("foreign-chain-databases-preview").hidden,false);

const expected = {request_digest:original.request_digest,targets:[]};
const databases = complete => ({expected,database_digest:"b".repeat(64),original_database_scopes_prepared:complete});
answer = async () => databases(false);
await emit("foreign-chain-databases-preview");
assert.equal(element("foreign-chain-databases-form").hidden,false);
assert.equal(element("foreign-chain-identity-preview").hidden,true);

answer = async () => databases(true);
await emit("foreign-chain-databases-preview");
assert.equal(element("foreign-chain-databases-form").hidden,true);
assert.equal(element("foreign-chain-identity-preview").hidden,false);
let release;
answer = () => new Promise(resolve => { release = resolve; });
const before = calls.length;
const running = emit("foreign-chain-identity-preview");
await Promise.resolve();
assert.equal(element("foreign-chain-identity-preview").disabled,true);
await emit("foreign-chain-identity-preview");
assert.equal(calls.length,before + 1);
release({identity_digest:"c".repeat(64),target_identity_proposal_only:true,foreign_admitted:false});
await running;
await emit("foreign-chain-identity-save");
assert.deepEqual(downloads,[`zxs-foreign-identity-${operation}.json`]);

answer = async () => {throw new Error("database changed");};
await emit("foreign-chain-identity-preview");
assert.equal(element("foreign-chain-identity-save").hidden,true);
await emit("foreign-chain-identity-save");
assert.equal(downloads.length,1);
assert.match(element("error").textContent,/database changed/);

answer = async () => databases(false);
await emit("foreign-chain-databases-preview");
storageFailure = true;
const noRequest = calls.length;
await emit("foreign-chain-databases-form","submit");
assert.equal(calls.length,noRequest);
assert.match(element("error").textContent,/尚未发送/);
storageFailure = false;
answer = async () => {throw new Error("connection lost");};
await emit("foreign-chain-databases-form","submit");
assert.equal(calls.at(-1).body.confirmation,"prepare-original-three-scope-databases-for-entire-chain");
assert.equal(JSON.parse(storage.get("zxs-helper:foreign-chain-original")).operation_id,operation);
assert.match(element("error").textContent,new RegExp(operation));
assert.equal(element("foreign-chain-databases-form").hidden,true);
assert.equal(element("foreign-chain-identity-preview").hidden,true);

await emit("foreign-chain-user","input");
assert.equal(element("foreign-chain-identity-plan").hidden,true);
await emit("foreign-chain-identity-save");
assert.equal(downloads.length,1);
console.log("foreign helper handlers: complete gates, single request, stale proposal, retained retry and input reset passed");

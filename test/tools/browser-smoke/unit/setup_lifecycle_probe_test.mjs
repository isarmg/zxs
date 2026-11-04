// Host models exercise probe decisions, not product WASM or actual browser flows.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const rust=readFileSync(new URL('../runner/management.rs',import.meta.url),'utf8');
const script=name=>rust.split(`const ${name}: &str = r#"`)[1].split('"#;')[0];
const evaluate=(name,context)=>runInNewContext(`(function(){${script(name)}})()`,context);
function fixture(){
    const user={isConnected:true,disabled:false,readOnly:false,checkVisibility:()=>true},password={isConnected:true,disabled:false,readOnly:false,checkVisibility:()=>true,value:''},submit={isConnected:true,disabled:true,checkVisibility:()=>true},check={disabled:false};
    const warning={textContent:'Outcome unconfirmed. Check setup status.',checkVisibility:()=>true};
    const form={isConnected:true,textContent:'Browser smoke: initialize response lost after successful initialization',
        querySelector(selector){return ({'[data-setup-password]':password,'[data-setup-check]':check,
            '[data-setup-unknown]':warning,'[data-setup-disk], [data-setup-completion]':null})[selector];},
        querySelectorAll:selector=>selector==='button[type=submit]'?[submit]:[warning]};
    const login={isConnected:true,checkVisibility:()=>true,querySelector:selector=>({'[name=username]':user,'[name=password]':password,'button[type=submit]':submit})[selector]};
    const language={disabled:true};
    const state={form,setupPosts:['/api/setup/initialize'],statusReads:[],logins:[],dropped:true,
        initialization:{status:200,initialized:true,storagePending:true},submitStimuli:0,stimuliSettled:true};
    const nodes={'[data-first-account-form]':form,'form[data-auth-form="login"]':null,'[data-language-toggle]':language};
    const forms=[form];
    const context={window:{__zxsLostInitialize:state},document:{querySelector:selector=>nodes[selector]??null,querySelectorAll:()=>forms}};
    return {context,forms,form,login,language,warning,password,user,submit,check,state,nodes,
        run:()=>evaluate('LOST_INITIALIZE_STATE',context),
        recover(){nodes['[data-first-account-form]']=null;nodes['form[data-auth-form="login"]']=login;form.isConnected=false;forms.splice(0,forms.length,login);submit.disabled=false;language.disabled=false;}};
}
let count=0;
const check=(test)=>{test();count++;};
check(()=>{const r=fixture().run();for(const key of ['sameForm','unknown','transportError','keyAbsent','passwordCleared','settled','noInitialization','reloadBlocked'])assert.equal(r[key],true,key);assert.equal(r.complete,false);});
check(()=>{const f=fixture();f.submit.disabled=false;assert.equal(f.run().noInitialization,false,'An enabled setup button is not fenced');});
check(()=>{const f=fixture();f.warning.checkVisibility=()=>false;assert.equal(f.run().unknown,false,'Hidden unknown warning must fail');});
check(()=>{const f=fixture();f.submit.checkVisibility=()=>false;assert.equal(f.run().noInitialization,false,'Hidden disabled setup action must fail');});
check(()=>{const f=fixture();f.check.disabled=true;assert.equal(f.run().settled,false);});
check(()=>{const f=fixture();f.warning.textContent='An ordinary error';assert.equal(f.run().unknown,false);});
check(()=>{const f=fixture();f.password.value='synthetic-secret';assert.equal(f.run().passwordCleared,false);});
check(()=>{const f=fixture();f.nodes['[data-setup-key]']={};assert.equal(f.run().keyAbsent,false);});
check(()=>{const f=fixture();f.nodes['[data-first-account-form]']={...f.form};assert.equal(f.run().sameForm,false);});
check(()=>{const f=fixture();f.recover();const r=f.run();assert.equal(r.complete,true);assert.equal(r.settled,true);assert.equal(r.noInitialization,true);assert.equal(r.unknown,false);assert.equal(r.sameForm,false);assert.equal(r.reloadBlocked,false);});
check(()=>{const f=fixture();f.recover();f.form.isConnected=true;assert.equal(f.run().complete,false,'Connected stale setup cannot count as completed');});
check(()=>{const f=fixture();f.recover();f.nodes['[data-first-account-form]']=f.form;assert.equal(f.run().complete,false,'Any setup form blocks completion');});
check(()=>{const f=fixture();f.recover();f.nodes['form[data-auth-form="login"]']=null;assert.equal(f.run().complete,false);});
for(const part of ['user','password','submit'])check(()=>{const f=fixture();f.recover();f[part].disabled=true;assert.equal(f.run().complete,false,`Disabled ${part} blocks completed login`);});
check(()=>{const f=fixture();f.recover();f.forms.push({...f.login});assert.equal(f.run().complete,false,'Duplicate login forms cannot count as recovered');});
check(()=>{const f=fixture();f.recover();f.login.checkVisibility=()=>false;assert.equal(f.run().complete,false,'Hidden login cannot count as recovered');});
for(const part of ['user','password'])check(()=>{const f=fixture();f.recover();f[part].readOnly=true;assert.equal(f.run().complete,false,`Readonly ${part} blocks completed login`);});
for(const part of ['user','password','submit'])check(()=>{const f=fixture();f.recover();f[part].checkVisibility=()=>false;assert.equal(f.run().complete,false,`Hidden ${part} blocks completed login`);});
check(()=>{const f=fixture();f.recover();f.login.isConnected=false;assert.equal(f.run().complete,false);});
check(()=>{const f=fixture();delete f.context.window.__zxsLostInitialize;assert.throws(()=>f.run(),/missing/);});
const identity={value:'synthetic-admin'};
const response=(status,state)=>({status,clone:()=>({json:async()=>({state})})});
let queued=response(200,'empty'),calls=[];
const original=async(...args)=>{calls.push(args);return queued;};
const context={window:{fetch:original},document:{querySelector:()=>identity},location:{href:'http://synthetic.invalid/',origin:'http://synthetic.invalid'},URL};
evaluate('REAUTHORIZE_PROBE',context);const probe=context.window.__zxsSetupRecheck;
assert.equal(probe.identity,identity);count++;
assert.equal(await context.window.fetch('/api/setup/status'),queued);assert.equal(probe.reads.length,1);assert.equal(probe.reads[0].state,'empty');count++;
await context.window.fetch('http://other.invalid/api/setup/status');assert.equal(probe.reads.length,1);count++;
await context.window.fetch('/api/setup/initialize',{method:'POST'});assert.equal(probe.reads.length,1);count++;
queued=response(503,'unavailable');assert.equal(await context.window.fetch('/api/setup/status',{method:'GET'}),queued);assert.equal(probe.reads[1].status,503);count++;
assert.throws(()=>evaluate('REAUTHORIZE_PROBE',context),/already installed/);count++;
probe.restore();assert.equal(context.window.fetch,original);assert.equal(context.window.__zxsSetupRecheck,undefined);count++;
assert.match(rust,/s\.reads\.every\(r=>r\.status===200&&r\.state==='empty'\)/);assert.match(rust,/e === s\.identity/);count++;
assert.match(rust,/repeat_unknown_initialize\(browser,3\)/);assert.match(rust,/repeat_unknown_initialize\(browser,6\)/);count++;
assert.match(rust,/state\["setupPosts"\] != json!\(\["\/api\/setup\/initialize"\]\)/);count++;
assert.match(rust,/account result recheck replaced installed authority/);assert.match(rust,/original account recovery sent unexpected requests/);count++;
assert.doesNotMatch(rust,/click\(&browser\.css\("\[data-setup-confirm\]/);count++;
// Execute the exact readiness predicate embedded in the Rust driver.
const predicate=rust.match(/browser\.script\(&format!\("(const s=window\.__zxsSetupRecheck,[^"\n]+)"\)\)/)[1]
    .replace('{count}','2').replace('{expected}',JSON.stringify('synthetic-admin'));
function recheckFixture(){
    const form={},identity={value:'synthetic-admin',disabled:false},password={},check={disabled:false};
    const nodes={'[data-first-account-form]':form,'[data-setup-user]':identity,'[data-setup-password]':password,'[data-setup-check]':check};
    const state={form,identity,password,reads:[{status:200,state:'empty'},{status:200,state:'empty'}]};
    return {form,identity,password,check,nodes,state,run:()=>runInNewContext(`(function(){${predicate}})()`,{
        window:{__zxsSetupRecheck:state},document:{querySelector:selector=>nodes[selector]??null}})};
}
check(()=>assert.equal(recheckFixture().run(),true));
for(const selector of ['[data-first-account-form]','[data-setup-user]','[data-setup-password]'])check(()=>{
    const f=recheckFixture();f.nodes[selector]={...f.nodes[selector]};assert.equal(f.run(),false,`${selector} replacement cannot count as a stable recheck`);
});
for(const change of [f=>{f.state.reads.pop();},f=>{f.state.reads.push({status:200,state:'empty'});},
    f=>{f.state.reads[0].status=503;},f=>{f.state.reads[1].state='existing';},
    f=>{f.identity.value='changed';},f=>{f.identity.disabled=true;},f=>{f.check.disabled=true;},
    f=>{f.nodes['[data-setup-confirm]']={};}])check(()=>{const f=recheckFixture();change(f);assert.equal(f.run(),false);});
console.log(`PASS ${count} setup lifecycle probe models and wiring checks (not browser acceptance)`);

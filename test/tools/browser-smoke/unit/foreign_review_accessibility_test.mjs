import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
const validate = new Function(readFileSync(new URL('../runner/foreign_review/accessibility_model.js', import.meta.url), 'utf8'))();
const control = (name, disabled = false, tag = 'BUTTON') =>
  ({name, disabled, tag, checked: false, tabIndex: 0, brokenReferences: [], descriptions: []});
const description = (id, text, missing = null) => ({id, text, missing, local:true});
function recovery(name, disabled = false, tag = 'BUTTON') {
  return {...control(name, disabled, tag), descriptions:[
    description('zxs-foreign-plan-operation', 'Original review operation ID'),
    description('zxs-foreign-plan-scope', 'No target is reserved'),
  ]};
}
const draft = () => ({phase:'draft', wizards:1, name:'Foreign review', form:true, controls:[control('Target', false, 'INPUT')],
  modes:[{pressed:'true'},{pressed:'false'},{pressed:'false'}],
  expectedGroups:[{id:'group-a',history:'history-a',origin:'disk-a',missing:1}],
  groups:[{id:'group-a',text:'disk-a history-a',missing:'1',namedInput:true,descriptions:[
    description('zxs-foreign-plan-group-a-origin', 'disk-a'),
    description('zxs-foreign-plan-group-a-history', 'history-a'),
    description('zxs-foreign-plan-group-a-availability', 'Unavailable bodies: 1', '1'),
  ]}],
  review:false, execution:null, preview:control('Review'),
  planOnlyPhrase:'Save only the system-disk review record', noImportPhrase:'this is not an import completion receipt'});
let count = 0;
function pass(value) { assert.deepEqual(validate(value), []); count++; }
function fail(change) { const value=draft(); change(value); assert.notDeepEqual(validate(value), []); count++; }
pass(draft());
fail(s=>s.wizards=2);
fail(s=>s.name=' ');
fail(s=>s.controls[0].name='');
fail(s=>s.controls[0].brokenReferences=['missing-description']);
fail(s=>s.controls[0].tabIndex=1);
fail(s=>s.modes[1].pressed='true');
fail(s=>s.modes[1].pressed=null);
fail(s=>s.groups.push({...s.groups[0]}));
fail(s=>s.groups[0].text='history-a');
fail(s=>s.groups[0].missing='0');
fail(s=>s.groups[0].namedInput=false);
fail(s=>s.groups[0].descriptions=[]);
fail(s=>s.groups[0].descriptions[0].local=false);
fail(s=>s.groups[0].descriptions[0].text='disk-other');
fail(s=>s.groups[0].descriptions[1].text='history-other');
fail(s=>s.groups[0].descriptions[2].missing='0');
const review = {...draft(),phase:'review',review:true,execution:control('Unavailable',true),
  confirm:recovery('Save only the system-disk review record',false,'INPUT'),save:recovery('Save review',true)};
pass(review);
for (const change of [s=>s.execution.disabled=false,s=>s.execution.tag='DIV',s=>s.confirm.checked=true,
  s=>s.confirm.name='Import everything',s=>s.save.disabled=false]) {
  const value=structuredClone(review); change(value); assert.notDeepEqual(validate(value),[]);count++;
}
const unknown = {...structuredClone(review),phase:'unknown',confirm:null,save:null,
  alert:'Outcome unknown',retry:recovery('Retry original'),status:recovery('Query original'),preview:control('Review',true)};
unknown.controls.forEach(c=>c.disabled=true);
pass(unknown);
for (const change of [s=>s.alert='',s=>s.retry.disabled=true,s=>s.status.disabled=true,
  s=>s.controls[0].disabled=false,s=>s.preview.disabled=false]) {
  const value=structuredClone(unknown);change(value);assert.notDeepEqual(validate(value),[]);count++;
}
const stored = {...structuredClone(unknown),phase:'stored',savedStatus:'Saved; this is not an import completion receipt',
  revalidate:recovery('Revalidate')};
pass(stored);
for (const change of [s=>s.savedStatus='Import completed',s=>s.revalidate.disabled=true]) {
  const value=structuredClone(stored);change(value);assert.notDeepEqual(validate(value),[]);count++;
}
pass({...draft(),phase:'local',preview:control('Review',true)});
fail(s=>{s.phase='local';s.preview.disabled=false;});
// A preserved review after leaving the source wizard still exposes recovery.
pass({...structuredClone(unknown),form:false,groups:[],modes:[],controls:[]});
console.log(`PASS ${count} accessibility assertion-model cases; no real DOM, keyboard or screen-reader claim`);

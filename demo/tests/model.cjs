'use strict';
const {readFileSync}=require('node:fs');
const vm=require('node:vm');
const assert=require('node:assert/strict');
const {test}=require('node:test');
const html=readFileSync(require('node:path').join(__dirname,'../index.html'),'utf8');
const script=html.split('<script>')[1].split('</script>')[0];
new vm.Script(script);
const context=vm.createContext({TextEncoder,Date});
vm.runInContext(script.slice(0,script.indexOf('const esc ='))+';globalThis.Model=Simulation;',context);
const Model=context.Model;
function fixture(){const s=new Model(),f=Object.values(s.files)[0];return {s,f};}
function secondPrimary(s) {
  const disk = s.registerDisk('主盘 B');
  const dir = s.mkdir(disk.root, '目标目录');
  return {disk, dir};
}
function refusedUnchanged(s, action) {
  const before = JSON.stringify(s);
  assert.throws(action);
  assert.equal(JSON.stringify(s), before);
}
test('same bytes do not create nodes; A B A does; current name survives history',()=>{
  const {s,f}=fixture(),old=f.selected;s.saveFile(f.id,s.current(f));assert.equal(f.versions.length,1);
  s.saveFile(f.id,'B');s.saveFile(f.id,'实验记录：第一版。');assert.equal(f.versions.length,3);
  s.rename(f.id,'最新.txt');s.selectVersion(f.id,old);assert.equal(f.name,'最新.txt');
});
test('manual draft is shared, does not advance HEAD, and blocks implicit discard',()=>{
  const {s,f}=fixture(),old=f.selected;s.mode(f.id);s.saveFile(f.id,'draft');
  assert.equal(f.heads.main,old);assert.equal(f.versions.length,1);assert.equal(s.current(f),'draft');
  assert.throws(()=>s.mode(f.id));assert.throws(()=>s.selectVersion(f.id,old));
  const reboot=Object.assign(Object.create(Model.prototype),JSON.parse(JSON.stringify(s)));
  assert.equal(reboot.current(reboot.files[f.id]),'draft');s.commitFile(f.id);assert.equal(f.versions.length,2);
});
test('copy creates independent identity; same-disk rename move delete preserve identity',()=>{
  const {s,f}=fixture(),dest=Object.values(s.dirs).find(d=>d.name==='资料');
  const copy=s.copy(f.id,dest.id);assert.notEqual(copy.id,f.id);assert.equal(copy.versions.length,1);
  s.remove(f.id);assert.equal(f.trash.length,16);assert.equal(f.versions.length,1);
  assert.throws(()=>s.saveFile(f.id,'no'));s.move(f.id,f.dir);assert.equal(f.deleted,false);
});
test('same deleted path new file gets independent container',()=>{
  const {s,f}=fixture();s.remove(f.id);const n=s.add(f.dir,f.name,'new');assert.notEqual(n.id,f.id);
  assert.throws(()=>s.move(f.id,f.dir));
});
test('whole backup migration preserves old source until verified decision; retries do not duplicate',()=>{
  const {s,f}=fixture(),a=s.disks[f.disk],b=Object.values(s.disks).find(d=>d.level===2);
  const j=s.planBackup(a.id,b.id);s.stepJob(j.id);s.stepJob(j.id,true);assert.equal(b.replicas[a.id],undefined);
  s.stepJob(j.id);s.stepJob(j.id);assert.throws(()=>s.cancelJob(j.id));s.stepJob(j.id);s.stepJob(j.id);
  assert(b.replicas[a.id]);assert(s.files[f.id]);assert.equal(j.phase,'installed');
});
test('backup source drift and capacity exhaustion reject instead of silently truncating',()=>{
  const {s,f}=fixture(),b=Object.values(s.disks).find(d=>d.level===2),j=s.planBackup(f.disk,b.id);
  s.stepJob(j.id);s.stepJob(j.id);s.saveFile(f.id,'changed');assert.throws(()=>s.stepJob(j.id));
  b.capacity=1;assert.throws(()=>s.planBackup(f.disk,b.id));
});
test('topology level fence and unique disk naming include offline members',()=>{
  const {s}=fixture(),ds=Object.values(s.disks);assert.throws(()=>s.configureBackup(ds[2].id,[ds[0].id]));
  s.toggleDisk(ds[0].id);assert.throws(()=>s.registerDisk('磁盘 A'));s.renameDisk(ds[1].id,'备份改名');
  assert.equal(ds[1].uuid,ds[1].id.padStart(32,'0'));
});
test('UTF8 budget and reserved names fail without truncation',()=>{
  const {s,f}=fixture();assert.throws(()=>s.add(f.dir,'中'.repeat(80),'x'));
  assert.throws(()=>s.add(f.dir,'.zxs','x'));assert.throws(()=>s.add(f.dir,'CON.txt','x'));
});
test('only SMB exposes folder creation and its action updates the shared directory model',()=>{
  const {s}=fixture();
  const controls=[];
  const uiContext=vm.createContext({
    sim:s,ui:{page:'文件'},TextEncoder,
    button:(label,action)=>{controls.push({label,action});return label;},
    esc:String,row:(...parts)=>parts.join(''),box:(_title,body)=>body,
    input:()=>'',select:()=>'',dirOptions:()=>[],value:()=> 'SMB新目录',
    bytes:value=>new TextEncoder().encode(value).length,
  });
  vm.runInContext(script.slice(script.indexOf('function webFileTable()'),
    script.indexOf('function referenceDetails()'))+';webFileTable();',uiContext);
  assert(!controls.some(c=>c.label==='新建文件夹'));
  assert(controls.some(c=>c.label==='刷新文件夹'));
  controls.length=0;
  vm.runInContext(script.slice(script.indexOf('function smbActions()'),
    script.indexOf('function fileRow('))+';smbActions();',uiContext);
  const create=controls.find(c=>c.label==='创建普通目录');
  assert(create);const parent=s.currentDir;create.action();
  assert(Object.values(s.dirs).some(d=>d.name==='SMB新目录'&&d.parent===parent));
});
test('explicit SMB transfers preserve move identity and isolate copies and conflicts',()=>{
  const {s,f}=fixture(),target=s.mkdir(f.dir,'目标');
  s.transferSMB({kind:'file',id:f.id},{kind:'dir',id:target.id});
  assert.equal(s.files[f.id].dir,target.id);
  const copied=s.transferSMB({kind:'file',id:f.id},{kind:'dir',id:s.disks[f.disk].root},true);
  assert.notEqual(copied.id,f.id);
  assert.throws(()=>s.transferSMB({kind:'file',id:f.id},{kind:'dir',id:s.disks[f.disk].root},true));
  s.remove(f.id);s.transferSMB({kind:'file',id:f.id},{kind:'dir',id:target.id});assert.equal(f.deleted,false);
  const backup=Object.values(s.disks).find(d=>d.level===2);
  assert.throws(()=>s.transferSMB({kind:'file',id:f.id},{kind:'dir',id:backup.root}));
});
test('explicit cross-disk SMB move keeps the old FILE versions on A after disk renames', () => {
  const {s, f} = fixture();
  const b = secondPrimary(s), a = s.disks[f.disk];
  const history = JSON.stringify(f.versions);
  const oldName = a.name;
  s.renameDisk(a.id, '临时盘名');
  s.renameDisk(b.disk.id, oldName);
  s.renameDisk(a.id, '主盘 B');
  const moved = s.transferSMB({kind:'file', id:f.id}, {kind:'dir', id:b.dir.id});
  assert.notEqual(moved.id, f.id);
  assert.equal(moved.disk, b.disk.id);
  assert.equal(moved.deleted, false);
  assert.equal(moved.versions.length, 1);
  assert.equal(s.current(moved), s.current(f));
  assert.equal(f.disk, a.id);
  assert.equal(f.deleted, true);
  assert.equal(f.trash.length, 16);
  assert.equal(JSON.stringify(f.versions), history);
  s.saveFile(moved.id, 'only B changes');
  assert.equal(JSON.stringify(f.versions), history);
  assert.equal(s.current(f), '实验记录：第一版。');
});
test('explicit cross-disk copy never deletes or reuses the identical source', () => {
  const {s, f} = fixture();
  const b = secondPrimary(s);
  const twin = s.add(b.disk.root, f.name, s.current(f));
  const before = JSON.stringify(f);
  const copied = s.transferSMB({kind:'file', id:f.id}, {kind:'dir', id:b.dir.id}, true);
  assert.notEqual(copied.id, f.id);
  assert.notEqual(copied.id, twin.id);
  assert.equal(copied.disk, b.disk.id);
  assert.equal(JSON.stringify(f), before);
});
test('ordinary cross-disk moves copy the selected exposure without changing source draft or history', () => {
  for (const dirty of [false, true]) {
    const {s, f} = fixture();
    const b = secondPrimary(s), selected = f.selected;
    s.saveFile(f.id, 'later HEAD');
    s.selectVersion(f.id, selected);
    if (dirty) {
      s.mode(f.id);
      s.saveFile(f.id, 'current draft');
    }
    const exposed = s.current(f), draft = f.draft, auto = f.auto;
    const history = JSON.stringify({versions:f.versions, heads:f.heads, branch:f.branch});
    const moved = s.transferSMB({kind:'file', id:f.id}, {kind:'dir', id:b.dir.id});
    assert.equal(s.current(moved), exposed);
    assert.equal(moved.versions.length, 1);
    assert.equal(f.selected, selected);
    assert.equal(f.draft, draft);
    assert.equal(f.auto, auto);
    assert.equal(f.deleted, true);
    assert.equal(JSON.stringify({versions:f.versions, heads:f.heads, branch:f.branch}), history);
  }
});
test('same-disk ordinary move preserves the shared draft and selected identity', () => {
  const {s, f} = fixture();
  const target = s.mkdir(s.disks[f.disk].root, '同盘目标');
  s.mode(f.id);
  s.saveFile(f.id, 'shared draft');
  const selected = f.selected, history = JSON.stringify(f.versions);
  const moved = s.transferSMB({kind:'file', id:f.id}, {kind:'dir', id:target.id});
  assert.equal(moved, f);
  assert.equal(f.dir, target.id);
  assert.equal(f.selected, selected);
  assert.equal(f.draft, 'shared draft');
  assert.equal(JSON.stringify(f.versions), history);
});
test('cross-disk transfer conflicts, offline disks, and unknown explicit IDs leave state intact', () => {
  const {s, f} = fixture();
  const b = secondPrimary(s);
  s.add(b.dir.id, f.name, 'occupied');
  refusedUnchanged(s, () => s.transferSMB({kind:'file', id:f.id}, {kind:'dir', id:b.dir.id}));
  refusedUnchanged(s, () => s.transferSMB({kind:'file', id:'unknown-file'}, {kind:'dir', id:b.disk.root}));
  s.toggleDisk(b.disk.id);
  refusedUnchanged(s, () => s.transferSMB({kind:'file', id:f.id}, {kind:'dir', id:b.disk.root}));
  assert.equal(f.deleted, false);
});
test('SMB keyboard shortcuts dispatch only within the file browser and outside editors',()=>{
  let handler;const calls=[];
  const c=vm.createContext({document:{addEventListener:(_event,fn)=>{handler=fn;}},
    ui:{},act:fn=>fn(),smbClipboard:copy=>calls.push(copy?'copy':'cut'),
    smbPaste:()=>calls.push('paste'),smbEdit:mode=>calls.push(mode),smbDelete:()=>calls.push('delete')});
  const begin=script.indexOf("document.addEventListener('keydown',event=>{");
  vm.runInContext(script.slice(begin,script.indexOf('\nfunction render()',begin)),c);
  const key=(key,extra={})=>handler({key,preventDefault(){},target:{closest:()=>true,matches:()=>false},...extra});
  key('c',{ctrlKey:true});key('x',{metaKey:true});key('v',{ctrlKey:true});key('F2');key('Delete');
  assert.deepEqual(calls,['copy','cut','paste','rename','delete']);
  key('c',{ctrlKey:true,target:{closest:()=>false}});
  key('c',{ctrlKey:true,target:{closest:()=>true,matches:()=>true}});
  assert.equal(calls.length,5);
});

test('opaque project.zxs is one FILE with old bytes intact across rename and recursive backup', () => {
  const {s} = fixture(), archive = Object.values(s.files).find(file=>file.name==='project.zxs');
  const a=s.disks[archive.disk], b=Object.values(s.disks).find(d=>d.level===2),
    c=Object.values(s.disks).find(d=>d.level===3);
  const first=archive.selected, original=s.current(archive);
  const folders=JSON.stringify(s.dirs), identities=Object.keys(s.files);
  s.rename(archive.id,'renamed.zxs');
  s.saveFile(archive.id,'PK\u0003\u0004literal text, not a ZIP directory');
  assert.deepEqual(Object.keys(s.files),identities);
  assert.equal(JSON.stringify(s.dirs),folders);
  assert.equal(archive.versions[0].creationName,'project.zxs');
  assert.equal(archive.versions[1].creationName,'renamed.zxs');
  assert.equal(archive.versions[0].id,first);
  assert.equal(archive.versions[0].body,original);
  for (const [source,target] of [[a,b],[b,c]]) {
    s.configureBackup(target.id,[source.id]);
    const job=s.planBackup(source.id,target.id);
    for (let n=0;n<4;n++) s.stepJob(job.id);
    assert.equal(job.phase,'installed');
  }
  const atB=b.replicas[a.id].own, atC=c.replicas[b.id].inherited[a.id].own;
  assert.deepEqual(Object.keys(atC).sort(),['dirs','files','units']);
  for (const stored of [atB,atC]) {
    assert.deepEqual(Object.keys(stored.files),identities);
    assert.deepEqual(stored.files[archive.id].versions.map(v=>v.body),
                     archive.versions.map(v=>v.body));
    assert.equal(stored.units.filter(unit=>unit.id===archive.id&&unit.kind==='container').length,1);
  }
  s.saveFile(archive.id,'new source body after both copies');
  assert.equal(atC.files[archive.id].versions.length,2);
});

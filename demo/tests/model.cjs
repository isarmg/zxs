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
function fixture(){const s=new Model(),f=Object.values(s.files)[0],r=Object.values(s.repos)[0];return {s,f,r};}
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
test('copy creates independent identity; rename move delete preserve identity',()=>{
  const {s,f}=fixture(),dest=Object.values(s.dirs).find(d=>d.name==='资料');
  const copy=s.copy(f.id,dest.id);assert.notEqual(copy.id,f.id);assert.equal(copy.versions.length,1);
  s.remove(f.id);assert.equal(f.trash.length,16);assert.equal(f.versions.length,1);
  assert.throws(()=>s.saveFile(f.id,'no'));s.move(f.id,f.dir);assert.equal(f.deleted,false);
});
test('same deleted path new file gets independent container',()=>{
  const {s,f}=fixture();s.remove(f.id);const n=s.add(f.dir,f.name,'new');assert.notEqual(n.id,f.id);
  assert.throws(()=>s.move(f.id,f.dir));
});
test('initial worktree is sparse and concurrent commits fork at current HEAD',()=>{
  const {s,r}=fixture(),source=r.heads.main,a=s.createWT(source),b=s.createWT(source);
  assert.equal(Object.keys(a.edits).length,0);assert.notEqual(a.name,b.name);
  s.editWT(a.id,'报告.txt','A');s.editWT(b.id,'报告.txt','B');
  const first=s.commitWT(a.id),second=s.commitWT(b.id);
  assert.equal(first.branch,'main');assert.notEqual(second.branch,'main');
  assert.equal(s.nodes[source].files['报告.txt'].body,'实验记录：第一版。');
  assert.equal(Object.keys(a.edits).length,0);assert.equal(s.view(a)['报告.txt'].body,'A');
});
test('deleted worktree can release visible name; recovery reallocates without overwrite',()=>{
  const {s,r,f}=fixture(),source=r.heads.main,a=s.createWT(source);
  s.editWT(a.id,'报告.txt','A');s.deleteWT(a.id);const count=f.versions.length;
  const b=s.createWT(source);assert.equal(a.name,b.name);s.restoreWT(a.id);assert.notEqual(a.name,b.name);
  s.deleteWT(a.id);assert.equal(f.versions.length,count);assert.equal(s.view(b)['报告.txt'].body,'实验记录：第一版。');
});
test('worktree unknown imports immediately create independent deleted containers',()=>{
  const {s,r}=fixture(),a=s.createWT(r.heads.main),b=s.createWT(r.heads.main);
  s.importWT(a.id,null,false,'same.txt');s.importWT(b.id,null,false,'same.txt');
  const x=a.imports['same.txt'].fileId,y=b.imports['same.txt'].fileId;
  assert.notEqual(x,y);assert(s.files[x].deleted);assert(s.files[y].deleted);
});
test('explicit external move retains identity; offline is not physical absence',()=>{
  const {s,r,f}=fixture();s.rename(f.id,'moved.txt');const w=s.createWT(r.heads.main);
  s.importWT(w.id,f.id,true);assert.equal(w.imports['moved.txt'].fileId,f.id);assert(f.deleted);
  s.editWT(w.id,'moved.txt','edit');s.toggleDisk(f.disk);assert.throws(()=>s.commitWT(w.id));
  s.toggleDisk(f.disk);s.purge(f.id);s.commitWT(w.id);assert.equal(s.files[f.id],undefined);
  assert.equal(s.view(w)['moved.txt'].body,'edit');
});
test('worktree rename/delete/empty dirs are isolated, file rename is not a content node',()=>{
  const {s,r,f}=fixture(),w=s.createWT(r.heads.main);s.mkdirWT(w.id,'folder');
  s.renameWT(w.id,'报告.txt','folder/new.txt');s.removeWTFile(w.id,'计划.md');
  const n=s.commitWT(w.id);assert(n.files['folder/new.txt']);assert.equal(n.files['计划.md'],undefined);
  assert.equal(f.versions.length,1);assert.equal(f.name,'报告.txt');assert(n.dirs.includes('folder'));
});
test('directory revoke preserves visible files and permits independent reinitialization',()=>{
  const {s,r,f}=fixture();s.revokeRepo(r.id);assert(s.files[f.id]);
  const fresh=s.initRepo(r.dir);assert.notEqual(fresh.id,r.id);assert.throws(()=>s.restoreRepo(r.id,r.dir));
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

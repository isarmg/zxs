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
function secondPrimary(s) {
  const disk = s.registerDisk('主盘 B');
  const dir = s.mkdir(disk.root, '目标仓库');
  const repo = s.initRepo(dir.id);
  const node = s.snapshot(repo.id);
  return {disk, dir, repo, node};
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
test('SMB worktree drop uses explicit container identity and rejects unknown sources',()=>{
  const {s,f,r}=fixture(),w=s.createWT(r.heads[r.branch]);s.rename(f.id,'新导入.txt');
  s.transferSMB({kind:'file',id:f.id},{kind:'wt',id:w.id});
  assert.equal(s.view(w)['新导入.txt'].fileId,f.id);assert.equal(f.deleted,true);
  assert.throws(()=>s.transferSMB({kind:'unknown',id:f.id},{kind:'wt',id:w.id}));
  assert.throws(()=>s.transferSMB({kind:'file',id:f.id},{kind:'wt',id:w.id}));
});
test('same-disk common and deleted containers remain valid worktree references', () => {
  const {s, f, r} = fixture();
  const source = r.heads.main;
  const w = s.createWT(source);
  assert.equal(r.disk, f.disk);
  assert.equal(w.disk, f.disk);
  assert.equal(s.view(w)['报告.txt'].disk, f.disk);
  s.editWT(w.id, '报告.txt', 'common edit');
  s.commitWT(w.id);
  assert.equal(s.current(f), 'common edit');
  s.remove(f.id);
  s.renameDisk(f.disk, '同盘改名');
  s.editWT(w.id, '报告.txt', 'deleted edit');
  const node = s.commitWT(w.id);
  assert.equal(node.files['报告.txt'].fileId, f.id);
  assert.equal(node.files['报告.txt'].disk, f.disk);
  assert.equal(s.current(f), 'deleted edit');
  assert.equal(f.deleted, true);
  assert.equal(s.nodes[source].files['报告.txt'].body, '实验记录：第一版。');
});
test('explicit cross-disk SMB move creates B identity and retains A historical reference', () => {
  const {s, f, r} = fixture();
  const b = secondPrimary(s), a = s.disks[f.disk];
  const source = r.heads.main, history = JSON.stringify(s.nodes[source]);
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
  assert.equal(JSON.stringify(s.nodes[source]), history);
  assert.equal(s.nodes[source].files['报告.txt'].fileId, f.id);
  assert.equal(s.nodes[source].files['报告.txt'].disk, a.id);
  const destination = s.snapshot(b.repo.id);
  assert.equal(destination.files['报告.txt'].fileId, moved.id);
  assert.equal(destination.files['报告.txt'].disk, b.disk.id);
  const oldWorktree = s.createWT(source);
  s.editWT(oldWorktree.id, '报告.txt', 'source history edit');
  s.commitWT(oldWorktree.id);
  assert.equal(s.current(f), 'source history edit');
  assert.equal(s.current(moved), '实验记录：第一版。');
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
test('cross-disk move into worktree maps B deleted container and leaves A history on A', () => {
  const {s, f, r} = fixture();
  const b = secondPrimary(s), w = s.createWT(b.node.id);
  const oldNode = r.heads.main, oldVersions = JSON.stringify(f.versions);
  const moved = s.transferSMB({kind:'file', id:f.id}, {kind:'wt', id:w.id});
  assert.notEqual(moved.id, f.id);
  assert.equal(moved.disk, b.disk.id);
  assert.equal(moved.deleted, true);
  assert.equal(moved.trash.length, 16);
  assert.equal(f.disk, r.disk);
  assert.equal(f.deleted, true);
  assert.equal(s.view(w)[f.name].fileId, moved.id);
  assert.equal(s.view(w)[f.name].disk, b.disk.id);
  assert.equal(s.nodes[oldNode].files[f.name].fileId, f.id);
  assert.equal(s.nodes[oldNode].files[f.name].disk, r.disk);
  s.editWT(w.id, f.name, 'B only');
  const committed = s.commitWT(w.id);
  assert.equal(committed.files[f.name].fileId, moved.id);
  assert.equal(committed.files[f.name].disk, b.disk.id);
  assert.equal(s.current(moved), 'B only');
  assert.equal(JSON.stringify(f.versions), oldVersions);
});
test('cross-disk worktree copies are separate destination containers with unchanged source', () => {
  const {s, f} = fixture();
  const b = secondPrimary(s), first = s.createWT(b.node.id), second = s.createWT(b.node.id);
  const before = JSON.stringify(f);
  const one = s.transferSMB({kind:'file', id:f.id}, {kind:'wt', id:first.id}, true);
  const two = s.transferSMB({kind:'file', id:f.id}, {kind:'wt', id:second.id}, true);
  assert.notEqual(one.id, two.id);
  assert.notEqual(one.id, f.id);
  assert.equal(one.disk, b.disk.id);
  assert.equal(two.disk, b.disk.id);
  assert(one.deleted && two.deleted);
  assert.equal(JSON.stringify(f), before);
});
test('cross-disk worktree export moves only the occurrence, not its original container', () => {
  const {s, f, r} = fixture();
  const b = secondPrimary(s), source = r.heads.main, w = s.createWT(source);
  const before = JSON.stringify(f);
  const copied = s.transferSMB({kind:'wt-file', id:w.id, path:f.name},
    {kind:'dir', id:b.disk.root}, true);
  assert.equal(f.deleted, false);
  assert(s.view(w)[f.name]);
  const moved = s.transferSMB({kind:'wt-file', id:w.id, path:f.name},
    {kind:'dir', id:b.dir.id});
  assert.notEqual(moved.id, f.id);
  assert.notEqual(moved.id, copied.id);
  assert.equal(moved.disk, b.disk.id);
  assert.equal(f.disk, r.disk);
  assert.equal(JSON.stringify(f), before);
  assert.equal(s.view(w)[f.name], undefined);
  assert.equal(s.nodes[source].files[f.name].fileId, f.id);
  assert.equal(s.nodes[source].files[f.name].disk, r.disk);
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
test('snapshot and worktree creation refuse foreign references before allocating any IDs', () => {
  const {s, r} = fixture();
  const b = secondPrimary(s);
  const laterFile = Object.values(s.files)[1];
  laterFile.disk = b.disk.id;
  refusedUnchanged(s, () => s.snapshot(r.id));
  refusedUnchanged(s, () => s.createWT(r.heads.main));
  laterFile.disk = r.disk;
  const entry = s.nodes[r.heads.main].files[laterFile.name];
  entry.disk = b.disk.id;
  delete s.files[entry.fileId];
  refusedUnchanged(s, () => s.createWT(r.heads.main));
});
test('worktree rejects foreign source metadata before publishing, deleting, or importing', () => {
  const {s, f, r} = fixture();
  const b = secondPrimary(s), w = s.createWT(r.heads.main);
  s.editWT(w.id, f.name, 'pending edit');
  const foreign = s.add(b.dir.id, f.name, s.current(f));
  w.imports['foreign.txt'] = {fileId:foreign.id, disk:foreign.disk,
    version:foreign.selected, branch:foreign.branch, body:s.current(f), storage:'foreign'};
  refusedUnchanged(s, () => s.commitWT(w.id));
  refusedUnchanged(s, () => s.deleteWT(w.id));
  refusedUnchanged(s, () => s.importWT(w.id, null, false, 'new.txt'));
  refusedUnchanged(s, () => s.exportWT(w.id, f.name, b.disk.root, true));
  w.imports['foreign.txt'].disk = r.disk;
  refusedUnchanged(s, () => s.commitWT(w.id));
  refusedUnchanged(s, () => s.deleteWT(w.id));
});
test('repository restoration is same-disk and retains stable ownership after renames', () => {
  const {s, r} = fixture();
  const b = secondPrimary(s), local = s.mkdir(s.disks[r.disk].root, '恢复位置');
  s.revokeRepo(r.id);
  refusedUnchanged(s, () => s.restoreRepo(r.id, b.disk.root));
  s.renameDisk(r.disk, '原盘新名字');
  s.restoreRepo(r.id, local.id);
  assert.equal(r.disk, local.disk);
  assert.equal(r.dir, local.id);
  assert.equal(s.createWT(r.heads.main).disk, local.disk);
});
test('cross-disk transfer conflicts, offline disks, and unknown explicit IDs leave state intact', () => {
  const {s, f} = fixture();
  const b = secondPrimary(s), w = s.createWT(b.node.id);
  s.add(b.dir.id, f.name, 'occupied');
  refusedUnchanged(s, () => s.transferSMB({kind:'file', id:f.id}, {kind:'dir', id:b.dir.id}));
  s.transferSMB({kind:'file', id:f.id}, {kind:'wt', id:w.id}, true);
  refusedUnchanged(s, () => s.transferSMB({kind:'file', id:f.id}, {kind:'wt', id:w.id}));
  refusedUnchanged(s, () => s.importWT(w.id, 'unknown-file', false, 'new.txt'));
  refusedUnchanged(s, () => s.importWT(w.id, null, true, 'new.txt'));
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

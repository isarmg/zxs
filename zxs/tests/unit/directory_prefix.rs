use super::*;
use crate::{ordinary::directory::types::Basis,pool::lease::{Access,LocalPool},test_support::PrivateTemp};
use std::path::Path;
fn id()->OperationId{OperationId::new().unwrap()}
fn obj(d:&HeldDir)->NativeObject{NativeObject{entity:d.entity,birth:fs::Birth::read(&d.file).unwrap()}}
fn origin(parent:&Basis,name:&str)->DirOrigin{DirOrigin{op:id(),uid:fs::local_uid(),parent:parent.clone(),name:name.into(),
    entry:EntryId::new().unwrap(),id:DirectoryId::new().unwrap(),ticket:id()}}
fn decide(j:&Journal,pool:PoolId,o:DirOrigin,child:&HeldDir)->Decision{
    let(n,prev)=j.frontier();Decision::Directory(DirCreated::new(pool,n+1,prev,o,[9;32],0,obj(child)).unwrap())
}
fn advance_fixture(j:&mut Journal,i:&mut OriginalIndex,reg:&Allowed<'_>,d:Decision){
    i.admit(&d,&j.decisions,j.frame_bytes,j.frontier().1,reg).unwrap();
    i.remember(&d,j.decisions.len(),reg).unwrap();j.frame_bytes+=d.bytes().len();j.decisions.push(d);
}
#[test]
#[ignore="genuine FILE history and native private objects; directory frames remain in memory only"]
fn previous_original_directories_are_typed_scoped_and_cannot_take_a_real_file_entry_or_native_birth(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let a=t.child("one");let reg=crate::initialize(id(),&sys,&r.child("recycle"),std::slice::from_ref(&a)).unwrap();
    let v=reg.volumes.iter().find(|v|v.volume.path==a).unwrap().id;
    let file=crate::create_empty(&sys,id(),v,"report").unwrap();
    let(p,_,mut j)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let original=j.frontier();let volume=p.ordinary(v).unwrap();
    let auth=p.journal_registration();let root=Basis::registered(&volume).unwrap();
    let h=volume.root.new_child_directory(c"outer").unwrap();let child=h.new_child_directory(c"report").unwrap();
    let f=volume.root.child_directory(c"report").unwrap().unwrap();
    let mut idx=OriginalIndex::default();idx.ops.insert(auth.basis().1);
    for(n,d)in j.decisions.iter().enumerate(){idx.remember(d,n,&auth).unwrap();}
    let rejects=|o|idx.admit(&decide(&j,p.id(),o,&h),&j.decisions,j.frame_bytes,original.1,&auth).is_err();
    assert!(rejects(origin(&root,"report")));assert!(rejects(DirOrigin{entry:file.entry,..origin(&root,"different")}));
    assert!(rejects(DirOrigin{ticket:file.created_by,..origin(&root,"different")}));
    assert!(rejects(DirOrigin{op:j.file(file.file).unwrap().create.attempt,..origin(&root,"different")}));
    assert!(idx.admit(&decide(&j,p.id(),origin(&root,"different"),&f),&j.decisions,j.frame_bytes,original.1,&auth).is_err());
    let top=decide(&j,p.id(),origin(&root,"outer"),&h);let top=top.directory().unwrap().clone();
    let bottom=Basis{address:DirParent::Child(v,top.origin.id),anchor:top.digest,object:top.object.clone()};
    assert!(idx.new_directory(&origin(&bottom,"report"),&j.decisions,&auth).is_err());
    advance_fixture(&mut j,&mut idx,&auth,Decision::Directory(top.clone()));
    let another=decide(&j,p.id(),origin(&bottom,"report"),&child);let another=another.directory().unwrap().clone();
    advance_fixture(&mut j,&mut idx,&auth,Decision::Directory(another.clone()));
    j.entries=idx.entries;j.names=idx.names;j.directory_rows=idx.directory_rows;j.directory_tokens=idx.directory_tokens;
    assert!(j.occupied_root(v,"outer"));assert_eq!(j.on(v).map(|d|d.create.file).collect::<Vec<_>>(),[file.file]);
    assert!(j.ticket_used(another.origin.ticket));
    for o in [&top.origin,&another.origin]{assert!(j.entry_used(o.entry));
        assert!(j.require_file_token(o.ticket).is_err());assert!(j.require_file_token(o.op).is_err());}
    assert!(j.require_file_token(id()).is_ok());
    assert!(j.new_directory_origin(&DirOrigin{op:another.origin.ticket,..origin(&bottom,"new")},&auth).is_err());
    assert!(j.new_directory_origin(&DirOrigin{ticket:file.created_by,..origin(&bottom,"new")},&auth).is_err());
    assert!(j.new_directory_origin(&DirOrigin{entry:file.entry,..origin(&bottom,"new")},&auth).is_err());
    assert!(j.new_directory_origin(&origin(&bottom,"new"),&auth).is_ok());
    assert_eq!(j.directory_lineage(DirParent::Child(v,another.origin.id)).unwrap(),[&top,&another]);
    assert!(j.directory_lineage(DirParent::Child(v,DirectoryId(file.file.0))).is_err());
    assert_eq!(p.snapshot().unwrap().1.frontier(),original); // never installed or published by the fixture prefix
}

#[test]
#[ignore="two registered native filesystems; exact directory budget at root and beneath a confirmed parent"]
fn directory_originals_budget_the_registered_absolute_names_and_every_issued_slot_before_any_new_operand(){
    use std::os::unix::{ffi::OsStrExt,fs::DirBuilderExt};
    use crate::ordinary::material;
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let short=t.child("normal");let ticket=id();
    // Derive the width from the actual mkdir spelling. Do not hard-code the
    // FILE spelling or assume it stays equal to the directory ticket.
    let widest=(0..allocation::SLOTS).map(|slot|material::staging(ticket,slot).to_bytes().len()).max().unwrap();
    let limit=4096-1-widest;
    let yes=crate::test_support::native_root_length(&t.child("yes"),limit);
    let no=crate::test_support::native_root_length(&t.child("no"),limit+1);
    let mut deep=t.child("deep");let mut count=deep.components().filter(|c|matches!(c,std::path::Component::Normal(_))).count();
    while count<fs::MAX_PATH_DEPTH-1{deep.push("p");std::fs::DirBuilder::new().mode(0o700).create(&deep).unwrap();count+=1;}
    let reg=crate::initialize(id(),&sys,&r.child("recycle"),&[short.clone(),yes.clone(),no.clone(),deep.clone()]).unwrap();
    let volume=|path:&Path|reg.volumes.iter().find(|r|r.volume.path==path).unwrap().id;
    {let(p,_,j)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let allowed=p.journal_registration();
        for (path,accept) in [(&no,false),(&yes,true),(&deep,true)]{
            let root=p.ordinary(volume(path)).unwrap();let basis=Basis::registered(&root).unwrap();
            let mut op=origin(&basis,"x");op.ticket=ticket;
            assert!(root.root.checked_child_path(c"x").is_ok());
            let slot=material::staging(ticket,0);
            if path==&no{assert_eq!(path.as_os_str().as_bytes().len()+1+slot.to_bytes().len(),4097);}
            if path==&yes{assert_eq!(root.root.checked_child_path(&slot).unwrap().as_os_str().as_bytes().len(),4096);}
            assert_eq!(material::check_ticket_paths(&root.root,"x",ticket).is_ok(),accept);
            assert_eq!(j.new_directory_origin(&op,&allowed).is_ok(),accept);
            let mut idx=OriginalIndex::default();idx.ops.insert(allowed.basis().1);
            assert_eq!(idx.new_directory(&op,&[],&allowed).is_ok(),accept);
            if accept{
                // Only simulate the ORIGINAL PREFIX, not an installed mkdir.
                // Native control object supplies child birth for the relation;
                // the actual new child location is still vacant.
                let control=HeldDir::open(&t.child(if path==&deep{"control-depth"}else{"control-bytes"})).unwrap();
                let first=decide(&j,p.id(),op,&control);
                idx.admit(&first,&[],0,j.frontier().1,&allowed).unwrap();idx.remember(&first,0,&allowed).unwrap();
                let d=first.directory().unwrap();let child=Basis{address:DirParent::Child(root.id(),d.origin.id),anchor:d.digest,object:d.object.clone()};
                let second=origin(&child,"y");let ds=[first];assert!(idx.new_directory(&second,&ds,&allowed).is_err());
                // At exact-depth root the first child is 256; the next is 257
                // despite a short name. At exact-byte root its private names,
                // not its human child or depth, exceed the registered limit.
            }
            assert!(root.root.names().unwrap().is_empty());
        }
        let(_,now)=p.snapshot().unwrap();assert_eq!(now.frontier(),j.frontier());assert!(now.allocated.is_none()&&now.pending.is_none());
        assert!(!p.system.entry_present(allocation::NAME).unwrap()&&!p.system.entry_present(INTENT).unwrap());
    }
    let peer=crate::create_empty(&sys,id(),volume(&short),"sibling").unwrap();
    assert_eq!(crate::list_initial_files(&sys,volume(&short)).unwrap().files[0].file,peer.file);
}

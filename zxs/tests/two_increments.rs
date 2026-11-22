//! Real offline storage fixtures, not an invocation of the delivered server.
//! The independent OPEN/replay boundaries are the library calls themselves.
use std::{collections::BTreeSet,fs,os::unix::fs::MetadataExt,path::Path};
use storage_server::{DirectoryEntry,DirectoryParent as Parent,DraftClaim,Entity,FileId,FormalNode,FormalStorage,LengthPolicy,
    MoveStatus,NodeId,OperationId,RecycleAction as Action,RecycleFilter,RestoreTarget,Role,SpanEdit};
#[path="unit/support.rs"]mod support;
const F0:&[u8]=b"The quick brown fox jumps over the lazy dog";
const F0_SHA:&str="d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592";
const HELLO_SHA:&str="2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
const ABC_SHA:&str="ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
fn op()->OperationId{OperationId::new().unwrap()}
fn claim(n:u64,sha:&str)->DraftClaim{DraftClaim::from_hex(n,sha).unwrap()}
fn edit(offset:u64,n:u64,sha:&str,length:u64)->SpanEdit{
    SpanEdit::new(offset,claim(n,sha),LengthPolicy::Set(length)).unwrap()
}
fn node(sys:&Path,file:FileId,id:NodeId)->Vec<u8>{
    let mut out=Vec::new();storage_server::read_node_to(sys,file,id,&mut out).unwrap();out
}
fn current(sys:&Path,file:FileId)->Vec<u8>{
    let mut out=Vec::new();storage_server::read_current_to(sys,file,&mut out).unwrap();out
}
fn range(sys:&Path,file:FileId,offset:u64,len:u64)->Vec<u8>{
    let mut out=Vec::new();storage_server::read_current_range_to(sys,file,offset,len,&mut out).unwrap();out
}
fn node_range(sys:&Path,file:FileId,id:NodeId,offset:u64,len:u64)->Vec<u8>{
    let mut out=Vec::new();storage_server::read_node_range_to(sys,file,id,offset,len,&mut out).unwrap();out
}
fn incremental(got:&FormalNode,parent:&FormalNode,n:u64,sha:&str){
    assert_eq!(got.kind,FormalStorage::Incremental{parent:parent.node,parent_version:parent.version,changed:claim(n,sha)});
}
fn denied_or_absent<T>(r:std::io::Result<Option<T>>){assert!(!matches!(r,Ok(Some(_))));}

#[test]
#[ignore="private registered FILE A, FILE B, and genuinely separate dedicated recycle filesystems"]
fn two_editors_one_file_three_fixed_histories_and_original_replays_after_custody_round_trip(){
    let a=support::PrivateTemp::under(&std::env::temp_dir());
    let b=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_MOVE_FILE_TMPDIR").unwrap()));
    let rr=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=a.child("system");let old=a.child("A");let new=b.child("B");let bin=rr.child("R");
    assert_ne!(fs::metadata(&old).unwrap().dev(),fs::metadata(&new).unwrap().dev());
    storage_server::initialize(op(),&sys,&bin,&[old.clone(),new.clone()]).unwrap();
    let pool=storage_server::inspect(&sys).unwrap();let registered=|path:&Path|pool.volumes.iter()
        .find(|v|v.volume.role==Role::File&&v.volume.path==path).unwrap().id;
    let from=registered(&old);let to=registered(&new);
    let birth=storage_server::create_empty(&sys,op(),from,"shared").unwrap();let id=birth.file;
    let first_save=op();let first_body=claim(43,F0_SHA);let w0=storage_server::save_first(&sys,first_save,id,first_body,F0).unwrap();
    let c0=op();let f0=storage_server::commit_working(&sys,c0,id,1).unwrap();
    let save_a=op();let ea=edit(4,5,HELLO_SHA,12);let wa=storage_server::save_interval(&sys,save_a,id,1,ea,&b"hello"[..]).unwrap();
    assert_eq!(wa.revision,2);
    let a_bytes=b"The hello br";assert_eq!(current(&sys,id),a_bytes);
    let c1=op();let i1=storage_server::commit_working(&sys,c1,id,2).unwrap();incremental(&i1,&f0,5,HELLO_SHA);

    // A→B froze exactly F0 + I1. I2 is B's independent later pack.
    let mv=op();let moved=storage_server::move_file(&sys,mv,id,from,to,0).unwrap();
    assert_eq!(moved.status,MoveStatus::CompleteCheckedNow);
    fs::rename(&old,a.0.join("unavailable-A")).unwrap();
    let save_b=op();let eb=edit(6,3,ABC_SHA,20);let wb=storage_server::save_interval(&sys,save_b,id,2,eb,&b"abc"[..]).unwrap();
    assert_eq!(wb.base_node,i1.node);
    let mut b_bytes=b"The heabc br".to_vec();b_bytes.resize(20,0);
    assert_eq!(current(&sys,id),b_bytes);
    let c2=op();let i2=storage_server::commit_working(&sys,c2,id,3).unwrap();
    assert_ne!(i2.node,i1.node);assert_ne!(i2.node,f0.node);incremental(&i2,&i1,3,ABC_SHA);
    let checked=||{
        assert_eq!(node(&sys,id,f0.node),F0);
        assert_eq!(node(&sys,id,i1.node),a_bytes);
        assert_eq!(node(&sys,id,i2.node),b_bytes);
        assert_eq!(current(&sys,id),b_bytes);
        assert_eq!(node_range(&sys,id,i2.node,4,5),b"heabc");
        assert_eq!(range(&sys,id,12,8),[0;8]);
        assert_eq!(node_range(&sys,id,f0.node,12,8),&F0[12..20]);
        let mut bytes=Vec::new();
        assert!(storage_server::read_node_range_to(&sys,id,i1.node,12,1,&mut bytes).is_err()&&bytes.is_empty());
        let actual=storage_server::current_info(&sys,id).unwrap().file;
        assert_eq!((actual.file,actual.entry,actual.history),(id,birth.entry,birth.history));
    };
    checked();
    let delete=op();let deleting=Action::Delete{file:id,from:to};
    let deleted=storage_server::apply_recycle(&sys,delete,1,deleting).unwrap();
    assert_eq!(deleted.status,MoveStatus::CompleteCheckedNow);let key=deleted.deletion.unwrap();
    for n in [f0.node,i1.node,i2.node]{
        let mut bytes=Vec::new();assert!(storage_server::read_node_to(&sys,id,n,&mut bytes).is_err()&&bytes.is_empty());
    }
    let listed=storage_server::list_retained(&sys,RecycleFilter::default(),8,None).unwrap();
    assert!(listed.entries.iter().any(|e|e.original.file==id&&e.original.deletion==Some(key)));
    let rest=op();let restoring=Action::Restore{deleted:key,target:RestoreTarget::Volume(to)};
    let restored=storage_server::apply_recycle(&sys,rest,2,restoring).unwrap();
    assert_eq!((restored.status,restored.deletion),(MoveStatus::CompleteCheckedNow,Some(key)));checked();
    assert_eq!(fs::read_dir(new.join(support::physical("shared",&id.to_string()))).unwrap().count(),5);
    // Completed requests do not need the old source, current FILE or R.
    fs::rename(&new,b.0.join("unavailable-B")).unwrap();fs::rename(&bin,rr.0.join("unavailable-R")).unwrap();
    assert_eq!(storage_server::save_first(&sys,first_save,id,first_body,&[][..]).unwrap(),w0);
    assert_eq!(storage_server::save_interval(&sys,save_a,id,1,ea,&[][..]).unwrap(),wa);
    assert_eq!(storage_server::save_interval(&sys,save_b,id,2,eb,&[][..]).unwrap(),wb);
    for(issued,rev,original)in[(c0,1,&f0),(c1,2,&i1),(c2,3,&i2)]{
        assert_eq!(&storage_server::commit_working(&sys,issued,id,rev).unwrap(),original);
    }
    assert_eq!(storage_server::move_file(&sys,mv,id,from,to,0).unwrap().status,MoveStatus::OriginalPublished);
    assert_eq!(storage_server::apply_recycle(&sys,delete,1,deleting).unwrap().deletion,Some(key));
    assert_eq!(storage_server::apply_recycle(&sys,rest,2,restoring).unwrap().status,MoveStatus::OriginalPublished);
    assert!(storage_server::save_interval(&sys,save_a,id,2,ea,&[][..]).is_err());
    let mut wrong=key;wrong.decision=[0;32];
    assert!(storage_server::recycle_receipt(&sys,rest,2,Action::Restore{deleted:wrong,target:RestoreTarget::Volume(to)}).is_err());
}

#[test]
#[ignore="exact case-sensitive names; real private registered FILE and separate recycle filesystems required"]
fn exact_display_lookup_two_equal_spelling_histories_and_one_unchanged_name_per_container(){
    let h=support::PrivateTemp::under(&std::env::temp_dir());
    let rh=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=h.child("system");let a=h.child("A");let b=h.child("B");let c=h.child("C");let bin=rh.child("dedicated-R");
    let registry=storage_server::initialize(op(),&sys,&bin,&[a.clone(),b.clone(),c.clone()]).unwrap();
    let volume=|p:&Path|registry.volumes.iter().find(|r|r.volume.path==p).unwrap().id;
    let(va,vb,vc)=(volume(&a),volume(&b),volume(&c));
    let lookup=|v,name:&str|storage_server::lookup_file(&sys,v,name);
    let at=|v,name:&str,body:&[u8],sha:&str|{
        let f=storage_server::create_empty(&sys,op(),v,name).unwrap();
        assert_eq!(lookup(v,name).unwrap().unwrap().file,f.file);
        storage_server::save_first(&sys,op(),f.file,claim(body.len() as u64,sha),body).unwrap();
        let first=storage_server::commit_working(&sys,op(),f.file,1).unwrap();assert_eq!(node(&sys,f.file,first.node),body);
        (f.file,first.node)
    };
    let u=at(va,"Report.xlsx",b"abc",ABC_SHA);let l=at(va,"report.xlsx",b"hello",HELLO_SHA);
    let other=at(vb,"Report.xlsx",b"hello",HELLO_SHA);assert_ne!(u.0,l.0);assert_ne!(u.0,other.0);
    let upper=format!("Report-{}.xlsx",u.0);let lower=format!("report-{}.xlsx",l.0);let duplicate=format!("Report-{}.xlsx",other.0);
    let leaves=|p:&Path|fs::read_dir(p).unwrap().map(|e|e.unwrap().file_name().into_string().unwrap()).collect::<BTreeSet<_>>();
    assert_eq!(leaves(&a),[upper.clone(),lower.clone()].into_iter().collect());assert_eq!(leaves(&b),[duplicate.clone()].into_iter().collect());
    denied_or_absent(lookup(va,"REPORT.xlsx"));denied_or_absent(lookup(va,&upper)); // not a second route
    let files=storage_server::list_initial_files(&sys,va).unwrap();let mixed=storage_server::list_directory(&sys,Parent::Root(va)).unwrap();
    let names=BTreeSet::from(["Report.xlsx","report.xlsx"]);
    assert_eq!(files.files.iter().map(|f|f.name.as_str()).collect::<BTreeSet<_>>(),names);
    assert_eq!(mixed.entries.iter().map(DirectoryEntry::name).collect::<BTreeSet<_>>(),names);
    assert!(storage_server::make_directory(&sys,op(),Parent::Root(va),"Report.xlsx").is_err()); // logical collision
    assert!(storage_server::make_directory(&sys,op(),Parent::Root(va),&upper).is_err()); // physical collision
    storage_server::make_directory(&sys,op(),Parent::Root(vc),"Report.xlsx").unwrap();
    storage_server::make_directory(&sys,op(),Parent::Root(vb),&upper).unwrap(); // different original FILE
    let del=|file,from,epoch|{
        let row=storage_server::apply_recycle(&sys,op(),epoch,Action::Delete{file,from}).unwrap();
        assert_eq!(row.status,MoveStatus::CompleteCheckedNow);row.deletion.unwrap()
    };
    let restore=|issued,key,epoch,target|storage_server::apply_recycle(&sys,issued,epoch,Action::Restore{deleted:key,target});
    let du=del(u.0,va,0);let dl=del(l.0,va,0);let db=del(other.0,vb,0);
    assert_eq!(leaves(&bin),[upper.clone(),lower.clone(),duplicate.clone()].into_iter().collect());
    denied_or_absent(lookup(va,"Report.xlsx"));denied_or_absent(lookup(va,"report.xlsx"));
    let before=fs::read(sys.join("REGISTRATION")).unwrap();let items=leaves(&bin);
    for v in [vb,vc]{ // first conflicts physically, second logically
        let o=op();let target=RestoreTarget::Volume(v);
        assert!(restore(o,du,1,target).is_err());
        assert!(storage_server::recycle_receipt(&sys,o,1,Action::Restore{deleted:du,target}).unwrap().is_none());
        assert_eq!(fs::read(sys.join("REGISTRATION")).unwrap(),before);
        assert!(!sys.join("FILE-ALLOCATION").exists()&&!sys.join("FILE-INTENT").exists());assert_eq!(leaves(&bin),items);
    }
    // Reverse the three original deletes, not a name-derived identity.
    restore(op(),db,1,RestoreTarget::Original).unwrap();restore(op(),dl,1,RestoreTarget::Original).unwrap();
    let old_restore=op();restore(old_restore,du,1,RestoreTarget::Original).unwrap();assert!(leaves(&bin).is_empty());
    for (v,name,row,native,payload) in [(va,"Report.xlsx",u,&upper,&b"abc"[..]),(va,"report.xlsx",l,&lower,&b"hello"[..]),
        (vb,"Report.xlsx",other,&duplicate,&b"hello"[..])]{
        assert_eq!(lookup(v,name).unwrap().unwrap().file,row.0);
        assert_eq!(current(&sys,row.0),payload);assert_eq!(node(&sys,row.0,row.1),payload);
        let parent=if v==va{&a}else{&b};assert!(parent.join(native).join("FILE").is_file());
    }
    let again=del(u.0,va,2);assert_ne!(du,again);assert_eq!(leaves(&bin),[upper.clone()].into_iter().collect());
    assert_eq!(restore(old_restore,du,1,RestoreTarget::Original).unwrap().status,MoveStatus::OriginalPublished);
    assert!(restore(op(),du,3,RestoreTarget::Original).is_err());
    denied_or_absent(lookup(va,"Report.xlsx"));assert_eq!(leaves(&bin),[upper.clone()].into_iter().collect());
    restore(op(),again,3,RestoreTarget::Original).unwrap();assert_eq!(node(&sys,u.0,u.1),b"abc");
    // Exact Unicode display, with no case mapping or NFC/NFD lookup.
    let cn=at(va,"方案.xlsx",b"abc",ABC_SHA);assert!(a.join(format!("方案-{}.xlsx",cn.0)).is_dir());
    let composed=at(vc,"é.txt",b"hello",HELLO_SHA);
    denied_or_absent(lookup(vc,"e\u{301}.txt"));
    let decomposed=at(vc,"e\u{301}.txt",b"abc",ABC_SHA);assert_ne!(composed.0,decomposed.0);
    assert_eq!(lookup(vc,"é.txt").unwrap().unwrap().file,composed.0);
}

#[test]
#[ignore="real private ordinary roots on separate filesystems and an independent recycle filesystem"]
fn three_scoped_parents_keep_separate_file_identities_through_history_move_and_recycle(){
    let a=support::PrivateTemp::under(&std::env::temp_dir());
    let b=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_MOVE_FILE_TMPDIR").unwrap()));
    let rr=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=a.child("system");let old=a.child("A");let new=b.child("B");let bin=rr.child("R");
    assert_ne!(fs::metadata(&old).unwrap().dev(),fs::metadata(&new).unwrap().dev());
    storage_server::initialize(op(),&sys,&bin,&[old.clone(),new.clone()]).unwrap();
    let pool=storage_server::inspect(&sys).unwrap();let vol=|p:&Path|pool.volumes.iter().find(|v|v.volume.path==p).unwrap().id;
    let va=vol(&old);let vb=vol(&new);let name="方案.xlsx";
    let md=|p,n|{
        let v=match p{Parent::Root(v)|Parent::Child(v,_)=>v};
        Parent::Child(v,storage_server::make_directory(&sys,op(),p,n).unwrap().directory)
    };
    let al=md(Parent::Root(va),"left");let ar=md(Parent::Root(va),"right");
    let bl=md(Parent::Root(vb),"left");let br=md(Parent::Root(vb),"right");
    let get=|p|storage_server::lookup_file_at(&sys,p,name);
    let make=|p,create|storage_server::create_empty_at(&sys,create,p,name).unwrap();
    let id_op=op();let born=make(al,id_op);let file=born.file;
    let root_file=make(Parent::Root(va),op()).file;let sibling=make(ar,op()).file;let other=make(br,op()).file;
    assert_eq!(BTreeSet::from([file,root_file,sibling,other]).len(),4);
    assert_eq!(get(al).unwrap().unwrap().file,file);assert_eq!(get(ar).unwrap().unwrap().file,sibling);
    assert_eq!(storage_server::lookup_file(&sys,va,name).unwrap().unwrap().file,root_file);
    let root_list=storage_server::list_initial_files(&sys,va).unwrap();
    assert_eq!(root_list.files.iter().map(|f|f.file).collect::<Vec<_>>(),vec![root_file]);
    let child_list=storage_server::list_directory(&sys,al).unwrap();assert_eq!(child_list.entries,vec![DirectoryEntry::File(born.clone())]);
    assert!(storage_server::create_empty(&sys,id_op,va,name).is_err());assert!(storage_server::make_directory(&sys,op(),al,name).is_err());
    storage_server::save_first(&sys,op(),other,claim(5,HELLO_SHA),&b"hello"[..]).unwrap();
    let other_node=storage_server::commit_working(&sys,op(),other,1).unwrap().node;
    let sibling_path=new.join("right").join(format!("方案-{other}.xlsx"));
    let observed=(fs::metadata(&sibling_path).unwrap().dev(),fs::metadata(&sibling_path).unwrap().ino());
    storage_server::save_first(&sys,op(),file,claim(43,F0_SHA),F0).unwrap();
    let f0=storage_server::commit_working(&sys,op(),file,1).unwrap();
    storage_server::save_interval(&sys,op(),file,1,edit(4,5,HELLO_SHA,12),&b"hello"[..]).unwrap();
    let i1=storage_server::commit_working(&sys,op(),file,2).unwrap();incremental(&i1,&f0,5,HELLO_SHA);
    let mv=op();
    assert!(storage_server::move_file_at(&sys,mv,file,ar,bl,0).is_err());
    assert!(storage_server::move_file_at(&sys,mv,file,al,br,0).is_err());
    assert!(!sys.join("FILE-ALLOCATION").exists());
    let moved=storage_server::move_file_at(&sys,mv,file,al,bl,0).unwrap();
    assert_eq!((moved.status,moved.source_parent,moved.destination_parent),(MoveStatus::CompleteCheckedNow,Some(al),Some(bl)));
    denied_or_absent(get(al));
    let after=get(bl).unwrap().unwrap();assert_eq!((after.file,after.entry,after.history),(file,born.entry,born.history));
    // Reuse the old name for an unrelated FILE, then lose the old volume.
    let reuse=make(al,op()).file;assert_ne!(reuse,file);
    fs::rename(&old,a.0.join("unavailable-A")).unwrap();
    assert_eq!(storage_server::move_receipt_at(&sys,mv,file,al,bl,0).unwrap().unwrap().status,MoveStatus::OriginalPublished);
    assert!(storage_server::move_receipt_at(&sys,mv,file,ar,bl,0).is_err());
    assert!(storage_server::move_receipt(&sys,mv,file,va,vb,0).is_err());
    storage_server::save_interval(&sys,op(),file,2,edit(6,3,ABC_SHA,20),&b"abc"[..]).unwrap();
    let i2=storage_server::commit_working(&sys,op(),file,3).unwrap();incremental(&i2,&i1,3,ABC_SHA);
    let mut latest=b"The heabc br".to_vec();latest.resize(20,0);
    let verify=||{
        for(n,want)in[(f0.node,F0),(i1.node,&b"The hello br"[..]),(i2.node,latest.as_slice())]{
            assert_eq!(node(&sys,file,n),want);}
        assert_eq!(current(&sys,file),latest);assert_eq!(range(&sys,file,12,8),vec![0;8]);
    };
    verify();let native=format!("方案-{file}.xlsx");assert_eq!(new.join("left").join(&native).read_dir().unwrap().count(),5);
    let del=op();let deleting=Action::DeleteAt{file,source:bl};
    let receipt=storage_server::apply_recycle(&sys,del,1,deleting).unwrap();let key=receipt.deletion.unwrap();
    assert_eq!(receipt.deleted_from,bl);
    assert!(bin.join(&native).is_dir());denied_or_absent(get(bl));
    let mut bytes=Vec::new();assert!(storage_server::read_node_to(&sys,file,f0.node,&mut bytes).is_err()&&bytes.is_empty());
    let page=storage_server::list_retained(&sys,RecycleFilter{ordinary_source:Some(vb),..RecycleFilter::default()},16,None).unwrap();
    assert_eq!(page.entries.len(),1);assert_eq!((page.entries[0].original.file,page.entries[0].original.deleted_from),(file,bl));
    assert_eq!(storage_server::recycle_receipt(&sys,del,1,deleting).unwrap().unwrap().status,MoveStatus::OriginalPublished);
    assert!(storage_server::recycle_receipt(&sys,del,1,Action::Delete{file,from:vb}).is_err());
    let restore=op();let request=|deleted,target|Action::Restore{deleted,target};
    assert!(storage_server::apply_recycle(&sys,restore,2,request(key,RestoreTarget::Original)).is_err());
    assert!(storage_server::apply_recycle(&sys,restore,2,request(key,RestoreTarget::At(br))).is_err());
    // A directory with the exact display name blocks its original parent.
    let blocker=storage_server::make_directory(&sys,op(),bl,name).unwrap();let s_before=fs::read(sys.join("REGISTRATION")).unwrap();
    assert!(storage_server::apply_recycle(&sys,restore,2,request(key,RestoreTarget::OriginalParent)).is_err());
    assert!(!sys.join("FILE-ALLOCATION").exists()&&!sys.join("FILE-INTENT").exists());
    assert_eq!(fs::read(sys.join("REGISTRATION")).unwrap(),s_before);
    assert!(bin.join(&native).is_dir());
    assert!(storage_server::list_directory(&sys,bl).unwrap().entries.contains(&DirectoryEntry::Directory(blocker)));
    let sub=md(Parent::Root(vb),"recovered");let dest=request(key,RestoreTarget::At(sub));
    assert_eq!(storage_server::apply_recycle(&sys,restore,2,dest).unwrap().status,MoveStatus::CompleteCheckedNow);
    assert_eq!(get(sub).unwrap().unwrap().history,born.history);
    assert!(new.join("recovered").join(&native).is_dir());assert!(!bin.join(&native).exists());
    verify();assert_eq!(node(&sys,other,other_node),b"hello");
    assert_eq!((fs::metadata(&sibling_path).unwrap().dev(),fs::metadata(&sibling_path).unwrap().ino()),observed);
    let key2=storage_server::apply_recycle(&sys,op(),3,Action::DeleteAt{file,source:sub}).unwrap().deletion.unwrap();
    assert_ne!(key.decision,key2.decision);
    assert!(storage_server::apply_recycle(&sys,op(),4,request(key,RestoreTarget::OriginalParent)).is_err());
    assert_eq!(storage_server::apply_recycle(&sys,restore,2,dest).unwrap().status,MoveStatus::OriginalPublished);
    assert!(bin.join(&native).is_dir());
    let reopen=op();storage_server::apply_recycle(&sys,reopen,4,request(key2,RestoreTarget::OriginalParent)).unwrap();
    // Explicitly selecting the original parent is the very same request.
    assert_eq!(storage_server::recycle_receipt(&sys,reopen,4,request(key2,RestoreTarget::At(sub))).unwrap().unwrap().status,MoveStatus::OriginalPublished);
    assert!(storage_server::recycle_receipt(&sys,reopen,4,request(key2,RestoreTarget::At(bl))).is_err());
    verify();assert_eq!(make(al,id_op).file,file);
    fs::rename(a.0.join("unavailable-A"),&old).unwrap();assert_eq!(get(al).unwrap().unwrap().file,reuse);
    assert_eq!(get(ar).unwrap().unwrap().file,sibling);
}

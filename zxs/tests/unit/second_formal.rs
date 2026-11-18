use super::*;
use crate::{DraftClaim,FormalStorage,LengthPolicy,Role,SpanEdit,VolumeId,create_empty,current_info,initialize,
    read_current_to,read_current_range_to,read_node_range_to,read_node_to,save_first,save_interval,test_support::PrivateTemp};
use std::{fs as osfs,io::Cursor,os::unix::fs::{FileExt,MetadataExt,PermissionsExt},path::PathBuf};

fn claim(bytes:&[u8])->DraftClaim{DraftClaim::new(bytes.len() as u64,crate::hash::digest(bytes).unwrap()).unwrap()}
struct Fixture{top:PrivateTemp,_r:PrivateTemp,system:PathBuf,data:PathBuf,vol:VolumeId,base:FormalNode}
impl Fixture{
    fn new(input:&[u8])->Self{
        let top=PrivateTemp::under(&std::env::temp_dir());let system=top.child("system");let data=top.child("data");
        let rr=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));let recycle=rr.child("recycle");
        let pool=initialize(OperationId::new().unwrap(),&system,&recycle,std::slice::from_ref(&data)).unwrap();
        let vol=pool.volumes.iter().find(|p|p.volume.role==Role::File).unwrap().id;let f=create_empty(&system,OperationId::new().unwrap(),vol,"original").unwrap();
        save_first(&system,OperationId::new().unwrap(),f.file,claim(input),Cursor::new(input)).unwrap();
        let base=commit_working(&system,OperationId::new().unwrap(),f.file,1).unwrap();Self{top,_r:rr,system,data,vol,base}
    }
    fn edit(&self,rev:u64,at:u64,data:&[u8],how:LengthPolicy)->OperationId{
        let op=OperationId::new().unwrap();save_interval(&self.system,op,self.base.file,rev,SpanEdit::new(at,claim(data),how).unwrap(),Cursor::new(data)).unwrap();op
    }
    fn native(&self,n:&FormalNode)->PathBuf{self.data.join("original").join(format!("{}-original",n.storage))}
    fn work(&self,op:OperationId)->PathBuf{self.data.join("original").join(format!("working-{op}"))}
}
#[test]
#[ignore="two private registered native filesystems"]
fn second_node_has_original_parent_and_independent_packed_final_edits_not_all_raw_or_superseded_work(){
    let data=vec![b'a';3*crate::DRAFT_CHUNK+15];let t=Fixture::new(&data);let id=t.base.file;
    let held=osfs::metadata(t.native(&t.base)).unwrap();let at=crate::DRAFT_CHUNK as u64+4;
    let first=t.edit(1,at,b"12345",LengthPolicy::KeepOrGrow);let second=t.edit(2,at+1,b"xy",LengthPolicy::KeepOrGrow);
    let last=t.edit(3,data.len() as u64+2,b"!",LengthPolicy::KeepOrGrow);
    let op=OperationId::new().unwrap();let d=commit_working(&t.system,op,id,4).unwrap();
    assert_eq!(d.branch,t.base.branch);assert_eq!(d.history,t.base.history);assert_ne!(d.node,t.base.node);
    assert_ne!(d.version,t.base.version);assert_ne!(d.storage,t.base.storage);assert_eq!(d.created_from_save,last);assert_eq!(d.working_revision,4);
    assert_eq!(d.kind,FormalStorage::Incremental{parent:t.base.node,parent_version:t.base.version,changed:claim(b"1xy45!")});
    let raw=osfs::metadata(t.native(&t.base)).unwrap();assert_eq!((raw.ino(),raw.blocks(),raw.len()),(held.ino(),held.blocks(),held.len()));
    let packed=osfs::metadata(t.native(&d)).unwrap();assert!(packed.len()<4096);assert!(packed.blocks()<raw.blocks()/4);
    for old in [first,second,last]{assert!(!t.work(old).exists());}
    let mut expected=data.clone();expected[at as usize..at as usize+5].copy_from_slice(b"1xy45");expected.extend_from_slice(b"\0\0!");
    assert_eq!(d.body,claim(&expected));let mut out=Vec::new();
    let current=read_current_to(&t.system,id,&mut out).unwrap();assert_eq!(current.head,Some(d.clone()));assert!(current.edit.is_none());assert_eq!(out,expected);
    let mut partial=Vec::new();read_node_range_to(&t.system,id,d.node,at-1,7,&mut partial).unwrap();assert_eq!(partial,b"a1xy45a");
    let mut old=Vec::new();read_node_to(&t.system,id,t.base.node,&mut old).unwrap();assert_eq!(old,data);
    let other=create_empty(&t.system,OperationId::new().unwrap(),t.vol,"other").unwrap();
    assert!(read_node_range_to(&t.system,other.file,d.node,at,1,io::sink()).is_err());
    assert_eq!(osfs::read_dir(t.data.join("original")).unwrap().count(),4);
    osfs::rename(&t.data,t.top.0.join("held-original-root")).unwrap();t.top.child("data");
    assert_eq!(commit_working(&t.system,op,id,4).unwrap(),d);assert_eq!(formal_receipt(&t.system,op).unwrap().unwrap().published,Some(d.clone()));
    assert!(commit_working(&t.system,op,id,3).is_err());assert!(current_info(&t.system,id).is_err());
}
#[test]
#[ignore="two private native test filesystems"]
fn pure_resize_incr_has_empty_changed_stream_and_old_truncated_parent_bytes_never_reappear(){
    for (sizes,expected) in [(vec![0u64],b"".as_slice()),(vec![2,7],b"ab\0\0\0\0\0".as_slice())]{
        let t=Fixture::new(b"abcdef");for (i,&n) in sizes.iter().enumerate(){t.edit(i as u64+1,0,b"",LengthPolicy::Set(n));}
        let d=commit_working(&t.system,OperationId::new().unwrap(),t.base.file,1+sizes.len() as u64).unwrap();
        assert!(matches!(d.kind,FormalStorage::Incremental{changed,..} if changed==claim(b"")));assert_eq!(d.body,claim(expected));
        let mut got=Vec::new();read_node_to(&t.system,t.base.file,d.node,&mut got).unwrap();assert_eq!(got.as_slice(),expected);
        assert!(osfs::metadata(t.native(&d)).unwrap().len()>0); // metadata even when there are no changed bytes
    }
}
#[test]
#[ignore="two private native test filesystems"]
fn every_incr_cut_has_only_the_old_dirty_edge_or_the_new_node_and_retries_the_original(){
    for stop in [Cut::Reservation,Cut::IncrLinked,Cut::BeforeIntent,Cut::Intent,Cut::FrameSynced,Cut::MarkedBeforeSync,
        Cut::Decision,Cut::WorkUnlinked,Cut::WorkReleased,Cut::Installation,Cut::IntentReleased,Cut::Release]{
        let t=Fixture::new(b"abcdef");let id=t.base.file;let a=t.edit(1,1,b"X",LengthPolicy::KeepOrGrow);
        let b=t.edit(2,3,b"Y",LengthPolicy::KeepOrGrow);let op=OperationId::new().unwrap();
        assert!(commit_inner(&t.system,op,id,3,|c|if c==stop{Err(io::Error::other("cold stop"))}else{Ok(())}).is_err());
        let confirmed=matches!(stop,Cut::MarkedBeforeSync|Cut::Decision|Cut::WorkUnlinked|Cut::WorkReleased|Cut::Installation|Cut::IntentReleased|Cut::Release);
        let r=formal_receipt(&t.system,op).unwrap().unwrap();assert_eq!(r.published.is_some(),confirmed);
        let mut got=Vec::new();let current=read_current_to(&t.system,id,&mut got).unwrap();assert_eq!(got,b"aXcYef");
        if confirmed{assert_eq!(current.head,r.published);assert!(current.edit.is_none());}
        else{assert_eq!(current.head,Some(t.base.clone()));assert_eq!(current.edit.unwrap().operation,b);}
        let node=commit_working(&t.system,op,id,3).unwrap();assert_eq!(node.decision_sequence,6);assert_eq!(node.created_from_save,b);
        assert!(!t.work(a).exists()&&!t.work(b).exists());assert_eq!(osfs::read_dir(&t.system).unwrap().count(),1);
        let mut steady=Vec::new();read_current_range_to(&t.system,id,1,4,&mut steady).unwrap();assert_eq!(&steady[..],b"XcYe");
    }
}
#[test]
#[ignore="two private native test filesystems"]
fn old_frozen_reader_blocks_retirement_not_the_published_incr_and_exact_retry_does_not_publish_twice(){
    let t=Fixture::new(b"123456");let id=t.base.file;let a=t.edit(1,2,b"X",LengthPolicy::KeepOrGrow);
    let b=t.edit(2,3,b"Y",LengthPolicy::KeepOrGrow);let op=OperationId::new().unwrap();
    let (_,old)=crate::ordinary::draft::capture_current(&t.system,id).unwrap();
    assert!(matches!(&old,Some(crate::ordinary::draft::Body::Composed(_))));
    assert!(commit_working(&t.system,op,id,3).is_err());
    let receipt=formal_receipt(&t.system,op).unwrap().unwrap();assert_eq!(receipt.status,CreationStatus::PublishedInstallationUnconfirmed);
    let done=receipt.published.unwrap();assert!(t.work(a).exists()&&t.work(b).exists());
    let mut out=Vec::new();read_node_to(&t.system,id,done.node,&mut out).unwrap();assert_eq!(out,b"12XY56");
    drop(old);assert_eq!(commit_working(&t.system,op,id,3).unwrap(),done);
    assert!(!t.work(a).exists()&&!t.work(b).exists());assert_eq!(formal_receipt(&t.system,op).unwrap().unwrap().status,CreationStatus::Published);
}
#[test]
#[ignore="two private native test filesystems"]
fn corruption_in_unchanged_contributing_parent_denies_formal_decision_before_creating_named_output(){
    let t=Fixture::new(&vec![b'q';2*crate::DRAFT_CHUNK]);let id=t.base.file;let e=t.edit(1,crate::DRAFT_CHUNK as u64,b"k",LengthPolicy::KeepOrGrow);
    let before=osfs::read_dir(t.data.join("original")).unwrap().count();let reg=osfs::read(t.system.join("REGISTRATION")).unwrap();
    let f=osfs::OpenOptions::new().write(true).open(t.native(&t.base)).unwrap();f.write_all_at(b"!",9).unwrap();f.sync_all().unwrap();
    let op=OperationId::new().unwrap();assert!(commit_working(&t.system,op,id,2).is_err());
    assert!(formal_receipt(&t.system,op).unwrap().is_none());assert!(t.work(e).exists());
    assert_eq!(osfs::read_dir(t.data.join("original")).unwrap().count(),before);assert_eq!(osfs::read(t.system.join("REGISTRATION")).unwrap(),reg);
}
#[test]
#[ignore="two private native test filesystems"]
fn mutated_incremental_object_never_reads_from_an_old_working_extent_or_substitutes_a_new_inode(){
    for mode in 0..3{
        let t=Fixture::new(b"abcdefgh");let id=t.base.file;t.edit(1,3,b"XY",LengthPolicy::KeepOrGrow);
        let d=commit_working(&t.system,OperationId::new().unwrap(),id,2).unwrap();let path=t.native(&d);
        let (_,_,s)=LocalPool::with_snapshot(&t.system,Access::Read).unwrap();let rec=s.delta(id).unwrap();
        let head=rec.original.prefix(&types::Content::default()).len();let desc=map::Map::checked_bytes(rec.original.rows).unwrap();drop(s);
        if mode==2{let original=osfs::read(&path).unwrap();osfs::rename(&path,t.top.0.join("held-incr")).unwrap();
            osfs::write(&path,original).unwrap();osfs::set_permissions(&path,osfs::Permissions::from_mode(0o600)).unwrap();}
        else{let pos=if mode==0{head+4}else{head+desc};let f=osfs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
            let mut bad=[0];f.read_exact_at(&mut bad,pos as u64).unwrap();bad[0]^=0x40;f.write_all_at(&bad,pos as u64).unwrap();f.sync_all().unwrap();}
        let mut blocked=Vec::new();assert!(read_node_range_to(&t.system,id,d.node,3,1,&mut blocked).is_err());assert!(blocked.is_empty());
        let mut old=Vec::new();read_node_to(&t.system,id,t.base.node,&mut old).unwrap();assert_eq!(old,b"abcdefgh");
    }
}

#[test]
#[ignore="two private native test filesystems"]
fn no_mutable_base_after_incr_and_no_old_work_adopted_from_an_issued_name(){
    struct NeverRead;
    impl io::Read for NeverRead{
        fn read(&mut self,_:&mut[u8])->io::Result<usize>{panic!("unsupported request consumed client bytes")}
    }
    let t=Fixture::new(b"abcd");let id=t.base.file;let edit=t.edit(1,1,b"X",LengthPolicy::KeepOrGrow);
    let finished=commit_working(&t.system,OperationId::new().unwrap(),id,2).unwrap();
    let new_op=OperationId::new().unwrap();
    let next=SpanEdit::new(2,claim(b"Y"),LengthPolicy::KeepOrGrow).unwrap();
    assert!(save_interval(&t.system,new_op,id,2,next,NeverRead).is_err());
    assert!(crate::interval_receipt(&t.system,new_op).unwrap().is_none());
    assert_eq!(current_info(&t.system,id).unwrap().head,Some(finished.clone()));
    let old_name=t.work(edit);assert!(!old_name.exists());
    osfs::write(&old_name,b"unknown, not the original").unwrap();
    let mut none=Vec::new();assert!(read_node_to(&t.system,id,finished.node,&mut none).is_err());
    assert!(none.is_empty());assert_eq!(osfs::read(&old_name).unwrap(),b"unknown, not the original");
}

#[test]
#[ignore="two private native test filesystems"]
fn broken_parent_index_with_no_remaining_parent_intervals_cannot_publish_an_unreadable_increment(){
    for empty in [false,true]{
        let t=Fixture::new(b"old");let id=t.base.file;
        let e=if empty{t.edit(1,0,b"",LengthPolicy::Set(0))}else{t.edit(1,0,b"new",LengthPolicy::KeepOrGrow)};
        let folder=t.data.join("original");let before=osfs::read_dir(&folder).unwrap().count();let reg=osfs::read(t.system.join("REGISTRATION")).unwrap();
        let f=osfs::OpenOptions::new().read(true).write(true).open(folder.join(format!("node-{}",t.base.node))).unwrap();
        let at=f.metadata().unwrap().len()-1;let mut link=[0];f.read_exact_at(&mut link,at).unwrap();
        link[0]^=1;f.write_all_at(&link,at).unwrap();f.sync_all().unwrap();
        let op=OperationId::new().unwrap();assert!(commit_working(&t.system,op,id,2).is_err());
        assert_eq!(formal_receipt(&t.system,op).unwrap(),None);assert!(t.work(e).exists());
        assert_eq!(osfs::read_dir(&folder).unwrap().count(),before);
        assert_eq!(osfs::read(t.system.join("REGISTRATION")).unwrap(),reg);
        assert_eq!(osfs::read_dir(&t.system).unwrap().count(),1);
    }
}

#[test]
#[ignore="two private registered native filesystems"]
fn a_same_inode_changed_dense_byte_between_link_and_first_named_pin_cannot_certify_or_retire_the_healthy_draft(){
    let t=Fixture::new(b"abcd");let id=t.base.file;let work=t.edit(1,1,b"xyz",LengthPolicy::KeepOrGrow);
    let folder=t.data.join("original");let before:std::collections::BTreeSet<_>=osfs::read_dir(&folder).unwrap().map(|v|v.unwrap().file_name()).collect();
    let original=osfs::read(t.system.join("REGISTRATION")).unwrap();
    let op=OperationId::new().unwrap();let mut hit=0;
    assert!(commit_inner(&t.system,op,id,2,|step|{
        if step==Cut::IncrLinked{
            let mut new=osfs::read_dir(&folder)?.filter_map(|v|v.ok()).filter(|v|!before.contains(&v.file_name()));
            let leaf=new.next().unwrap();assert!(new.next().is_none());
            let f=osfs::OpenOptions::new().read(true).write(true).open(leaf.path())?;
            // Dense changed stream is exactly xyz followed by one 32-byte link.
            let pos=f.metadata()?.len()-32-3;let mut original=[0];f.read_exact_at(&mut original,pos)?;
            assert_eq!(original[0],b'x');f.write_all_at(b"!",pos)?;f.sync_all()?;hit+=1;
        }Ok(())
    }).is_err());
    assert_eq!(hit,1);assert_eq!(osfs::read(t.system.join("REGISTRATION")).unwrap(),original);
    assert!(!t.system.join("FILE-INTENT").exists());assert!(t.work(work).exists());
    let receipt=formal_receipt(&t.system,op).unwrap().unwrap();assert!(receipt.published.is_none());
    assert!(commit_working(&t.system,op,id,2).is_err());assert!(t.work(work).exists());
    let mut bytes=Vec::new();read_current_to(&t.system,id,&mut bytes).unwrap();assert_eq!(bytes,b"axyz");
}
#[test]
#[ignore="two private registered native filesystems"]
fn strict_inventory_after_partial_cleanup_refuses_equal_byte_work_name_substitutes_but_fixed_nodes_never_use_them(){
    let t=Fixture::new(b"abcdef");let id=t.base.file;
    let first=t.edit(1,1,b"x",LengthPolicy::KeepOrGrow);let last=t.edit(2,3,b"y",LengthPolicy::KeepOrGrow);
    let op=OperationId::new().unwrap();
    assert!(commit_inner(&t.system,op,id,3,|step|if step==Cut::WorkReleased{Err(io::Error::other("partial original cleanup"))}else{Ok(())}).is_err());
    assert!(!t.work(first).exists()&&t.work(last).exists());
    assert_eq!(crate::list_initial_files(&t.system,t.vol).unwrap().files.len(),1);
    let d=formal_receipt(&t.system,op).unwrap().unwrap().published.unwrap();
    let other=t.top.0.join("original-still-held");osfs::rename(t.work(last),&other).unwrap();
    let foreign=osfs::read(&other).unwrap();osfs::write(t.work(last),&foreign).unwrap();
    osfs::set_permissions(t.work(last),osfs::Permissions::from_mode(0o600)).unwrap();
    assert!(crate::list_initial_files(&t.system,t.vol).is_err());
    let mut out=Vec::new();read_node_to(&t.system,id,d.node,&mut out).unwrap();assert_eq!(out,b"axcyef");
    assert!(commit_working(&t.system,op,id,3).is_err());assert_eq!(osfs::read(t.work(last)).unwrap(),foreign);
}

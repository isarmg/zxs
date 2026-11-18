use super::*;
use crate::{create_empty,current_info,first_save_receipt,initialize,read_current_to,save_first,test_support::PrivateTemp,
    DRAFT_CHUNK,DraftClaim,FileInfo,Role,VolumeId};
use std::{fs as osfs,io::Cursor,os::unix::fs::{FileExt,MetadataExt,PermissionsExt},path::PathBuf};

struct Fixture{home:PrivateTemp,_recycle:PrivateTemp,system:PathBuf,data:PathBuf,volume:VolumeId,file:FileInfo,save:OperationId}
impl Fixture{
    fn new(bytes:&[u8])->Self{
        let home=PrivateTemp::under(&std::env::temp_dir());let system=home.child("system");let data=home.child("data");
        let recycle=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));let r=recycle.child("recycle");
        let p=initialize(OperationId::new().unwrap(),&system,&r,std::slice::from_ref(&data)).unwrap();
        let volume=p.volumes.iter().find(|x|x.volume.role==Role::File).unwrap().id;
        let file=create_empty(&system,OperationId::new().unwrap(),volume,"first.txt").unwrap();let save=OperationId::new().unwrap();
        save_first(&system,save,file.file,claim(bytes),Cursor::new(bytes)).unwrap();
        Self{home,_recycle:recycle,system,data,volume,file,save}
    }
    fn owner(&self)->PathBuf{self.data.join("first.txt")}
    fn work(&self)->PathBuf{self.owner().join(format!("working-{}",self.save))}
    fn body(&self,f:&FormalNode)->PathBuf{self.owner().join(format!("{}-{}",f.storage,f.creation_name))}
    fn graph(&self,f:&FormalNode)->PathBuf{self.owner().join(format!("node-{}",f.node))}
}
fn claim(bytes:&[u8])->DraftClaim{DraftClaim::new(bytes.len() as u64,crate::hash::digest(bytes).unwrap()).unwrap()}
#[test]
fn formal_storage_ids_are_exactly_fifteen_safe_ascii_bytes(){
    assert!(crate::VersionStorageId::checked(*b"0123456789abcde").is_ok());
    assert!(crate::VersionStorageId::checked(*b"000000000000000").is_err());
    assert!(crate::VersionStorageId::checked(*b"0123456789abc/X").is_err());
}
#[test]
#[ignore="two owned private registered native filesystems"]
fn empty_and_nonempty_first_explicit_full_are_independent_raw_files_after_cold_reopen(){
    for bytes in [b"".as_slice(),b"raw first independent body\0\xff".as_slice()]{
        let t=Fixture::new(bytes);let work_inode=osfs::metadata(t.work()).unwrap().ino();
        let before=current_info(&t.system,t.file.file).unwrap();assert!(before.head.is_none()&&before.working.is_some());
        let op=OperationId::new().unwrap();
        assert!(commit_first(&t.system,op,t.file.file,0).is_err());assert!(first_commit_receipt(&t.system,op).unwrap().is_none());
        let f=commit_first(&t.system,op,t.file.file,1).unwrap();
        assert_eq!(f.file,t.file.file);assert_eq!(f.history,t.file.history);assert_eq!(f.created_from_save,t.save);
        assert_eq!(f.creation_name,"first.txt");assert_eq!(f.storage.to_string().len(),15);assert_eq!(f.body,claim(bytes));
        assert_eq!(osfs::read(t.body(&f)).unwrap(),bytes);assert_ne!(osfs::metadata(t.body(&f)).unwrap().ino(),work_inode);
        assert!(t.graph(&f).is_file());assert!(!t.work().exists());assert_eq!(osfs::read_dir(t.owner()).unwrap().count(),3);
        let current=current_info(&t.system,t.file.file).unwrap();assert_eq!(current.head,Some(f.clone()));assert!(current.working.is_none());
        let mut out=Vec::new();assert_eq!(read_node_to(&t.system,t.file.file,f.node,&mut out).unwrap(),f);assert_eq!(out,bytes);
        out.clear();read_current_to(&t.system,t.file.file,&mut out).unwrap();assert_eq!(out,bytes);
        assert_eq!(first_save_receipt(&t.system,t.save).unwrap().unwrap().body,claim(bytes));
        assert_eq!(first_commit_receipt(&t.system,op).unwrap().unwrap().published,Some(f.clone()));
        assert_eq!(commit_first(&t.system,op,t.file.file,1).unwrap(),f);
        assert!(commit_first(&t.system,OperationId::new().unwrap(),t.file.file,1).is_err());
        let other=create_empty(&t.system,OperationId::new().unwrap(),t.volume,"sibling").unwrap();
        assert!(read_node_to(&t.system,other.file,f.node,io::sink()).is_err());
        assert!(commit_first(&t.system,op,other.file,1).is_err());assert!(commit_first(&t.system,OperationId::new().unwrap(),other.file,1).is_err());
        assert_eq!(osfs::read_dir(t.data.join("sibling")).unwrap().count(),1);
        osfs::rename(&t.data,t.home.0.join("unavailable")).unwrap();t.home.child("data");
        assert!(read_node_to(&t.system,t.file.file,f.node,io::sink()).is_err());
        assert_eq!(commit_first(&t.system,op,t.file.file,1).unwrap(),f);
    }
}
#[test]
#[ignore="two explicitly owned native test filesystems"]
fn source_intent_single_decision_and_raw_full_cleanup_resume_at_each_original_cut(){
    let bytes=b"original published once";
    for stop in [Cut::Reservation,Cut::FormalIndex,Cut::BeforeIntent,Cut::Intent,Cut::FrameSynced,Cut::MarkedBeforeSync,
        Cut::Decision,Cut::WorkReleased,Cut::Installation,Cut::IntentReleased,Cut::Release]{
        let t=Fixture::new(bytes);let op=OperationId::new().unwrap();
        commit_inner(&t.system,op,t.file.file,1,|cut|if cut==stop{Err(io::Error::other("interrupted original formal commit"))}else{Ok(())}).unwrap_err();
        let observation=first_commit_receipt(&t.system,op).unwrap().unwrap();
        let chosen=matches!(stop,Cut::MarkedBeforeSync|Cut::Decision|Cut::WorkReleased|Cut::Installation|Cut::IntentReleased|Cut::Release);
        assert_eq!(observation.published.is_some(),chosen);
        let (reserve,original_node,original_version,original_storage)={
            let (_,_,s)=LocalPool::with_snapshot(&t.system,Access::Read).unwrap();
            let a=s.committing.as_ref();let original=s.head(t.file.file).map(|f|&f.original).or_else(||a.map(|a|&a.original)).unwrap();
            (a.is_some(),original.node,original.version,original.storage)
        };
        assert_eq!(reserve,stop!=Cut::Release);
        assert_eq!(t.work().exists(),!matches!(stop,Cut::WorkReleased|Cut::Installation|Cut::IntentReleased|Cut::Release));
        let mut latest=Vec::new();let current=read_current_to(&t.system,t.file.file,&mut latest).unwrap();
        assert_eq!(current.head.is_some(),chosen);assert_eq!(current.working.is_none(),chosen);assert_eq!(latest,bytes);
        assert_eq!(read_node_to(&t.system,t.file.file,original_node,io::sink()).is_ok(),chosen);
        let eventual=commit_first(&t.system,op,t.file.file,1).unwrap();
        assert_eq!((eventual.node,eventual.version,eventual.storage),(original_node,original_version,original_storage));
        assert_eq!(eventual.decision_sequence,3);assert_eq!(osfs::read(t.body(&eventual)).unwrap(),bytes);
        assert!(!t.work().exists());assert_eq!(osfs::read_dir(&t.system).unwrap().count(),1);
        assert_eq!(eventual,commit_first(&t.system,op,t.file.file,1).unwrap());
    }
}
#[test]
#[ignore="two private native test filesystems"]
fn already_open_work_reader_is_never_unlinked_until_its_lease_finishes(){
    let t=Fixture::new(b"old pin");let old=crate::ordinary::draft::capture_current(&t.system,t.file.file).unwrap();
    let op=OperationId::new().unwrap();assert!(commit_first(&t.system,op,t.file.file,1).is_err());
    let r=first_commit_receipt(&t.system,op).unwrap().unwrap();
    assert_eq!(r.status,CreationStatus::PublishedInstallationUnconfirmed);let f=r.published.unwrap();
    assert!(t.work().exists());let mut current=Vec::new();read_node_to(&t.system,t.file.file,f.node,&mut current).unwrap();
    assert_eq!(current,b"old pin");drop(old);assert_eq!(commit_first(&t.system,op,t.file.file,1).unwrap(),f);assert!(!t.work().exists());
}
#[test]
#[ignore="two private native test filesystems"]
fn malformed_original_work_fails_before_naming_full_and_damaged_history_is_not_rebuilt_from_work(){
    let t=Fixture::new(b"first bytes".as_slice());let op=OperationId::new().unwrap();
    let before=osfs::read(t.system.join("REGISTRATION")).unwrap();let saved=osfs::read(t.work()).unwrap();
    let bad=osfs::OpenOptions::new().write(true).open(t.work()).unwrap();bad.write_all_at(&[0xfa],saved.len() as u64-33).unwrap();bad.sync_all().unwrap();drop(bad);
    assert!(commit_first(&t.system,op,t.file.file,1).is_err());assert!(first_commit_receipt(&t.system,op).unwrap().is_none());
    assert_eq!(osfs::read(t.system.join("REGISTRATION")).unwrap(),before);assert_eq!(osfs::read_dir(t.owner()).unwrap().count(),2);
    let e=Fixture::new(&vec![0x42;DRAFT_CHUNK+19]);let id=e.file.file;let originally_saved=osfs::read(e.work()).unwrap();
    commit_inner(&e.system,op,id,1,|c|if c==Cut::Decision{Err(io::Error::other("not cleaned"))}else{Ok(())}).unwrap_err();
    let formal=first_commit_receipt(&e.system,op).unwrap().unwrap().published.unwrap();let raw=e.body(&formal);
    let h=osfs::OpenOptions::new().write(true).open(&raw).unwrap();h.write_all_at(&[0x41],DRAFT_CHUNK as u64+3).unwrap();h.sync_all().unwrap();drop(h);
    let mut verified=Vec::new();assert!(read_node_to(&e.system,id,formal.node,&mut verified).is_err());assert_eq!(verified,vec![0x42;DRAFT_CHUNK]);
    assert!(commit_first(&e.system,op,id,1).is_err());assert_eq!(osfs::read(e.work()).unwrap(),originally_saved);
    assert_eq!(osfs::read(raw).unwrap()[DRAFT_CHUNK+3],0x41);
}
#[test]
#[ignore="two expressly supplied private native filesystems"]
fn same_bytes_on_a_different_inode_cannot_replace_raw_node_or_authorize_old_work_cleanup(){
    for what in ["working","raw","node"]{
        let t=Fixture::new(b"equal bytes");let op=OperationId::new().unwrap();
        commit_inner(&t.system,op,t.file.file,1,|s|if s==Cut::Decision{Err(io::Error::other("leave original work"))}else{Ok(())}).unwrap_err();
        let receipt=first_commit_receipt(&t.system,op).unwrap().unwrap().published.unwrap();
        let target=match what{"working"=>t.work(),"raw"=>t.body(&receipt),_=>t.graph(&receipt)};
        let body=osfs::read(&target).unwrap();let inode=osfs::metadata(&target).unwrap().ino();
        osfs::rename(&target,t.home.0.join("original")).unwrap();osfs::write(&target,&body).unwrap();
        osfs::set_permissions(&target,osfs::Permissions::from_mode(0o600)).unwrap();
        assert_ne!(osfs::metadata(&target).unwrap().ino(),inode);assert!(commit_first(&t.system,op,t.file.file,1).is_err());
        assert!(t.work().exists());assert_eq!(osfs::read(&target).unwrap(),body);
        let mut hist=Vec::new();let result=read_node_to(&t.system,t.file.file,receipt.node,&mut hist);
        if what=="working"{
            assert_eq!(result.unwrap(),receipt);assert_eq!(hist,b"equal bytes");
            let mut current=Vec::new();read_current_to(&t.system,t.file.file,&mut current).unwrap();assert_eq!(current,b"equal bytes");
            assert!(crate::list_initial_files(&t.system,t.volume).is_err());
        }
        else{assert!(result.is_err());assert!(hist.is_empty());}
    }
}

#[test]
#[ignore="two expressly supplied private native test filesystems"]
fn small_history_range_authenticates_the_actual_index_and_reads_only_intersecting_raw_blocks(){
    let n=DRAFT_CHUNK;let mut original=vec![0x31;2*n+37];original[n..2*n].fill(0x62);original[2*n..].fill(0xc3);
    let t=Fixture::new(&original);let f=commit_first(&t.system,OperationId::new().unwrap(),t.file.file,1).unwrap();let id=t.file.file;
    let index=t.graph(&f);let index_bytes=osfs::read(&index).unwrap();
    let actual=crate::hash::digest(&index_bytes).unwrap();
    {let (_,_,s)=LocalPool::with_snapshot(&t.system,Access::Read).unwrap();assert_eq!(s.node(id,f.node).unwrap().materials.index_digest,actual);}
    let range=|off:u64,len:u64,out:&mut Vec<u8>|read_node_range_to(&t.system,id,f.node,off,len,out);
    let mut out=Vec::new();assert_eq!(range((n-3) as u64,8,&mut out).unwrap(),f);assert_eq!(out,&original[n-3..n+5]);
    out.clear();range(original.len() as u64,0,&mut out).unwrap();assert!(out.is_empty());
    assert!(range(original.len() as u64,1,&mut out).is_err());assert!(range(u64::MAX,2,&mut out).is_err());assert!(out.is_empty());
    let raw=t.body(&f);let uh=osfs::OpenOptions::new().write(true).open(&raw).unwrap();
    uh.write_all_at(&[0xfe],1).unwrap();uh.sync_all().unwrap();drop(uh);
    // A damaged unrelated leading body block cannot be the source of our
    // late anchor. A whole-body sequential read correctly rejects that block.
    assert!(read_node_to(&t.system,id,f.node,io::sink()).is_err());
    range((2*n+6) as u64,4,&mut out).unwrap();assert_eq!(out,&original[2*n+6..2*n+10]);out.clear();
    let ih=osfs::OpenOptions::new().write(true).open(&index).unwrap();
    // Alter a link outside the request, keeping the original inode and length.
    let earlier=index_bytes.len()-3*32+2;ih.write_all_at(&[index_bytes[earlier]^1],earlier as u64).unwrap();ih.sync_all().unwrap();
    assert!(range((2*n+6) as u64,4,&mut out).is_err());assert!(out.is_empty());
    ih.write_all_at(&[index_bytes[earlier]],earlier as u64).unwrap();ih.sync_all().unwrap();drop(ih);
    let uh=osfs::OpenOptions::new().write(true).open(&raw).unwrap();uh.write_all_at(&[0],(2*n+29) as u64).unwrap();uh.sync_all().unwrap();
    // Even corruption *outside* the four requested bytes but inside their
    // selected physical block must be observed before any of it is released.
    assert!(range((2*n+6) as u64,4,&mut out).is_err());assert!(out.is_empty());
}

#[test]
#[ignore="two expressly supplied private native test filesystems"]
fn a_zero_byte_formal_node_allows_only_the_empty_range_and_still_checks_its_real_index(){
    let t=Fixture::new(b"");let f=commit_first(&t.system,OperationId::new().unwrap(),t.file.file,1).unwrap();let mut out=Vec::new();
    assert_eq!(read_node_range_to(&t.system,f.file,f.node,0,0,&mut out).unwrap(),f);
    assert!(read_node_range_to(&t.system,f.file,f.node,1,0,&mut out).is_err());assert!(out.is_empty());
    let index=t.graph(&f);let b=osfs::read(&index).unwrap();osfs::rename(&index,t.home.0.join("different-born-node")).unwrap();
    osfs::write(&index,&b).unwrap();osfs::set_permissions(&index,osfs::Permissions::from_mode(0o600)).unwrap();
    assert!(read_node_range_to(&t.system,f.file,f.node,0,0,&mut out).is_err());assert!(out.is_empty());
}

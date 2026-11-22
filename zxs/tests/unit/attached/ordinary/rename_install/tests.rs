use super::*;
use crate::{BranchId,EntryId,FileId,HistoryId,OperationId,PoolId,VolumeId,test_support::PrivateTemp};
use std::{fs::{self,OpenOptions},io::Write,os::unix::fs::{DirBuilderExt,MetadataExt,OpenOptionsExt}};
#[test]
#[ignore="source only: same original directory and physical witness; no service"]
fn exact_rename_reuses_the_file_and_witness_after_the_true_pre_sync_cut(){
    let tmp=PrivateTemp::under(&std::env::temp_dir());
    let parent=HeldDir::trusted_storage(&tmp.0).unwrap();
    let birth=Birth{pool:PoolId([1;16]),volume:VolumeId([2;16]),operation:OperationId([3;16]),
        entry:EntryId([4;16]),file:FileId([5;16]),history:HistoryId([6;16]),branch:BranchId([7;16]),
        name:"Report.txt".into(),ticket:OperationId([8;16]),slot:1};
    let old=birth.container().unwrap();let new=Birth::placed_container("report.txt",birth.file).unwrap();
    let d=parent.new_child_directory(&old).unwrap();
    let h=d.checked_child_path(c"FILE").unwrap();
    let mut file=OpenOptions::new().create_new(true).write(true).mode(0o600).open(&h).unwrap();
    file.write_all(&birth.header()).unwrap();file.sync_all().unwrap();
    let w=parent.checked_child_path(&birth.witness().unwrap()).unwrap();
    fs::hard_link(&h,&w).unwrap();
    let original=d.entity;let witness=fs::metadata(&w).unwrap().ino();
    let target=parent.checked_child_path(&new).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
    let select=|s|RenameInstall::hold(&birth,&parent,"Report.txt","report.txt",s,|p|p.only_header());
    assert!(matches!(select(RenameStage::BeforeReserve),Err(e) if e.kind()==io::ErrorKind::AlreadyExists));
    assert!(h.exists());fs::remove_dir(&target).unwrap();
    let mut reader=birth.working_records_at(&parent,&parent.names().unwrap().into_iter().collect(),false,
        |p|p.only_header()).unwrap();
    let change=select(RenameStage::BeforeReserve).unwrap();
    assert_eq!(change.current_directory().entity,original);
    assert!(matches!(select(RenameStage::BeforeReserve),Err(e) if e.kind()==io::ErrorKind::WouldBlock));
    let cut=std::cell::Cell::new(false);
    assert!(change.install(||Ok(()),||{
        assert!(!d.path.exists()&&target.is_dir());cut.set(true);Err(io::ErrorKind::Interrupted.into())
    },|_|->io::Result<()>{panic!("parent sync and final assertion not reached")}).is_err());
    assert!(cut.get());assert!(select(RenameStage::BeforeReserve).is_err());
    let (installed,())=select(RenameStage::Reserved).unwrap().install(
        ||panic!("cold retry must not execute new rename"),
        ||panic!("cold retry must not move anything"),
        |actual|{assert_eq!(actual.directory().entity,original);assert_eq!(actual.directory().path,target);Ok(())}
    ).unwrap();
    installed.require(&parent).unwrap();
    assert!(reader.require(&parent).is_err());
    reader.rebind_renamed(&parent,&installed).unwrap();
    reader.require(&parent).unwrap();
    assert_eq!(fs::metadata(target.join("FILE")).unwrap().ino(),witness);
    assert_eq!(fs::metadata(w).unwrap().ino(),witness);
    assert!(!d.path.exists());
    drop(installed);
    // This tests physical rollback only; a committed SQL operation
    // cannot supply RenameStage::Reserved to the eventual issuer.
    let (back,())=select(RenameStage::Reserved).unwrap().restore_old(||Ok(()),|actual|{
        assert_eq!(actual.directory().entity,original);
        assert_eq!(actual.directory().path,d.path);
        assert!(!target.exists());Ok(())
    }).unwrap();
    reader.rebind_renamed(&parent,&back).unwrap();
    assert_eq!(fs::metadata(&h).unwrap().ino(),witness);
}

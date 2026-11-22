use super::*;
use crate::{BranchId,EntryId,FileId,HistoryId,OperationId,PoolId,VolumeId,test_support::PrivateTemp};
use std::{cell::Cell,fs::{self,OpenOptions},io::Write,os::unix::fs::{MetadataExt,OpenOptionsExt}};

#[test]
#[ignore="source only: actual flat FILE across two parents, no service"]
fn the_single_original_file_survives_the_durable_split_without_copy(){
    let tmp=PrivateTemp::under(&std::env::temp_dir());
    let root=HeldDir::trusted_storage(&tmp.0).unwrap();
    let old=root.new_child_directory(c"Old").unwrap();
    let new=root.new_child_directory(c"New").unwrap();
    let birth=Birth{pool:PoolId([1;16]),volume:VolumeId([2;16]),operation:OperationId([3;16]),
        entry:EntryId([4;16]),file:FileId([5;16]),history:HistoryId([6;16]),branch:BranchId([7;16]),
        name:"Report".into(),ticket:OperationId([8;16]),slot:1};
    let source=old.new_child_directory(&birth.container().unwrap()).unwrap();
    let target=new.checked_child_path(&Birth::placed_container("report",birth.file).unwrap()).unwrap();
    let original_header=source.checked_child_path(c"FILE").unwrap();
    let source_link=old.checked_child_path(&birth.witness().unwrap()).unwrap();
    let target_link=new.checked_child_path(&birth.witness().unwrap()).unwrap();
    let write_header=|target:&std::path::Path|{
        let mut h=OpenOptions::new().create_new(true).write(true).mode(0o600).open(target).unwrap();
        h.write_all(&birth.header()).unwrap();h.sync_all().unwrap();
    };
    write_header(&original_header);fs::hard_link(&original_header,&source_link).unwrap();
    let inode=fs::metadata(&source_link).unwrap().ino();
    let hold=|mode|FlatParentMove::hold(&birth,&old,&new,"Report","report",mode,|p|p.only_header());
    let known=birth.working_records_at(&old,&old.names().unwrap().into_iter().collect(),false,|p|p.only_header()).unwrap();

    write_header(&target_link);
    assert!(matches!(hold(RenameStage::BeforeReserve),Err(e) if e.kind()==io::ErrorKind::AlreadyExists));
    assert!(source_link.exists()&&original_header.exists());fs::remove_file(&target_link).unwrap();
    let unchanged=hold(RenameStage::Reserved).unwrap();
    assert!(unchanged.install(|_|panic!("cold Old admission"),|_|panic!("cold Old rename"),|_|Ok(())).is_err());
    let cut=Cell::new(false);
    let first=hold(RenameStage::BeforeReserve).unwrap();
    first.require_original(&known).unwrap();
    assert!(first.install(|side|{assert_eq!(side,ParentMoveStep::Old);Ok(())},|side|{
        assert_eq!(side,ParentMoveStep::LinkAtNew);
        assert!(original_header.exists()&&target_link.exists()&&!source_link.exists()&&!target.exists());
        cut.set(true);Err(io::ErrorKind::Interrupted.into())
    },|_|->io::Result<()>{panic!("no move receipt before both names are synced")}).is_err());
    assert!(cut.get());assert!(known.require(&old).is_err());
    assert!(hold(RenameStage::BeforeReserve).is_err());

    let temporarily=new.checked_child_path(c"held-back").unwrap();
    fs::rename(&target_link,&temporarily).unwrap();
    write_header(&target_link);
    assert!(hold(RenameStage::Reserved).is_err());
    fs::remove_file(&target_link).unwrap();fs::rename(&temporarily,&target_link).unwrap();

    let split=hold(RenameStage::Reserved).unwrap();
    assert_eq!(split.observed(),ParentMoveStep::LinkAtNew);
    let (destination,())=split.install(|after|{assert_eq!(after,ParentMoveStep::LinkAtNew);Ok(())},
        |after|{assert_eq!(after,ParentMoveStep::New);Ok(())},|actual|{
            assert_eq!(actual.directory().entity,source.entity);
            assert_eq!(actual.directory().path,target);
            assert!(!source.path.exists());Ok(())
        }).unwrap();
    destination.require(&new).unwrap();
    assert_eq!(fs::metadata(target.join("FILE")).unwrap().ino(),inode);
    assert_eq!(fs::metadata(&target_link).unwrap().ino(),inode);
    drop(destination);

    let back=hold(RenameStage::Reserved).unwrap();assert_eq!(back.observed(),ParentMoveStep::New);
    let (prior,())=back.restore(|_|Ok(()),|_|Ok(()),|actual|{
        assert_eq!(actual.directory().entity,source.entity);
        assert!(source_link.exists()&&!target_link.exists()&&!target.exists());Ok(())
    }).unwrap();
    prior.require(&old).unwrap();
    assert_eq!(fs::metadata(&source_link).unwrap().ino(),inode);
}

#[test]
fn immediately_usable_target_budget_bills_both_names(){
    let occupied=vec![b"x".to_vec();DIRECTORY_NAMES-1]; // count boundary only
    assert!(room(&occupied,&[c"leaf"]).is_ok());
    assert!(matches!(room(&occupied,&[c"leaf",c"witness"]),Err(e) if e.kind()==io::ErrorKind::StorageFull));
    let occupied=vec![vec![b'x';255];DIRECTORY_BYTES/256];
    assert!(matches!(room(&occupied,&[c"leaf"]),Err(e) if e.kind()==io::ErrorKind::StorageFull));
}

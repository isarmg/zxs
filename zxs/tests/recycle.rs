//! Typed lifecycle scenario for explicitly selected, real private filesystems.
use std::{fs,io::Cursor,path::Path};
use storage_server::{DraftClaim,Entity,MoveStatus,OperationId,RecycleAction as Action,RecycleFilter,RecycleKind,RestoreTarget,Role,VolumeId};
#[path="unit/support.rs"]mod support;
const ABC:&str="ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
fn op()->OperationId{OperationId::new().unwrap()}
#[test]
#[ignore="private roots on at least two real native filesystems"]
fn delete_restore_list_and_exact_receipts_keep_the_original_with_offline_source(){
    let a=support::PrivateTemp::under(&std::env::temp_dir());let sys=a.child("system");let old=a.child("original");let to=a.child("other");
    let rr=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));let bin=rr.child("recycle");
    let pool=storage_server::initialize(op(),&sys,&bin,&[old.clone(),to.clone()]).unwrap();
    let find=|path:&Path|->VolumeId{pool.volumes.iter().find(|v|v.volume.role==Role::File&&v.volume.path==path).unwrap().id};
    let(file_root,target_root)=(find(&old),find(&to));
    let file=storage_server::create_empty(&sys,op(),file_root,"named").unwrap();
    storage_server::save_first(&sys,op(),file.file,DraftClaim::from_hex(3,ABC).unwrap(),Cursor::new(b"abc")).unwrap();
    let head=storage_server::commit_working(&sys,op(),file.file,1).unwrap();let issued=op();let restore=op();
    let d=Action::Delete{file:file.file,from:file_root};
    let applied=storage_server::apply_recycle(&sys,issued,0,d).unwrap();
    assert_eq!((applied.kind,applied.status),(RecycleKind::Delete,MoveStatus::CompleteCheckedNow));
    let key=applied.deletion.unwrap();
    assert_eq!((key.file,key.operation,key.decision),(file.file,issued,applied.decision.unwrap()));
    fs::rename(&old,a.0.join("unavailable-original")).unwrap();let off=rr.0.join("unavailable-recycle");
    fs::rename(&bin,&off).unwrap();
    let r0=storage_server::recycle_receipt(&sys,issued,0,d).unwrap().unwrap();
    assert_eq!((r0.kind,r0.status,r0.deletion),(RecycleKind::Delete,MoveStatus::OriginalPublished,Some(key)));
    let list=storage_server::list_retained(&sys,RecycleFilter::default(),8,None).unwrap();
    assert!(list.entries.iter().any(|r|r.original.deletion==Some(key)&&r.retained_by_this_deletion_now));
    fs::rename(&off,&bin).unwrap();
    let seed=Action::Restore{deleted:key,target:RestoreTarget::Volume(target_root)};
    let restored=storage_server::apply_recycle(&sys,restore,1,seed).unwrap();
    assert_eq!((restored.kind,restored.status,restored.deletion),(RecycleKind::Restore,MoveStatus::CompleteCheckedNow,Some(key)));
    let mut body=Vec::new();let n=storage_server::read_node_to(&sys,file.file,head.node,&mut body).unwrap();assert_eq!(body,b"abc");assert_eq!(n,head);
    fs::rename(&to,a.0.join("target-also-offline")).unwrap();fs::rename(&bin,&off).unwrap();
    let receipt=storage_server::recycle_receipt(&sys,restore,1,seed).unwrap().unwrap();
    assert_eq!((receipt.kind,receipt.status,receipt.deletion),(RecycleKind::Restore,MoveStatus::OriginalPublished,Some(key)));
    assert_eq!(storage_server::apply_recycle(&sys,restore,1,seed).unwrap(),receipt); // neither data root opened
    let mut wrong=key;wrong.decision=[0;32];
    assert!(storage_server::recycle_receipt(&sys,restore,1,Action::Restore{deleted:wrong,target:RestoreTarget::Volume(target_root)}).is_err());
}

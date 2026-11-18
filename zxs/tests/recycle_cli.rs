//! Run only in an explicitly selected private native fixture. Tests the real
//! public argv, receipt and listing, not a substitute command dispatcher.
use std::{fs,io::Cursor,path::Path,process::{Command,Output}};
use zxs::{DraftClaim,Entity,OperationId,Role,VolumeId};
#[path="unit/support.rs"]mod support;
const ABC:&str="ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
fn op()->OperationId{OperationId::new().unwrap()}
fn cmd(verb:&str,op:OperationId,sys:&Path,words:&[String])->Output{
    Command::new(env!("CARGO_BIN_EXE_zxs")).arg(verb).arg(op.to_string()).arg(sys).args(words).output().unwrap()
}
fn ok(r:Output)->String{assert!(r.status.success(),"{}",String::from_utf8_lossy(&r.stderr));String::from_utf8(r.stdout).unwrap()}
#[test]
#[ignore="private roots on at least two real native filesystems"]
fn actual_local_delete_restore_list_and_two_receipt_commands_use_the_signed_originals_with_an_offline_source(){
    let a=support::PrivateTemp::under(&std::env::temp_dir());let sys=a.child("system");let old=a.child("original");let to=a.child("other");
    let rr=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));let bin=rr.child("recycle");
    let pool=zxs::initialize(op(),&sys,&bin,&[old.clone(),to.clone()]).unwrap();
    let find=|path:&Path|->VolumeId{pool.volumes.iter().find(|v|v.volume.role==Role::File&&v.volume.path==path).unwrap().id};
    let(file_root,target_root)=(find(&old),find(&to));
    let file=zxs::create_empty(&sys,op(),file_root,"named").unwrap();
    zxs::save_first(&sys,op(),file.file,DraftClaim::from_hex(3,ABC).unwrap(),Cursor::new(b"abc")).unwrap();
    let head=zxs::commit_working(&sys,op(),file.file,1).unwrap();let issued=op();let restore=op();
    let d=[file.file.to_string(),file_root.to_string(),"0".to_owned()];
    let printed=ok(cmd("file-delete",issued,&sys,&d));assert!(printed.contains("Original Delete CompleteCheckedNow"));
    let receipt=zxs::recycle_receipt(&sys,issued,0,zxs::RecycleAction::Delete{file:file.file,from:file_root}).unwrap().unwrap();
    let key=receipt.deletion.unwrap();let hex:String=key.decision.iter().map(|v|format!("{v:02x}")).collect();
    assert!(printed.contains(&hex));fs::rename(&old,a.0.join("unavailable-original")).unwrap();let off=rr.0.join("unavailable-recycle");
    fs::rename(&bin,&off).unwrap();let r0=ok(cmd("file-delete-receipt",issued,&sys,&d));assert!(r0.contains("Original Delete OriginalPublished")&&r0.contains(&hex));
    let list=ok(Command::new(env!("CARGO_BIN_EXE_zxs")).arg("recycle-list").arg(&sys).args(["8","all","-","-"]).output().unwrap());
    assert!(list.contains("availability not inspected")&&list.contains(&hex)&&list.contains(&file.file.to_string()));
    fs::rename(&off,&bin).unwrap();
    let args=[file.file.to_string(),issued.to_string(),hex.clone(),"1".to_owned(),target_root.to_string()];
    let restored=ok(cmd("file-restore",restore,&sys,&args));assert!(restored.contains("Original Restore CompleteCheckedNow")&&restored.contains(&hex));
    let mut body=Vec::new();let n=zxs::read_node_to(&sys,file.file,head.node,&mut body).unwrap();assert_eq!(body,b"abc");assert_eq!(n,head);
    fs::rename(&to,a.0.join("target-also-offline")).unwrap();fs::rename(&bin,&off).unwrap();
    let restored=ok(cmd("file-restore-receipt",restore,&sys,&args));assert!(restored.contains("Original Restore OriginalPublished"));
    assert!(ok(cmd("file-restore",restore,&sys,&args)).contains(&hex)); // original replay, neither data root opened
    let mut wrong=args.clone();wrong[2]="00".repeat(32);let rejected=cmd("file-restore-receipt",restore,&sys,&wrong);
    assert!(!rejected.status.success()&&rejected.stdout.is_empty());
}

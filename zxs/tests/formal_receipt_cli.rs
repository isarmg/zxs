//! Actual public argv/exit status/stdout, not a library-only receipt assertion.
//! Source-only opt-in; requires two private native test filesystems.
use std::{fs,io::Cursor,path::Path,process::Command};
use zxs::{DraftClaim,Entity,LengthPolicy,OperationId,Role,SpanEdit};
#[path="unit/support.rs"] mod support;
const ABC:&str="ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
fn cli(system:&Path,op:OperationId)->std::process::Output{
    Command::new(env!("CARGO_BIN_EXE_zxs")).arg("file-commit-receipt").arg(system).arg(op.to_string()).output().unwrap()
}
#[test]
#[ignore="two private registered native filesystems"]
fn formal_receipt_command_returns_both_complete_originals_offline_and_never_returns_a_working_save(){
    let t=support::PrivateTemp::under(&std::env::temp_dir());let system=t.child("system");let data=t.child("data");
    let rr=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let reg=zxs::initialize(OperationId::new().unwrap(),&system,&rr.child("recycle"),std::slice::from_ref(&data)).unwrap();
    let vol=reg.volumes.iter().find(|v|v.volume.role==Role::File).unwrap().id;
    let id=zxs::create_empty(&system,OperationId::new().unwrap(),vol,"source").unwrap().file;
    let saved=OperationId::new().unwrap();let abc=DraftClaim::from_hex(3,ABC).unwrap();
    zxs::save_first(&system,saved,id,abc,Cursor::new(b"abc")).unwrap();
    let first=OperationId::new().unwrap();let original=zxs::commit_working(&system,first,id,1).unwrap();
    zxs::save_interval(&system,OperationId::new().unwrap(),id,1,SpanEdit::new(1,abc,LengthPolicy::KeepOrGrow).unwrap(),Cursor::new(b"abc")).unwrap();
    let second=OperationId::new().unwrap();let next=zxs::commit_working(&system,second,id,2).unwrap();
    fs::rename(&data,t.0.join("offline")).unwrap();t.child("data");
    let f=cli(&system,first);assert!(f.status.success());let f=String::from_utf8(f.stdout).unwrap();
    assert!(f.contains("Published manual FULL")&&f.contains(&original.node.to_string())&&f.contains(&format!("body-sha256 {ABC}")));
    let d=cli(&system,second);assert!(d.status.success());let d=String::from_utf8(d.stdout).unwrap();
    let logical:String=next.body.sha256.iter().map(|b|format!("{b:02x}")).collect();
    assert!(d.contains(&format!("INCR parent {} parent-version {}",original.node,original.version)));
    assert!(d.contains(&format!("packed-bytes 3 packed-sha256 {ABC}"))&&d.contains(&format!("body-sha256 {logical}")));
    assert!(d.contains(&next.node.to_string())&&d.contains(&second.to_string()));
    let wrong=cli(&system,saved);assert!(!wrong.status.success()&&wrong.stdout.is_empty());
    assert!(!cli(&system,OperationId::new().unwrap()).status.success());
}

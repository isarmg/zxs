//! Offline typed storage fixture only; not a shipped command or a service.
use std::{fs,io::Cursor,path::Path};
use zxs::{DraftClaim,Entity,FormalStorage,LengthPolicy,OperationId,Role,SpanEdit};
#[path="unit/support.rs"] mod support;
const ABC:&str="ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
#[test]
#[ignore="two private registered native filesystems"]
fn original_offline_formal_receipts_never_return_a_working_save(){
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
    let f=zxs::formal_receipt(&system,first).unwrap().unwrap();
    assert_eq!((f.file,f.operation),(id,first));assert_eq!(f.published.as_ref(),Some(&original));
    assert_eq!((original.kind.clone(),original.body),(FormalStorage::Full,abc));
    let d=zxs::formal_receipt(&system,second).unwrap().unwrap();
    assert_eq!((d.file,d.operation),(id,second));assert_eq!(d.published.as_ref(),Some(&next));
    assert_eq!(next.kind,FormalStorage::Incremental{parent:original.node,parent_version:original.version,changed:abc});
    assert!(zxs::formal_receipt(&system,saved).is_err());
    assert!(zxs::formal_receipt(&system,OperationId::new().unwrap()).unwrap().is_none());
}

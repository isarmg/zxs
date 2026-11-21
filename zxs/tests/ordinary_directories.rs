//! Typed mixed directory and FILE workload against real separate fixtures.
use std::{collections::BTreeSet,fs,io::Cursor,path::Path};
use zxs::{DirectoryEntry,DirectoryParent as Parent,Entity,OperationId,Role};
#[path="unit/support.rs"]mod support;
fn op()->OperationId{OperationId::new().unwrap()}
#[test]
#[ignore="real original roots and separate recycle filesystem; no mounting or device changes"]
fn mixed_root_dir_navigation_does_not_disguise_a_directory_as_a_file_or_as_vacancy_for_restore(){
    let t=support::PrivateTemp::under(&std::env::temp_dir());
    let r=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("sys");let a=t.child("source");let b=t.child("target");let recycle=r.child("recycle");
    let pool=zxs::initialize(op(),&sys,&recycle,&[a.clone(),b.clone()]).unwrap();
    let role=|path:&Path|pool.volumes.iter().find(|v|v.volume.path==path).unwrap().id;
    let first=role(&a);let second=role(&b);let(aa,bb)=(Parent::Root(first),Parent::Root(second));
    let live=zxs::create_empty(&sys,op(),first,"live.txt").unwrap();let file=zxs::create_empty(&sys,op(),first,"report").unwrap();
    let claim=zxs::DraftClaim::from_hex(3,"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").unwrap();
    zxs::save_first(&sys,op(),file.file,claim,Cursor::new(b"abc")).unwrap();let node=zxs::commit_working(&sys,op(),file.file,1).unwrap();
    let del=zxs::apply_recycle(&sys,op(),0,zxs::RecycleAction::Delete{file:file.file,from:first}).unwrap();let key=del.deletion.unwrap();
    let original=op();let report=zxs::make_directory(&sys,original,aa,"report").unwrap();
    let dir=Parent::Child(first,report.directory);
    let nested=zxs::make_directory(&sys,op(),dir,"same").unwrap();
    let side=zxs::make_directory(&sys,op(),aa,"side").unwrap();
    let separate=zxs::make_directory(&sys,op(),Parent::Child(first,side.directory),"same").unwrap();
    assert_ne!(nested.directory,separate.directory);
    let files=zxs::list_initial_files(&sys,first).unwrap();
    assert_eq!(files.files.iter().map(|f|f.file).collect::<Vec<_>>(),vec![live.file]);
    let all=zxs::list_directory(&sys,aa).unwrap();
    assert_eq!(all.entries.iter().map(DirectoryEntry::name).collect::<BTreeSet<_>>(),
        BTreeSet::from(["live.txt","report","side"]));
    assert!(all.entries.iter().any(|e|matches!(e,DirectoryEntry::File(f) if f.file==live.file)));
    assert!(all.entries.contains(&DirectoryEntry::Directory(report.clone())));
    assert!(all.entries.contains(&DirectoryEntry::Directory(side)));
    let inside=zxs::list_directory(&sys,dir).unwrap();assert_eq!(inside.entries,vec![DirectoryEntry::Directory(nested)]);
    assert!(zxs::list_directory(&sys,Parent::Child(second,report.directory)).is_err());
    assert!(zxs::list_directory(&sys,Parent::Child(first,live.file.to_string().parse().unwrap())).is_err());
    let rid=pool.volumes.iter().find(|x|x.volume.role==Role::Recycle).unwrap().id;
    assert!(zxs::list_directory(&sys,Parent::Root(rid)).is_err());
    let attempted=op();let original_target=zxs::RecycleAction::Restore{deleted:key,target:zxs::RestoreTarget::Original};
    assert!(zxs::apply_recycle(&sys,attempted,1,original_target).is_err());
    assert!(zxs::recycle_receipt(&sys,attempted,1,original_target).unwrap().is_none());
    assert!(zxs::create_empty(&sys,op(),first,"report").is_err());
    assert_eq!(zxs::make_directory(&sys,original,aa,"report").unwrap(),report);
    assert_eq!(zxs::list_directory(&sys,dir).unwrap(),inside);
    let parked=t.0.join("original-offline");fs::rename(&a,&parked).unwrap();
    assert_eq!(zxs::make_directory(&sys,original,aa,"report").unwrap(),report);
    assert!(zxs::list_directory(&sys,dir).is_err());assert!(zxs::list_directory(&sys,bb).unwrap().entries.is_empty());
    let restored=zxs::apply_recycle(&sys,op(),1,zxs::RecycleAction::Restore{deleted:key,target:zxs::RestoreTarget::Volume(second)}).unwrap();
    assert_eq!(restored.file,file.file);let mut bytes=Vec::new();assert_eq!(zxs::read_node_to(&sys,file.file,node.node,&mut bytes).unwrap(),node);
    assert_eq!(bytes,b"abc");fs::rename(parked,&a).unwrap();
    assert_eq!(zxs::list_initial_files(&sys,first).unwrap().files,files.files);
    assert_eq!(zxs::list_directory(&sys,aa).unwrap(),all);
    assert!(zxs::list_initial_files(&sys,second).unwrap().files.iter().any(|f|f.file==file.file));
}

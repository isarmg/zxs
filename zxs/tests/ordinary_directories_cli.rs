//! Native source scenario only. These commands exercise the shipped CLI
//! parser, not a stand-in dispatcher or a synthetic two-type listing.
use std::{fs,io::Cursor,path::Path,process::{Command,Output}};
use zxs::{DirectoryId,Entity,OperationId,Role};
#[path="unit/support.rs"]mod support;
fn op()->OperationId{OperationId::new().unwrap()}
fn invoke(verb:&str,issued:Option<OperationId>,sys:&Path,tail:&[&str])->Output{
    let mut c=Command::new(env!("CARGO_BIN_EXE_zxs"));c.arg(verb);if let Some(o)=issued{c.arg(o.to_string());}
    c.arg(sys).args(tail).output().unwrap()
}
fn ok(x:Output)->String{assert!(x.status.success(),"{}",String::from_utf8_lossy(&x.stderr));String::from_utf8(x.stdout).unwrap()}
fn id(text:&str)->DirectoryId{text.lines().find_map(|s|s.strip_prefix("directory ")).unwrap().split_whitespace().next().unwrap().parse().unwrap()}
#[test]
#[ignore="real original roots and separate recycle filesystem; actual CLI argv, no mounting or device changes"]
fn mixed_root_dir_navigation_does_not_disguise_a_directory_as_a_file_or_as_vacancy_for_restore(){
    let t=support::PrivateTemp::under(&std::env::temp_dir());
    let r=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("sys");let a=t.child("source");let b=t.child("target");let recycle=r.child("recycle");
    let pool=zxs::initialize(op(),&sys,&recycle,&[a.clone(),b.clone()]).unwrap();
    let role=|path:&Path|pool.volumes.iter().find(|v|v.volume.path==path).unwrap().id;
    let first=role(&a);let second=role(&b);let(aa,bb)=(first.to_string(),second.to_string());
    let live=zxs::create_empty(&sys,op(),first,"live.txt").unwrap();let file=zxs::create_empty(&sys,op(),first,"report").unwrap();
    let claim=zxs::DraftClaim::from_hex(3,"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").unwrap();
    zxs::save_first(&sys,op(),file.file,claim,Cursor::new(b"abc")).unwrap();let node=zxs::commit_working(&sys,op(),file.file,1).unwrap();
    let del=zxs::apply_recycle(&sys,op(),0,zxs::RecycleAction::Delete{file:file.file,from:first}).unwrap();let key=del.deletion.unwrap();
    let original=op();let report=id(&ok(invoke("dir-mkdir",Some(original),&sys,&[&aa,"root","report"])));
    let dir=report.to_string();let native=id(&ok(invoke("dir-mkdir",Some(op()),&sys,&[&aa,&dir,"same"])));
    let side=id(&ok(invoke("dir-mkdir",Some(op()),&sys,&[&aa,"root","side"]))).to_string();
    let separate=id(&ok(invoke("dir-mkdir",Some(op()),&sys,&[&aa,&side,"same"])));assert_ne!(native,separate);
    let files=ok(invoke("file-list",None,&sys,&[&aa]));assert_eq!(files.lines().count(),1);assert!(files.contains(&live.file.to_string()));
    let all=ok(invoke("dir-list",None,&sys,&[&aa,"root"]));assert_eq!(all.lines().count(),3);assert!(all.contains(&live.file.to_string())&&all.contains(&dir)&&all.contains(&side));
    let inside=ok(invoke("dir-list",None,&sys,&[&aa,&dir]));assert_eq!(inside.lines().count(),1);assert!(inside.contains(&native.to_string())&&!inside.contains(&separate.to_string()));
    assert!(!invoke("dir-list",None,&sys,&[&bb,&dir]).status.success());
    assert!(!invoke("dir-list",None,&sys,&[&aa,&live.file.to_string()]).status.success());
    let rid=pool.volumes.iter().find(|x|x.volume.role==Role::Recycle).unwrap().id.to_string();assert!(!invoke("dir-list",None,&sys,&[&rid,"root"]).status.success());
    let attempted=op();let original_target=zxs::RecycleAction::Restore{deleted:key,target:zxs::RestoreTarget::Original};
    assert!(zxs::apply_recycle(&sys,attempted,1,original_target).is_err());
    assert!(zxs::recycle_receipt(&sys,attempted,1,original_target).unwrap().is_none());
    assert!(zxs::create_empty(&sys,op(),first,"report").is_err());
    assert_eq!(id(&ok(invoke("dir-mkdir",Some(original),&sys,&[&aa,"root","report"]))),report);
    assert_eq!(ok(invoke("dir-list",None,&sys,&[&aa,&dir])),inside);
    let parked=t.0.join("original-offline");fs::rename(&a,&parked).unwrap();
    assert_eq!(id(&ok(invoke("dir-mkdir",Some(original),&sys,&[&aa,"root","report"]))),report);
    assert!(!invoke("dir-list",None,&sys,&[&aa,&dir]).status.success());assert!(ok(invoke("dir-list",None,&sys,&[&bb,"root"])).is_empty());
    let restored=zxs::apply_recycle(&sys,op(),1,zxs::RecycleAction::Restore{deleted:key,target:zxs::RestoreTarget::Volume(second)}).unwrap();
    assert_eq!(restored.file,file.file);let mut bytes=Vec::new();assert_eq!(zxs::read_node_to(&sys,file.file,node.node,&mut bytes).unwrap(),node);
    assert_eq!(bytes,b"abc");fs::rename(parked,&a).unwrap();
    assert_eq!(ok(invoke("file-list",None,&sys,&[&aa])),files);
    assert_eq!(ok(invoke("dir-list",None,&sys,&[&aa,"root"])),all);
    assert!(ok(invoke("file-list",None,&sys,&[&bb])).contains(&file.file.to_string()));
}

use std::os::unix::fs::{DirBuilderExt,OpenOptionsExt};
use super::*;
use crate::test_support::PrivateTemp;
fn slot(parent:DirectoryParent,id:u8,name:&str)->DirectoryBirth{
    DirectoryBirth{pool:PoolId([1;16]),volume:VolumeId([2;16]),parent,id:DirectoryId([id;16]),
        entry:EntryId([id+32;16]),original:OperationId([id+64;16]),name:name.into()}
}
#[test]
fn fresh_and_cold_stage_charge_only_the_exact_peak_in_the_native_parent(){
    let all:BTreeSet<_>=(0..DIRECTORY_NAMES-1).map(|n|n.to_string().into_bytes()).collect();
    let long=slot(DirectoryParent::Root,3,&"x".repeat(180));
    assert_eq!(long.room(&all,false).unwrap_err().kind(),io::ErrorKind::StorageFull);
    let mut stage=all.clone();stage.insert(long.stage().into_bytes());
    assert!(long.room(&stage,true).is_ok());
    let mut bytes:BTreeSet<_>=(0..2047u16).map(|i|{let mut v=vec![b'x';255];v[..2].copy_from_slice(&i.to_be_bytes());v}).collect();
    bytes.insert(long.stage().into_bytes());bytes.insert(long.witness().into_bytes());
    assert!(long.room(&bytes,true).is_ok());
    bytes.insert(vec![b'y';100]);
    assert_eq!(long.room(&bytes,true).unwrap_err().kind(),io::ErrorKind::StorageFull);
}
#[test]
#[ignore="source-only native directory inode and exact outer marker fixture, no mounted user disks"]
fn traced_real_child_names_and_their_two_original_links_bind_parent_identity(){
    let t=PrivateTemp::under(&std::env::temp_dir());
    let root=HeldDir::trusted_storage(&t.0).unwrap();root.exclusive().unwrap();
    let p=slot(DirectoryParent::Root,3,"Reports");let second=slot(DirectoryParent::Root,4,"reports");
    let held=p.install(&root).unwrap();
    assert_eq!(p.install(&root).unwrap().dir.entity,held.dir.entity);
    assert_ne!(second.install(&root).unwrap().dir.entity,held.dir.entity);
    assert!(!t.0.join(p.stage().to_str().unwrap()).exists());

    let child=slot(DirectoryParent::Directory(p.id),5,"2027");
    held.directory().exclusive().unwrap();
    let next=child.install(held.directory()).unwrap();
    assert_eq!(next.dir.path,t.0.join("Reports/2027"));
    assert!(child.open(&root).is_err());
    let mut wrong=child.clone();wrong.parent=DirectoryParent::Directory(second.id);
    assert!(wrong.open(held.directory()).is_err());
    let pin=t.0.join(p.witness().to_str().unwrap());let hidden=t.0.join("parked-pin");
    std::fs::rename(&pin,&hidden).unwrap();
    let rogue=std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&pin).unwrap();
    rogue.write_all_at(&p.header(),0).unwrap();rogue.sync_all().unwrap();drop(rogue);
    assert!(p.open(&root).is_err());assert!(!held.require(&root).is_ok());
    std::fs::remove_file(&pin).unwrap();std::fs::rename(&hidden,&pin).unwrap();
    assert_eq!(p.open(&root).unwrap().dir.entity,held.dir.entity);

    let bad=slot(DirectoryParent::Root,6,"Guessed");let raw=t.0.join("Guessed");
    std::fs::DirBuilder::new().mode(0o700).create(&raw).unwrap();
    assert!(bad.install(&root).is_err());assert!(raw.is_dir());
    let incomplete=slot(DirectoryParent::Root,7,"Partial");
    let half=t.0.join(incomplete.stage().to_str().unwrap());
    std::fs::DirBuilder::new().mode(0o700).create(&half).unwrap();
    assert!(incomplete.install(&root).is_err()&&!t.0.join("Partial").exists()&&half.is_dir());
}

use super::*;
use crate::{BranchId,FileId,HistoryId,ordinary::portable_birth::Birth,test_support::PrivateTemp};
use std::{fs::{self,OpenOptions},io::Write,os::unix::fs::OpenOptionsExt};

fn birth(parent:DirectoryParent,n:u8,name:&str)->DirectoryBirth{
    DirectoryBirth{pool:PoolId([1;16]),volume:VolumeId([2;16]),parent,id:DirectoryId([n;16]),
        entry:EntryId([n+32;16]),original:OperationId([n+64;16]),name:name.into()}
}

#[test]
#[ignore="source-only cross-parent fault cut and actual directory/marker identity on a fresh private fixture"]
fn interrupted_link_reopens_one_original_pair_then_moves_the_same_tree(){
    let t=PrivateTemp::under(std::path::Path::new("/tmp"));
    let floor=HeldDir::trusted_storage(&t.0).unwrap();
    let left=birth(DirectoryParent::Root,3,"Left");let right=birth(DirectoryParent::Root,4,"Right");
    let a=left.install(&floor).unwrap();let b=right.install(&floor).unwrap();
    let original=birth(DirectoryParent::Directory(left.id),5,"Draft");
    let child=birth(DirectoryParent::Directory(original.id),6,"Inside");
    let opened=original.install(a.directory()).unwrap();let entity=opened.directory().entity;
    let initial_stamp=opened.marker.stamp();
    let child_entity=child.install(opened.directory()).unwrap().directory().entity;
    let empty=birth(DirectoryParent::Directory(original.id),7,"Empty");
    let empty_entity=empty.install(opened.directory()).unwrap().directory().entity;
    let file=Birth{pool:original.pool,volume:original.volume,operation:OperationId([101;16]),
        entry:EntryId([102;16]),file:FileId([103;16]),history:HistoryId([104;16]),
        branch:BranchId([105;16]),name:"Report".into(),ticket:OperationId([106;16]),slot:1};
    let holder=opened.directory().new_child_directory(&file.container().unwrap()).unwrap();
    let file_entity=holder.entity;
    let header=holder.checked_child_path(c"FILE").unwrap();
    let mut output=OpenOptions::new().create_new(true).write(true).mode(0o600).open(&header).unwrap();
    output.write_all(&file.header()).unwrap();output.sync_all().unwrap();drop(output);
    fs::hard_link(&header,opened.directory().checked_child_path(&file.witness().unwrap()).unwrap()).unwrap();
    drop(holder);
    drop(opened);
    let cold=DirectoryParentMove::hold_reserved(&original,&floor,a.directory(),b.directory(),"Draft","Draft").unwrap();
    assert_eq!(cold.phase(),Phase::Old);
    assert!(cold.settle(DirectoryRenameSide::Destination,|_|panic!("cold OLD never starts"),
        |_|panic!("there must be no syscall"),|_|Ok(())).is_err());
    let mut owner=Some(original.open(a.directory()).unwrap());
    let mut hot=DirectoryParentMove::hold_live(&original,&floor,a.directory(),b.directory(),"Draft","Draft",&mut owner).unwrap();
    assert!(owner.is_none());
    assert_eq!(hot.parents().0.entity,a.directory().entity);assert_eq!(hot.parents().1.entity,b.directory().entity);
    let (source,target)=hot.reservation_sample().unwrap();
    assert!(source.contains(original.witness().to_bytes())&&!target.contains(original.witness().to_bytes()));
    let mut split=false;
    assert!(hot.settle(DirectoryRenameSide::Destination,|seen|{assert_eq!(seen,Phase::Old);Ok(())},|at|{
        assert_eq!(at,Phase::LinkAtNew);split=true;Err(io::ErrorKind::ConnectionAborted.into())
    },|_| -> io::Result<()> {panic!("split cannot become SQL success")}).is_err());
    assert!(split&&a.directory().entry_present(c"Draft").unwrap());
    assert!(!a.directory().entry_present(&original.witness()).unwrap());
    assert!(!b.directory().entry_present(c"Draft").unwrap());
    assert!(b.directory().entry_present(&original.witness()).unwrap());
    assert!(original.open(a.directory()).is_err());
    let cold=DirectoryParentMove::hold_reserved(&original,&floor,a.directory(),b.directory(),"Draft","Draft").unwrap();
    assert_eq!(cold.phase(),Phase::LinkAtNew);
    let (moved,())=cold.settle(DirectoryRenameSide::Destination,|seen|{assert_eq!(seen,Phase::LinkAtNew);Ok(())},
        |at|{assert_eq!(at,Phase::New);Ok(())},|held|{assert_eq!(held.directory().entity,entity);Ok(())}).unwrap();
    assert_eq!(moved.directory().path,b.directory().path.join("Draft"));
    assert!(initial_stamp.same_moved_file_header(moved.marker.stamp()));
    assert_eq!(child.open(moved.directory()).unwrap().directory().entity,child_entity);
    assert_eq!(empty.open(moved.directory()).unwrap().directory().entity,empty_entity);
    let sample=moved.directory().names().unwrap().into_iter().collect();
    let original_file=file.installed(moved.directory(),&sample).unwrap();
    assert_eq!(original_file.directory().entity,file_entity);
    drop(original_file);
    assert!(!a.directory().entry_present(c"Draft").unwrap());
    assert_eq!(original.open(b.directory()).unwrap().directory().entity,entity);
    // A cold observed NEW can reverse the exact pair when the approved
    // inverse (not a fresh forward) was selected. Body moves first.
    drop(moved);
    let cold=DirectoryParentMove::hold_reserved(&original,&floor,a.directory(),b.directory(),"Draft","Draft").unwrap();
    assert_eq!(cold.phase(),Phase::New);
    let mut steps=Vec::new();
    let (back,())=cold.settle(DirectoryRenameSide::Original,|_|Ok(()),|p|{steps.push(p);Ok(())},|_|Ok(())).unwrap();
    assert_eq!(steps,[Phase::LinkAtNew,Phase::Old]);
    assert_eq!(back.directory().entity,entity);
    assert!(initial_stamp.same_moved_file_header(back.marker.stamp()));
    assert_eq!(original.open(a.directory()).unwrap().directory().entity,entity);
    assert!(!b.directory().entry_present(&original.witness()).unwrap());
}

#[test]
#[ignore="source-only real independent ancestor hold, destination collision and physical cycle refusal"]
fn an_original_owner_does_not_lose_its_directory_to_busy_or_wrong_raw_names(){
    let t=PrivateTemp::under(std::path::Path::new("/tmp"));
    let floor=HeldDir::trusted_storage(&t.0).unwrap();
    let a=birth(DirectoryParent::Root,3,"Left").install(&floor).unwrap();
    let right=birth(DirectoryParent::Root,4,"Right");let b=right.install(&floor).unwrap();
    let original=birth(DirectoryParent::Directory(DirectoryId([3;16])),5,"Draft");
    let child=birth(DirectoryParent::Directory(original.id),6,"Inside");
    let opened=original.install(a.directory()).unwrap();let entity=opened.directory().entity;
    let descendant=child.install(opened.directory()).unwrap();
    let ancestor_for_other=original.open(a.directory()).unwrap();
    let mut owner=Some(opened);
    let Err(busy)=DirectoryParentMove::hold_live(&original,&floor,a.directory(),b.directory(),"Draft","Other",&mut owner) else{panic!("EX cannot pass a real competing ancestor SH");};
    assert_eq!(busy.kind(),io::ErrorKind::WouldBlock);
    drop(ancestor_for_other);
    let probe=original.open(a.directory()).unwrap();assert!(probe.marker.file().try_lock().is_err());drop(probe);
    let Err(cycle)=DirectoryParentMove::hold_live(&original,&floor,a.directory(),descendant.directory(),"Draft","Other",&mut owner) else{panic!("a real descendant cannot be a new parent");};
    assert_eq!(cycle.kind(),io::ErrorKind::InvalidInput);
    assert_eq!(owner.as_ref().unwrap().directory().entity,entity);
    let other=birth(DirectoryParent::Directory(right.id),7,"Other").install(b.directory()).unwrap();
    let Err(occupied)=DirectoryParentMove::hold_live(&original,&floor,a.directory(),b.directory(),"Draft","Other",&mut owner) else{panic!("the actual target is not vacant");};
    assert_eq!(occupied.kind(),io::ErrorKind::AlreadyExists);
    assert_eq!(original.open(a.directory()).unwrap().directory().entity,entity);
    assert_eq!(b.directory().exact_directory(c"Other").unwrap().unwrap().entity,other.directory().entity);
    assert!(!b.directory().entry_present(&original.witness()).unwrap());
}

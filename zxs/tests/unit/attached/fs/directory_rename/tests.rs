use super::*;
use crate::test_support::PrivateTemp;
fn birth(parent:DirectoryParent,n:u8,name:&str)->DirectoryBirth{
    DirectoryBirth{pool:PoolId([1;16]),volume:VolumeId([2;16]),parent,id:DirectoryId([n;16]),
        entry:EntryId([n+32;16]),original:OperationId([n+64;16]),name:name.into()}
}
#[test]
#[ignore="source-only held original directory, normal rename and exact cold finish on a fresh private fixture"]
fn renaming_the_parent_keeps_the_original_child_and_cold_finish_only_syncs(){
    let t=PrivateTemp::under(std::path::Path::new("/tmp"));
    let root=HeldDir::trusted_storage(&t.0).unwrap();
    let original=birth(DirectoryParent::Root,3,"Draft");
    let old=original.install(&root).unwrap();
    let child=birth(DirectoryParent::Directory(original.id),4,"Raw");
    let nested=child.install(old.directory()).unwrap();let inode=nested.directory().entity;drop(nested);
    // A distinct true held marker SH is the ancestor proof of a live
    // descendant. The same native inode is not sufficient to bypass it.
    let ancestry=original.open(&root).unwrap();
    let mut owned=Some(old);
    let Err(error)=DirectoryRename::hold_live(&original,&root,"Draft","Finished",&mut owned) else{
        panic!("an independent ancestor lease must block the rename");
    };
    assert_eq!(error.kind(),io::ErrorKind::WouldBlock);
    drop(ancestry);
    let observer=original.open(&root).unwrap();
    assert!(observer.marker.file().try_lock().is_err()); // the restored original really is SH
    drop(observer);drop(owned.take().unwrap());
    let cold=DirectoryRename::hold(&original,&root,"Draft","Finished",None).unwrap();
    assert_eq!(cold.side(),DirectoryRenameSide::Original);
    assert!(cold.settle(DirectoryRenameSide::Destination,||panic!("cold old cannot begin"),||Ok(()),|_|Ok(())).is_err());
    let mut did_move=false;
    let hot=DirectoryRename::hold(&original,&root,"Draft","Finished",Some(original.open(&root).unwrap())).unwrap();
    assert_eq!(hot.parent().entity,root.entity);
    assert!(hot.settle(DirectoryRenameSide::Destination,||Ok(()),||{
        did_move=true;Err(io::ErrorKind::ConnectionAborted.into())
    },|_| -> io::Result<()> {panic!("cannot confirm at the interruption")}).is_err());
    assert!(did_move&&!root.path.join("Draft").exists());
    let new=DirectoryRename::hold(&original,&root,"Draft","Finished",None).unwrap();
    assert_eq!(new.side(),DirectoryRenameSide::Destination);
    let (opened,())=new.settle(DirectoryRenameSide::Destination,||panic!("no second rename"),||panic!("no second rename"),|_|Ok(())).unwrap();
    assert_eq!(opened.directory().path,root.path.join("Finished"));
    assert_eq!(child.open(opened.directory()).unwrap().directory().entity,inode);
    assert_eq!(opened.witness,original.witness());
}

#[test]
#[ignore="source-only exact-case relabel of one directory; no SMB listener"]
fn the_new_exact_spelling_reopens_the_same_birth_not_a_second_marker(){
    let t=PrivateTemp::under(std::path::Path::new("/tmp"));
    let root=HeldDir::trusted_storage(&t.0).unwrap();
    let b=birth(DirectoryParent::Root,8,"Reports");
    let original=b.install(&root).unwrap();let entity=original.directory().entity;
    // An empty directory may have only its current marker; the
    // merely present leaves alone would fit this proposed near-limit
    // path, while a full pending private pocket plus material cannot.
    let almost=PathBudget{bytes:MAX_PATH_BYTES-200,depth:5};
    assert!(almost.child(DIRECTORY_MARK).is_ok());
    let maximum=CString::new(vec![b'x';255]).unwrap();let mut remaining=MAX_GEOMETRY_NAMES;
    assert!(subtree_fits(original.directory(),almost,&maximum,&mut remaining).is_err());
    let held=DirectoryRename::hold(&b,&root,"Reports","reports",Some(original)).unwrap();
    let (moved,())=held.settle(DirectoryRenameSide::Destination,||Ok(()),||Ok(()),|_|Ok(())).unwrap();
    assert_eq!(moved.directory().entity,entity);
    let names=sample(&root).unwrap();
    assert!(!names.contains(b"Reports".as_slice())&&names.contains(b"reports".as_slice()));
    assert_eq!(b.open_position(&root,&names,"reports").unwrap().directory().entity,entity);
    assert!(b.open_position(&root,&names,"Reports").is_err());
    assert!(b.open_position(&root,&names,b.witness().to_str().unwrap()).is_err());
}

#[test]
#[ignore="source-only same-parent directory collision and original rollback in one fresh private filesystem"]
fn a_collision_is_untouched_and_only_a_proved_backward_move_recovers_the_original(){
    let t=PrivateTemp::under(std::path::Path::new("/tmp"));
    let root=HeldDir::trusted_storage(&t.0).unwrap();
    let orig=birth(DirectoryParent::Root,6,"Original");
    let opened=orig.install(&root).unwrap();let entity=opened.directory().entity;
    let occupied=birth(DirectoryParent::Root,7,"Other");
    let someone=occupied.install(&root).unwrap();let foreign=someone.directory().entity;drop(someone);
    let error=match DirectoryRename::hold(&orig,&root,"Original","Other",Some(opened)){
        Ok(_)=>panic!("occupied original name"),Err(e)=>e,
    };
    assert_eq!(error.kind(),io::ErrorKind::AlreadyExists);
    assert_eq!(occupied.open(&root).unwrap().directory().entity,foreign);
    let first=DirectoryRename::hold(&orig,&root,"Original","Moved",Some(orig.open(&root).unwrap())).unwrap();
    let (there,())=first.settle(DirectoryRenameSide::Destination,||Ok(()),||Ok(()),|_|Ok(())).unwrap();
    assert_eq!(there.directory().entity,entity);drop(there);
    let back=DirectoryRename::hold(&orig,&root,"Original","Moved",None).unwrap();
    let (again,())=back.settle(DirectoryRenameSide::Original,||Ok(()),||Ok(()),|_|Ok(())).unwrap();
    assert_eq!(again.directory().entity,entity);
    assert!(!root.path.join("Moved").exists());
    assert_eq!(again.witness,orig.witness());
}

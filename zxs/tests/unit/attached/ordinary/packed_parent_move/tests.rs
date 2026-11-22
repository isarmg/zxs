use super::*;
use crate::{BranchId,EntryId,FileId,HistoryId,PoolId,VolumeId,test_support::PrivateTemp};
use std::{cell::Cell,fs::{self,OpenOptions},io::Write,os::unix::fs::OpenOptionsExt};
#[test]
#[ignore="source only: real transfer pocket and old retained source guard; no service"]
fn original_pocket_and_retirement_target_survive_both_parent_changes(){
    let tmp=PrivateTemp::under(&std::env::temp_dir());
    let root=HeldDir::trusted_storage(&tmp.0).unwrap();
    let old=root.new_child_directory(c"Old").unwrap();
    let new=root.new_child_directory(c"New").unwrap();
    let birth=Birth{pool:PoolId([1;16]),volume:VolumeId([2;16]),operation:OperationId([3;16]),
        entry:EntryId([4;16]),file:FileId([5;16]),history:HistoryId([6;16]),branch:BranchId([7;16]),
        name:"Report".into(),ticket:OperationId([8;16]),slot:1};
    let after_restore=OperationId([9;16]);let name=Birth::placement_parent(after_restore).unwrap();
    let pocket=old.new_child_directory(&name).unwrap();
    let moved=new.checked_child_path(&name).unwrap();
    let terminal=birth.container().unwrap();
    let body=pocket.new_child_directory(&terminal).unwrap();
    let head=body.path.join("FILE");
    let mut f=OpenOptions::new().create_new(true).write(true).mode(0o600).open(&head).unwrap();
    f.write_all(&birth.header()).unwrap();f.sync_all().unwrap();
    fs::hard_link(&head,pocket.checked_child_path(&birth.witness().unwrap()).unwrap()).unwrap();
    let opened=birth.working_records_in(&pocket,&pocket.names().unwrap().into_iter().collect(),&terminal,false,
        |w|w.only_header()).unwrap().with_placement_parent(&old,old.exact_directory(&name).unwrap().unwrap()).unwrap();
    let proof=opened.capture_retirement(&old).unwrap();
    let hold=|from,to,state|PackedParentMove::hold(&birth,after_restore,&old,&new,from,to,state,|w|w.only_header());

    let move_one=hold("Report","report",RenameStage::BeforeReserve).unwrap();
    move_one.require_original(&opened).unwrap();
    let cut=Cell::new(false);
    assert!(move_one.install(|s|{assert_eq!(s,PackedParentStep::Old);Ok(())},|s|{
        assert_eq!(s,PackedParentStep::PocketAtNew);assert!(!pocket.path.exists());
        assert!(moved.join(terminal.to_str().unwrap()).join("FILE").exists());
        cut.set(true);Err(io::ErrorKind::Interrupted.into())
    },|_|->io::Result<()>{panic!("no receipt for an intermediate pocket")}).is_err());
    assert!(cut.get());assert!(opened.require(&old).is_err());

    let resumed=hold("Report","report",RenameStage::Reserved).unwrap();
    assert_eq!(resumed.observed(),PackedParentStep::PocketAtNew);
    let (there,())=resumed.install(|s|{assert_eq!(s,PackedParentStep::PocketAtNew);Ok(())},
        |s|{assert_eq!(s,PackedParentStep::New);Ok(())},|one|{
            assert_eq!(one.dir.entity,body.entity);
            assert_eq!(one.placement_parent.as_ref().unwrap().entity,pocket.entity);
            one.still_retirement_target(&new,&proof)
        }).unwrap();
    assert!(there.header.stamp()==opened.header.stamp());
    drop(there);

    let trace=std::cell::RefCell::new(Vec::new());
    let (prior,())=hold("Report","report",RenameStage::Reserved).unwrap()
        .restore(|_|Ok(()),|s|{trace.borrow_mut().push(s);Ok(())},|one|
            one.still_retirement_target(&old,&proof)
        ).unwrap();
    assert_eq!(*trace.borrow(),[PackedParentStep::PocketAtNew,PackedParentStep::Old]);
    drop(prior);
    let trace=std::cell::RefCell::new(Vec::new());
    let (same_name,())=hold("Report","Report",RenameStage::BeforeReserve).unwrap()
        .install(|_|Ok(()),|s|{trace.borrow_mut().push(s);Ok(())},|one|
            one.still_retirement_target(&new,&proof)
        ).unwrap();
    assert_eq!(*trace.borrow(),[PackedParentStep::New]);
    same_name.require(&new).unwrap();
}

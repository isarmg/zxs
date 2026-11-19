//! Scope-specific original requests are distinct from source authority. Exercise
//! both pure prefix choices and a real reservation through the ordinary MOVE path.
use super::*;
use crate::{DirectoryParent as Parent,ordinary::{file_parent::BirthParent as Anchor,
    custody::{recycle,transfer::Parents}},pool::lease::{Access,LocalPool}};

#[test]
#[ignore="three original filesystems, same-volume sibling directories and a true frozen root MOVE"]
fn only_the_real_source_and_an_empty_exact_parent_match_the_custody_projection(){
    let f=Fixture::build(true,false);
    let a=z::make_directory(&f.sys,op(),Parent::Root(f.from),"left").unwrap();
    let b=z::make_directory(&f.sys,op(),Parent::Root(f.to),"left").unwrap();
    let c=z::make_directory(&f.sys,op(),Parent::Root(f.to),"right").unwrap();
    z::make_directory(&f.sys,op(),Parent::Child(f.to,b.directory),"name").unwrap();
    let native_collision=z::make_directory(&f.sys,op(),Parent::Root(f.to),"native-collision").unwrap();
    let derived=native_leaf("name",f.file.file);
    z::make_directory(&f.sys,op(),Parent::Child(f.to,native_collision.directory),&derived).unwrap();
    // Same spelling as the candidate in a different case is not a collision.
    z::make_directory(&f.sys,op(),Parent::Child(f.to,c.directory),"Name").unwrap();
    let r=f.cmd();stop_at(&f,r,Cut::Reservation);
    {
        let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let a0=s.moving.as_ref().unwrap();
        let original=&a0.original;let prefix=s.frontier();
        let lookup=|id|s.file_parent_at(id,original.uid,&p.journal_registration()).unwrap();
        let pa=lookup(Parent::Child(f.from,a.directory));let left=lookup(Parent::Child(f.to,b.directory));
        let right=lookup(Parent::Child(f.to,c.directory));
        let route=|source,target|{let mut m=original.clone();m.parents=Parents::Move{source,target};m};
        assert!(s.custody_parent_projection(&p,original).is_ok());
        assert!(s.custody_parent_projection(&p,&route(Anchor::Root,left.clone())).is_err()); // real directory occupies B/left/name
        assert!(s.custody_parent_projection(&p,&route(pa,right.clone())).is_err()); // the original FILE is not A/left/name
        let clash=lookup(Parent::Child(f.to,native_collision.directory));
        assert!(s.custody_parent_projection(&p,&route(Anchor::Root,clash)).is_err()); // no business "name", but a directory owns its exact *physical* leaf
        let good=route(Anchor::Root,right.clone());assert!(s.custody_parent_projection(&p,&good).is_ok());
        assert_ne!(good.request_digest(p.id()).unwrap(),a0.request); // existing root ticket cannot be repurposed
        let mut wrong=left;
        if let Anchor::Child{directory,..}=&mut wrong{*directory=c.directory}else{unreachable!();}
        assert!(s.custody_parent_projection(&p,&route(Anchor::Root,wrong)).is_err()); // says right, presents left's native proof
        let(_,now)=p.snapshot().unwrap();assert_eq!(now.frontier(),prefix);assert_eq!(now.moving.as_ref().unwrap().original,*original);
    }
    assert_eq!(f.call(r).unwrap().status,MoveStatus::CompleteCheckedNow);
}

#[test]
#[ignore="three original filesystems; original retained key and exact restore-parent path boundary"]
fn restoration_parent_geometry_and_original_deleted_parent_do_not_consume_the_retained_credit(){
    for (len,ok) in [(4049usize,true),(4050,false)]{
        // '/x' is two bytes. B itself still admits all root protocol private names.
        let f=Fixture::planned(true,false,None,Some(len-2));
        let dir=z::make_directory(&f.sys,op(),Parent::Root(f.to),"x").unwrap();
        let a=z::make_directory(&f.sys,op(),Parent::Root(f.from),"x").unwrap();
        let deletion=z::apply_recycle(&f.sys,op(),0,z::RecycleAction::Delete{file:f.file.file,from:f.from}).unwrap();
        let key=deletion.deletion.unwrap();let orig=z::RecycleAction::Restore{deleted:key,target:z::RestoreTarget::Volume(f.to)};
        // This authentic *root* reservation supplies the original source
        // registration/roles; a descendant's proposed request must not replace it.
        let restore=op();let mut stopped=false;
        assert!(recycle::apply(&f.sys,restore,1,orig,&mut |cut|{
            if cut==Cut::Reservation{stopped=true;return Err(io::Error::other("leave a true original restore reservation"));}Ok(())
        }).is_err());assert!(stopped);
        {
            let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let a0=s.moving.as_ref().unwrap();
            let mut candidate=a0.original.clone();
            let addr=Parent::Child(f.to,dir.directory);let target=s.file_parent_at(addr,candidate.uid,&p.journal_registration()).unwrap();
            candidate.parents=Parents::Restore{deleted_from:Parent::Root(f.from),target:target.clone()};
            assert_ne!(candidate.request_digest(p.id()).unwrap(),a0.request);
            assert_eq!(s.custody_parent_projection(&p,&candidate).is_ok(),ok);
            candidate.parents=Parents::Restore{deleted_from:Parent::Child(f.from,a.directory),target};
            assert!(s.custody_parent_projection(&p,&candidate).is_err()); // known same-spelled directory, wrong deletion
            assert_eq!(s.placement(f.file.file).unwrap().retained_from().unwrap(),Some(Parent::Root(f.from)));
            let(_,again)=p.snapshot().unwrap();assert_eq!(again.frontier(),s.frontier());
            assert_eq!(again.moving.as_ref().unwrap().bytes,a0.bytes);
            assert_eq!(again.placement(f.file.file).unwrap().retained().unwrap().decision,key.decision);
        }
        assert_eq!(z::apply_recycle(&f.sys,restore,1,orig).unwrap().status,MoveStatus::CompleteCheckedNow);
        f.all_history();
    }
}

#[test]
#[ignore="dedicated original recycler and true installed ordinary child parents; no nested publication"]
fn restore_preparation_opens_only_its_exact_child_and_does_not_borrow_a_sibling_or_the_deleted_parent(){
    use crate::ordinary::custody::{admission::{LifecycleGrant,Side},incoming::IncomingSelection,source::Clean,
        purpose::Call,transfer::ParentChoice};
    let f=Fixture::build(false,false);let key=z::apply_recycle(&f.sys,op(),0,z::RecycleAction::Delete{file:f.file.file,from:f.from}).unwrap().deletion.unwrap();
    let bin={let(p,_,_)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();p.recycle_for(f.from).unwrap()};
    let left=z::make_directory(&f.sys,op(),Parent::Root(f.to),"left").unwrap();
    let right=z::make_directory(&f.sys,op(),Parent::Root(f.to),"right").unwrap();
    let left=Parent::Child(f.to,left.directory);let right=Parent::Child(f.to,right.directory);
    z::make_directory(&f.sys,op(),Parent::Root(f.to),"name").unwrap();
    z::make_directory(&f.sys,op(),left,"name").unwrap();
    // Only the root and left have an exact business collision; right has a
    // case-distinct directory and can select the original native target.
    z::make_directory(&f.sys,op(),right,"Name").unwrap();
    let r=Request{parents:ParentChoice::Restore{deleted_from:Parent::Root(f.from),target:right},
        op:op(),call:Call::Restore{deleted_by:key.operation,decision:key.decision},id:f.file.file,from:bin,to:f.to,epoch:1};
    let old=f.old.with_file_name("detached-deletion-site");fs::rename(&f.old,&old).unwrap();
    {
        let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Incoming).unwrap();
        let bad=Request{parents:ParentChoice::Restore{deleted_from:Parent::Root(f.from),target:left},..r};
        let root=Request{parents:ParentChoice::Roots,..r};
        assert_ne!(bad.digest(&p).unwrap(),r.digest(&p).unwrap());assert_ne!(root.digest(&p).unwrap(),r.digest(&p).unwrap());
        assert!(LifecycleGrant::capture(&p,bad).is_err()&&LifecycleGrant::capture(&p,root).is_err());
        let g=LifecycleGrant::capture(&p,r).unwrap();let end=p.custody_end(&g,Side::Target).unwrap();
        assert_eq!(end.role(),crate::Role::File);assert!(!end.target_present().unwrap());assert!(s.decided(r.op).is_none());
    }
    let source=Clean::capture_lifecycle(&f.sys,r).unwrap().verify().unwrap();
    let prepared=IncomingSelection::prepare(&f.sys,&source,r).unwrap();assert_eq!(prepared.inspection().unwrap().len(),4);
    let original=f.new.join("right");let parked=f.new.with_file_name("parked-real-parent");
    fs::rename(&original,&parked).unwrap();fs::create_dir(&original).unwrap();
    fs::set_permissions(&original,fs::metadata(&parked).unwrap().permissions()).unwrap();
    assert!(prepared.recheck(&f.sys).is_err());assert!(fs::read_dir(&original).unwrap().next().is_none());
    let untouched=f.new.with_file_name("untouched-foreign-parent");fs::rename(&original,&untouched).unwrap();
    fs::rename(&parked,&original).unwrap();prepared.recheck(&f.sys).unwrap();
    let(_,_,end)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();assert!(end.moving.is_none()&&end.decided(r.op).is_none());
    assert_eq!(end.placement(f.file.file).unwrap().retained().unwrap().operation,key.operation);
    assert!(fs::read_dir(untouched).unwrap().next().is_none());assert!(f.new.join("right/Name").is_dir());assert!(!f.old.exists());
}

#[test]
#[ignore="two real ordinary parents, original scoped reservation/decision versus root-only replay"]
fn a_scoped_move_cannot_issue_or_replay_a_root_move_with_the_same_op(){
    use crate::ordinary::custody::transfer::ParentChoice;
    let f=Fixture::build(false,false);let from=Parent::Root(f.from);let root=Parent::Root(f.to);
    let child=Parent::Child(f.to,z::make_directory(&f.sys,op(),root,"right").unwrap().directory);
    let wrong=Parent::Child(f.to,z::make_directory(&f.sys,op(),root,"left").unwrap().directory);
    let ordinary=f.cmd();let scoped=Request{parents:ParentChoice::Move{source:from,target:child},..ordinary};
    let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let frontier=s.frontier();
    assert_ne!(ordinary.digest(&p).unwrap(),scoped.digest(&p).unwrap());drop(p);
    let mut stopped=false;assert!(perform(&f.sys,scoped,&mut |point|{if point==Cut::Reservation{stopped=true;
        return Err(io::Error::other("original scoped reservation, no namespace change"));}Ok(())}).is_err());assert!(stopped);
    let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();assert_eq!(s.frontier(),frontier);
    let a=s.moving.as_ref().unwrap();assert_eq!(a.original.parent_choice(),scoped.parents);assert!(s.pending.is_none());
    assert!(ordinary.observe(&p,&s).is_err());assert_eq!(scoped.observe(&p,&s).unwrap().unwrap().status,MoveStatus::Reserved);drop(p);
    assert!(f.call(ordinary).is_err());assert!(z::move_file_at(&f.sys,ordinary.op,ordinary.id,from,wrong,0).is_err());f.all_history();
    assert_eq!(z::move_file_at(&f.sys,ordinary.op,ordinary.id,from,child,0).unwrap().status,MoveStatus::CompleteCheckedNow);
    assert_eq!(z::move_receipt_at(&f.sys,ordinary.op,ordinary.id,from,child,0).unwrap().unwrap().destination_parent,Some(child));
    assert!(z::move_receipt(&f.sys,ordinary.op,ordinary.id,ordinary.from,ordinary.to,0).is_err());
    assert!(z::move_receipt_at(&f.sys,ordinary.op,ordinary.id,from,wrong,0).is_err());
    assert_eq!(fs::read_dir(f.new.join("left")).unwrap().count(),0);assert_eq!(fs::read_dir(f.new.join("right")).unwrap().count(),1);
    f.all_history();
}

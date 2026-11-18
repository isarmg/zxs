// Test-source fixtures only. All paths are isolated test roots; no production
// directory is scanned or borrowed by any of these scenarios.
use std::io::Read;
use crate::pool::lease::{Access,LocalPool};

fn seed(f:&Fixture)->(FileInfo,FormalNode,Request){
    let item=z::create_empty(&f.sys,op(),f.from,"one").unwrap();
    z::save_first(&f.sys,op(),item.file,claim(BEFORE),Cursor::new(BEFORE)).unwrap();
    let first=z::commit_working(&f.sys,op(),item.file,1).unwrap();
    let r=Request{call:crate::ordinary::custody::purpose::Call::Move,op:op(),id:item.file,from:f.from,to:f.to,epoch:0};
    (item,first,r)
}
fn edit(at:u64,b:&[u8])->z::SpanEdit{z::SpanEdit::new(at,claim(b),z::LengthPolicy::KeepOrGrow).unwrap()}
fn node(f:&Fixture,file:crate::FileId,n:&FormalNode,wanted:&[u8]){
    let mut got=Vec::new();assert_eq!(z::read_node_to(&f.sys,file,n.node,&mut got).unwrap(),*n);assert_eq!(got,wanted)
}
fn frozen(f:&Fixture,r:Request,first:&FormalNode)->(u64,[u8;32]){
    let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let m=s.placed(r.id).unwrap();
    let(c,p,old_increment)=super::super::finish::frozen(&s,m).unwrap();
    assert!(old_increment.is_none()&&m.original.last.is_none()&&m.target.incr.is_none());
    assert_eq!((c.create.file,p.original.node),(r.id,first.node));assert_eq!(m.original.role_count(),3);
    (m.seq,m.digest)
}
fn round_trip(cross:bool){
    let f=Fixture::build(cross,false);let(item,first,r)=seed(&f);
    let original_ids=inventory_ids(&f.old.join("one"));assert_eq!(original_ids.len(),3);
    assert_eq!(f.call(r).unwrap().status,MoveStatus::CompleteCheckedNow);
    let dest=f.new.join("one");let three=inventory_ids(&dest);assert_eq!(three.len(),3);
    let fixed=frozen(&f,r,&first);
    if cross{for(name,id)in &original_ids{assert_ne!(id,&three[name]);}}else{assert_eq!(three,original_ids);}
    // A new object at the original business name must not acquire either
    // the old FILE's history or the right to supply a missing B material.
    let other=z::create_empty(&f.sys,op(),f.from,"one").unwrap();assert_ne!(item.file,other.file);
    assert_ne!(item.entry,other.entry);assert_ne!(item.history,other.history);
    let other_ids=inventory_ids(&f.old.join("one"));
    let away=f.old.with_file_name("offline-owner");fs::rename(&f.old,&away).unwrap();
    let patch=vec![b'A';4096];let w1=op();let e=edit(0,&patch);
    let a=z::save_interval(&f.sys,w1,r.id,1,e,Cursor::new(&patch)).unwrap();
    assert_eq!((a.base_node,a.base_version,a.source_revision,a.revision),(first.node,first.version,1,2));
    let w2=op();let b=z::save_interval(&f.sys,w2,r.id,2,edit(1,b"Z"),Cursor::new(b"Z")).unwrap();
    assert_eq!(b.revision,3);let mut expect=patch.clone();expect[1]=b'Z';
    let mut out=Vec::new();let now=z::read_current_range_to(&f.sys,r.id,0,4096,&mut out).unwrap();
    assert_eq!(now.file.volume,f.to);assert_eq!(out,expect);assert_eq!(now.edit,Some(b.clone()));
    node(&f,r.id,&first,BEFORE);assert_eq!(frozen(&f,r,&first),fixed);
    assert_eq!(z::list_initial_files(&f.sys,f.to).unwrap().files[0].file,r.id);
    assert_eq!(inventory_ids(&dest).len(),5);
    for(n,v)in &three{assert_eq!(&inventory_ids(&dest)[n],v);}
    {
        let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let site=s.placement(r.id).unwrap();
        let initial=&s.intervals(r.id).next().unwrap().original;
        assert_eq!(initial.volume,f.to);let parent=s.head(r.id).unwrap();assert!(initial.matches(site,parent,None));
        let mut false_source=initial.clone();false_source.volume=f.from;
        assert!(!false_source.matches(site,parent,None));assert_eq!(site.birth().create.volume,f.from);
    }
    // Holding the actual current FILE lease blocks work retirement even
    // though B's native child is already decided. The old move still has
    // precisely three roles; only the later B decisions authorize the rest.
    struct During<'a>{f:&'a Fixture,r:Request,first:&'a FormalNode,op:OperationId,entered:bool,data:Vec<u8>}
    impl Write for During<'_>{
        fn write(&mut self,b:&[u8])->io::Result<usize>{
            if !self.entered{
                self.entered=true;
                assert!(z::commit_working(&self.f.sys,self.op,self.r.id,3).is_err());
                let obs=z::formal_receipt(&self.f.sys,self.op)?.unwrap();
                assert_eq!(obs.status,z::CreationStatus::PublishedInstallationUnconfirmed);
                let child=obs.published.unwrap();assert_eq!(child.working_revision,3);
                assert_eq!(z::list_initial_files(&self.f.sys,self.f.to)?.files[0].file,self.r.id);
                assert_eq!(inventory_ids(&self.f.new.join("one")).len(),6);
                node(self.f,self.r.id,self.first,BEFORE);frozen(self.f,self.r,self.first);
                let rogue=self.f.new.join("one/unsigned");fs::write(&rogue,b"not issued")?;
                assert!(z::list_initial_files(&self.f.sys,self.f.to).is_err());
                assert_eq!(fs::read(&rogue)?,b"not issued");fs::remove_file(rogue)?;
            }
            self.data.extend_from_slice(b);Ok(b.len())
        }
        fn flush(&mut self)->io::Result<()>{Ok(())}
    }
    let commit=op();let mut sink=During{f:&f,r,first:&first,op:commit,entered:false,data:Vec::new()};
    z::read_current_to(&f.sys,r.id,&mut sink).unwrap();assert!(sink.entered);assert_eq!(sink.data,expect);
    let child=z::commit_working(&f.sys,commit,r.id,3).unwrap();assert_eq!((child.file,child.history,child.branch),(item.file,item.history,first.branch));
    assert_eq!(child.kind,z::FormalStorage::Incremental{parent:first.node,parent_version:first.version,changed:claim(&expect)});
    assert_eq!(child.body,claim(&expect));assert_ne!(child.node,first.node);
    assert_eq!(inventory_ids(&dest).len(),4);node(&f,r.id,&first,BEFORE);node(&f,r.id,&child,&expect);
    assert_eq!(z::formal_receipt(&f.sys,commit).unwrap().unwrap().status,z::CreationStatus::Published);
    assert_eq!(z::current_info(&f.sys,r.id).unwrap().head,Some(child.clone()));
    assert_eq!(frozen(&f,r,&first),fixed);assert_eq!(f.receipt(r).decision_sequence,Some(fixed.0));
    assert_eq!(f.call(r).unwrap().status,MoveStatus::OriginalPublished);
    fs::rename(&away,&f.old).unwrap();assert_eq!(inventory_ids(&f.old.join("one")),other_ids);
    let wrong=f.new.with_file_name("unavailable-destination");fs::rename(&f.new,&wrong).unwrap();
    let mut empty=Vec::new();assert!(z::read_current_to(&f.sys,r.id,&mut empty).is_err());assert!(empty.is_empty());
    assert!(z::read_node_to(&f.sys,r.id,first.node,&mut empty).is_err());assert!(empty.is_empty());
    assert_eq!(z::current_info(&f.sys,other.file).unwrap().file,other);
    assert_eq!(z::save_interval(&f.sys,w1,r.id,1,e,io::empty()).unwrap(),a);
    assert_eq!(z::commit_working(&f.sys,commit,r.id,3).unwrap(),child);
    assert_eq!(z::commit_working(&f.sys,first.created_by,r.id,1).unwrap(),first);
    assert_eq!(f.call(r).unwrap().status,MoveStatus::OriginalPublished);
}
#[test]
#[ignore="two private filesystems; real same-mount rename and post-MOVE FILE writes"]
fn same_mount_clean_single_full_keeps_parent_and_accepts_native_b_saves_and_first_increment(){round_trip(false);}
#[test]
#[ignore="three private filesystems; genuine cross-filesystem material and offline original root"]
fn cross_mount_clean_single_full_keeps_parent_and_accepts_native_b_saves_and_first_increment(){round_trip(true);}

#[test]
#[ignore="three private filesystems; exactly three old roles and a verified empty source"]
fn after_opened_a_single_full_reader_blocks_source_release_and_b_writes_but_not_b_reads(){
    let f=Fixture::build(true,false);let(_,first,r)=seed(&f);
    let writer=op();
    struct Old<'a>{f:&'a Fixture,r:Request,n:&'a FormalNode,w:OperationId,hit:bool}
    impl Write for Old<'_>{
        fn write(&mut self,b:&[u8])->io::Result<usize>{
            if !self.hit{self.hit=true;
                assert!(self.f.call(self.r).is_err());assert_eq!(self.f.receipt(self.r).status,MoveStatus::TargetReopenedSourcePending);
                assert_eq!(inventory_ids(&self.f.old.join("one")).len(),3);
                node(self.f,self.r.id,self.n,BEFORE);
                assert!(z::save_interval(&self.f.sys,self.w,self.r.id,1,edit(0,b"K"),Cursor::new(b"K")).is_err());
                assert!(z::interval_receipt(&self.f.sys,self.w)?.is_none());
            }Ok(b.len())
        }
        fn flush(&mut self)->io::Result<()>{Ok(())}
    }
    let mut old=Old{f:&f,r,n:&first,w:writer,hit:false};
    z::read_node_to(&f.sys,r.id,first.node,&mut old).unwrap();assert!(old.hit);
    for left in [2,1,0]{
        stop_at(&f,r,Cut::MoveLeafRetired);
        let held=fs::read_dir(&f.old).unwrap().map(|e|e.unwrap().path()).find(|n|n.file_name().unwrap().to_string_lossy().starts_with(".zxs-move-out-")).unwrap();
        assert_eq!(inventory_ids(&held).len(),left);
    }
    stop_at(&f,r,Cut::MoveEmptyConfirmed);assert_eq!(f.call(r).unwrap().status,MoveStatus::CompleteCheckedNow);
    z::save_interval(&f.sys,writer,r.id,1,edit(0,b"K"),Cursor::new(b"K")).unwrap();node(&f,r.id,&first,BEFORE);
}

#[test]
#[ignore="two private filesystems; true old per-FILE write lease vs same-mount relocation"]
fn an_inflight_original_a_write_cannot_be_redirected_to_its_same_inode_on_b(){
    let f=Fixture::build(false,false);let(_,first,r)=seed(&f);
    struct Overlap<'a>{f:&'a Fixture,r:Request,seen:bool}
    impl Read for Overlap<'_>{
        fn read(&mut self,buf:&mut[u8])->io::Result<usize>{
            if self.seen||buf.is_empty(){return Ok(0);}
            self.seen=true;assert!(self.f.call(self.r).is_err());
            assert!(z::move_receipt(&self.f.sys,self.r.op,self.r.id,self.r.from,self.r.to,0)?.is_none());
            buf[0]=b'Y';Ok(1)
        }
    }
    let mut src=Overlap{f:&f,r,seen:false};let op1=op();
    z::save_interval(&f.sys,op1,r.id,1,edit(0,b"Y"),&mut src).unwrap();assert!(src.seen);
    assert!(f.call(r).is_err());assert!(!f.new.join("one").exists());
    node(&f,r.id,&first,BEFORE);assert_eq!(z::current_info(&f.sys,r.id).unwrap().file.volume,f.from);
}

#[test]
#[ignore="three private registered test filesystems; validates existing source and history, not a recycle transaction"]
fn a_later_handoff_source_uses_the_actual_b_roles_and_never_adds_a_new_b_child_to_the_old_move(){
    use crate::ordinary::custody::purpose::{DeletedAt,Purpose};
    let f=Fixture::build(true,false);let(item,first,r)=seed(&f);
    let _=f.call(r).unwrap();let patch=b"B-local";
    z::save_interval(&f.sys,op(),item.file,1,edit(0,patch),Cursor::new(patch)).unwrap();
    let child=z::commit_working(&f.sys,op(),item.file,2).unwrap();
    let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
    let c=s.file(item.file).unwrap();let full=s.head(item.file).unwrap();let d=s.delta(item.file).unwrap();
    assert_eq!((full.original.node,d.original.node),(first.node,child.node));
    let m=s.placed(item.file).unwrap();let before=s.before_handoff(m).unwrap();assert_eq!((before.volume(),before.epoch()),(f.from,0));
    let frozen=m.require_at(before,full,None).unwrap();assert!(frozen.incr.is_none());assert_eq!(frozen.directory,c.create.directory);
    let b=s.placement(item.file).unwrap();let sites=b.sites(full,Some(d)).unwrap();
    assert_eq!((b.volume(),b.epoch(),b.key()),(f.to,1,m.digest));
    assert_eq!(sites.directory,m.target.directory);assert_ne!(sites.directory,c.create.directory);
    assert_eq!(sites.incr.as_ref(),Some(&d.material.object));assert!(m.target.incr.is_none());
    // The next possible logical origin is inspected, not reserved. The
    // current B-native child is not the A-born child of the historic MOVE.
    let recycle=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap().id;
    let checked=p.observe_dedicated(crate::pool::lease::recycle::Direction::ToRecycle,f.to,recycle).unwrap();
    let (_,to)=checked.registration().originals();let target=crate::ordinary::wire::NativeObject{entity:to.1,birth:to.2.clone()};
    let volume=p.ordinary(f.to).unwrap();let dir=volume.root.child_directory(c"one").unwrap().unwrap();
    let observed=super::super::source::Clean::at_original_sites(&volume,dir,c,full,Some(d),&sites).unwrap().verify().unwrap();
    let mut proposed=m.original.clone();proposed.op=op();proposed.ticket=op();proposed.last=Some(d.digest);
    proposed.source_material=observed.digest().unwrap();proposed.source_bytes=observed.physical_bytes();
    proposed.from=f.to;proposed.src_root=m.original.dst_root.clone();proposed.to=recycle;proposed.dst_root=target;proposed.epoch=1;
    proposed.purpose=Purpose::Delete{previous:m.digest,at:DeletedAt::new(55,0).unwrap()};
    b.require_next(&proposed).unwrap();
    proposed.purpose=Purpose::Delete{previous:c.digest,at:DeletedAt::new(55,0).unwrap()};
    assert!(b.require_next(&proposed).is_err());assert!(before.require_next(&proposed).is_err());
    let mut foreign=d.clone();foreign.original.volume=f.from;assert!(b.sites(full,Some(&foreign)).is_err());
    let mut forged=m.clone();forged.previous=[0;32];assert!(s.before_handoff(&forged).is_err());
    // Ordinary A can be offline: all cutoff/role projections are original
    // SYSTEM facts; they have not vouched for any online R or structural I/O.
    drop(observed);drop(volume);drop(checked);drop(s);drop(p);let off=f.old.with_file_name("offline-after-prior");fs::rename(&f.old,&off).unwrap();
    let(_,_,cold)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
    let got=cold.placement(item.file).unwrap().sites(cold.head(item.file).unwrap(),cold.delta(item.file)).unwrap();
    assert_eq!(got,sites);
}


#[test]
#[ignore="private registered roots; actual original reservation, intent and lower-bound decision"]
fn before_handoff_does_not_promote_the_reservation_a_sealed_frame_or_an_unconfirmed_complete_tail(){
    let f=Fixture::build(true,false);let r=f.cmd();
    stop_at(&f,r,Cut::BeforeIntent);let(p,_,reserved)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();drop(p);
    assert!(reserved.moving.is_some()&&reserved.pending.is_none()&&reserved.decided(r.op).is_none());
    stop_at(&f,r,Cut::Intent);let(p,_,sealed)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();drop(p);
    let m=sealed.pending.as_ref().unwrap().frame.moved().unwrap().clone();
    let mut swapped=m.clone();std::mem::swap(&mut swapped.target.raw,&mut swapped.target.node);
    let mut wrong=m.clone();wrong.digest[0]^=1;
    for view in [&reserved,&sealed]{for offered in [&m,&swapped,&wrong]{assert!(view.before_handoff(offered).is_err());}}
    stop_at(&f,r,Cut::FrameSynced);let(p,_,unconfirmed)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();drop(p);
    assert!(unconfirmed.decided(r.op).is_none()&&unconfirmed.tail==m.bytes.len());
    assert!(unconfirmed.before_handoff(&m).is_err());
    stop_at(&f,r,Cut::Decision);let(_,_,confirmed)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
    assert_eq!(confirmed.before_handoff(&m).unwrap().volume(),f.from);
    assert!(confirmed.before_handoff(&swapped).is_err());assert!(confirmed.before_handoff(&wrong).is_err());
    assert_eq!(f.call(r).unwrap().status,MoveStatus::CompleteCheckedNow);
}

#[test]
#[ignore="dedicated recycle filesystem and two registered ordinary roots; lifecycle observation only"]
fn only_the_original_local_subject_and_clean_file_can_borrow_the_two_physical_endpoints(){
    use crate::ordinary::custody::{admission::{LifecycleGrant,Side},purpose::Call};
    let f=Fixture::build(false,false);let rr=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|x|x.volume.role==z::Role::Recycle).unwrap();
    let request=Request{op:op(),id:f.file.file,from:f.from,to:rr.id,epoch:0,call:Call::Delete};
    let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
    let grant=LifecycleGrant::capture(&p,request).unwrap();
    assert_eq!(grant.prior(),s.file(request.id).unwrap().digest);
    assert_eq!(grant.name(Side::Source).to_bytes(),b"name");
    assert!(grant.name(Side::Target).to_bytes().starts_with(b".zxs-retained-"));
    let orig=p.custody_end(&grant,Side::Source).unwrap();let bin=p.custody_end(&grant,Side::Target).unwrap();
    assert_eq!((orig.id(),orig.role(),bin.id(),bin.role()),(f.from,crate::Role::File,rr.id,crate::Role::Recycle));
    assert_eq!(bin.identity().unwrap(),(grant.root(Side::Target).entity,grant.root(Side::Target).birth.clone()));
    assert!(grant.sites().incr.is_some());assert!(orig.target_present().is_err());
    assert!(bin.read_clean_source().is_err());assert!(orig.read_clean_source().is_err()); // read-only observation, not incoming.
    for bad in [
        Request{call:Call::Move,..request},Request{op:f.last.created_by,..request},
        Request{to:f.to,..request},Request{from:f.to,..request},Request{epoch:1,..request},
        Request{from:rr.id,to:f.to,epoch:1,call:Call::Restore{deleted_by:request.op,decision:[31;32]},..request},
    ]{assert!(LifecycleGrant::capture(&p,bad).is_err());}
    let other_cut=LocalPool::open(&f.sys,Access::Read).unwrap();
    assert!(other_cut.custody_end(&grant,Side::Target).is_err());assert!(p.ordinary(rr.id).is_err());
    assert!(!bin.target_present().unwrap());assert!(s.decided(request.op).is_none());assert!(s.moving.is_none());
    drop(orig);drop(bin);drop(other_cut);drop(grant);drop(s);drop(p);
    assert!(f.old.join("name").is_dir());assert!(fs::read_dir(&rr.volume.path).unwrap().next().is_none());
    let new=z::create_empty(&f.sys,request.op,f.to,"same-old-op-still-unused").unwrap();assert_ne!(new.file,request.id);
    let(p,_,_)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();assert!(LifecycleGrant::capture(&p,request).is_err());
}

#[test]
#[ignore="three private registered test filesystems; verifies source streams only, not a delete"]
fn soft_delete_preparation_reads_the_true_b_container_after_a_disconnect_and_refuses_changed_moved_history(){
    use crate::ordinary::custody::{purpose::Call,source::{Clean,Leaf}};
    let f=Fixture::build(true,false);let(item,first,r)=seed(&f);f.call(r).unwrap();
    let bytes=b"new native child of B";z::save_interval(&f.sys,op(),item.file,1,edit(0,bytes),Cursor::new(bytes)).unwrap();
    let second=z::commit_working(&f.sys,op(),item.file,2).unwrap();
    let recycle=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap();
    let req=Request{call:Call::Delete,op:op(),id:item.file,from:f.to,to:recycle.id,epoch:1};
    let offline=f.old.with_file_name("offline-A");fs::rename(&f.old,&offline).unwrap();
    {
        let held=Clean::capture_lifecycle(&f.sys,req).unwrap().verify().unwrap();
        assert_eq!((held.file(),held.volume(),held.head()),(item.file,f.to,second.node));
        assert_eq!(held.objects().len(),4);
        let mut raw=Vec::new();held.stream(Leaf::FullRaw,|_,v|{raw.extend_from_slice(v);Ok(())}).unwrap();assert_eq!(raw,BEFORE);
        let pack=held.objects().iter().find(|part|part.kind==Leaf::Incr).unwrap();
        let bmeta=fs::metadata(f.new.join("one").join(format!("{}-one",second.storage))).unwrap();
        assert_eq!((pack.original.entity.device,pack.original.entity.inode),(bmeta.dev(),bmeta.ino()));
    }
    assert!(Clean::capture_lifecycle(&f.sys,Request{from:f.from,..req}).is_err());
    let parent=f.new.join("one").join(format!("{}-one",first.storage));let mut corrupt=BEFORE.to_vec();corrupt[0]^=31;
    fs::write(&parent,&corrupt).unwrap();assert!(Clean::capture_lifecycle(&f.sys,req).unwrap().verify().is_err());
    assert_eq!(fs::read(&parent).unwrap(),corrupt);
    assert!(fs::read_dir(&recycle.volume.path).unwrap().next().is_none());
    let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();assert!(s.decided(req.op).is_none()&&s.moving.is_none());
}

#[test]
#[ignore="three private filesystems; actual original stage and INTENT at separate cut points"]
fn a_custody_loan_compares_decoded_phase_and_the_seal_incarnation_not_only_a_frame_hash_or_stat(){
    use crate::ordinary::custody::admission::Stage;
    let f=Fixture::build(true,false);let r=f.cmd();
    stop_at(&f,r,Cut::Reservation);let(p,_,mut early)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();drop(p);
    stop_at(&f,r,Cut::MoveLeafLinked);let(p,_,mut bound)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();drop(p);
    assert!(early.moving.as_ref().unwrap().stage().is_none()&&bound.moving.as_ref().unwrap().stage().is_some());
    // Equalize the two observed stamps only in these borrowed test values:
    // changes to the original phase still distinguish the two cuts.
    early.moving.as_mut().unwrap().stamp=bound.moving.as_ref().unwrap().stamp.clone();
    assert!(Stage::of(&early).unwrap()!=Stage::of(&bound).unwrap());
    stop_at(&f,r,Cut::Intent);let(p,_,mut sealed)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();drop(p);
    let real=Stage::of(&sealed).unwrap();
    let i=sealed.pending.as_mut().unwrap();let digest=i.frame.moved().unwrap().digest;
    i.stamp=bound.moving.take().unwrap().stamp;
    assert_eq!(i.frame.moved().unwrap().digest,digest);assert!(Stage::of(&sealed).unwrap()!=real);
    assert_eq!(f.call(r).unwrap().status,MoveStatus::CompleteCheckedNow);
}

#[test]
#[ignore="private original ordinary and dedicated recycle filesystems; unnamed material only"]
fn fresh_dedicated_copy_reads_back_all_true_anonymous_originals_before_any_ticket_or_visible_deletion(){
    use std::os::unix::ffi::OsStrExt;
    use crate::ordinary::custody::{admission::{LifecycleGrant,Side},incoming::IncomingSelection,purpose::Call,source::Clean};
    let f=Fixture::build(true,false);let rr=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap();
    let r=Request{call:Call::Delete,op:op(),id:f.file.file,from:f.from,to:rr.id,epoch:0};
    let held=Clean::capture_lifecycle(&f.sys,r).unwrap().verify().unwrap();
    let copy=IncomingSelection::prepare(&f.sys,&held,r).unwrap();let target=copy.inspection().unwrap();
    assert_eq!(target.len(),4);
    for (a,b) in target.iter().zip(held.objects()){
        assert_ne!(a.0,b.original.entity);assert_eq!(a.0.device,rr.volume.entity.device);
        assert_ne!(a.1.filesystem(),b.original.birth.filesystem());assert_eq!((a.2,a.3),(b.len,b.digest));
    }
    let(_,private)=r.call.leaves(r.id,r.op,&f.file.name).unwrap();
    assert!(fs::read_dir(&rr.volume.path).unwrap().next().is_none());
    let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
    let g=LifecycleGrant::capture(&p,r).unwrap();
    assert!(!p.custody_end(&g,Side::Target).unwrap().target_present().unwrap());
    assert!(s.moving.is_none()&&s.decided(r.op).is_none());drop(g);drop(s);drop(p);
    assert!(IncomingSelection::prepare(&f.sys,&held,Request{op:op(),..r}).is_err());
    // Occupancy or loss of the exact dedicated root leaves the original FILE
    // active, never removes an unknown item and doesn't create a SYSTEM ticket.
    let foreign=rr.volume.path.join("foreign");fs::write(&foreign,b"keep").unwrap();
    assert!(copy.recheck(&f.sys).is_err());assert!(IncomingSelection::prepare(&f.sys,&held,r).is_err());
    assert_eq!(fs::read(&foreign).unwrap(),b"keep");fs::remove_file(&foreign).unwrap();
    let at=rr.volume.path.join(std::ffi::OsStr::from_bytes(private.to_bytes()));fs::create_dir(&at).unwrap();
    assert!(copy.recheck(&f.sys).is_err());assert!(at.is_dir());fs::remove_dir(&at).unwrap();
    let lost=rr.volume.path.with_file_name("missing-selected-R");fs::rename(&rr.volume.path,&lost).unwrap();
    assert!(copy.recheck(&f.sys).is_err());fs::rename(lost,&rr.volume.path).unwrap();
    copy.recheck(&f.sys).unwrap();drop(copy);drop(held);f.all_history();
    let(_,_,now)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
    assert!(now.decided(r.op).is_none()&&now.moving.is_none());assert!(fs::read_dir(&rr.volume.path).unwrap().next().is_none());
}

#[test]
#[ignore="registered original FILE and independent recycle; exact writer proposal only"]
fn only_its_live_verified_prepared_target_proposes_registered_native_custody_at_the_actual_prefix(){
    use crate::ordinary::custody::{incoming::{IncomingSelection,proposed},purpose::{Call,Purpose},source::Clean,transfer::Origin};
    let f=Fixture::build(true,false);let rr=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap();
    let r=Request{op:op(),id:f.file.file,epoch:0,call:Call::Delete,from:f.from,to:rr.id};
    let original=Clean::capture_lifecycle(&f.sys,r).unwrap().verify().unwrap();
    let copy=IncomingSelection::prepare(&f.sys,&original,r).unwrap();
    let proposal=copy.with_writer(&f.sys,|turn|proposed(&turn)).unwrap();
    assert_eq!(proposal.source_material,original.digest().unwrap());assert_eq!(proposal.source_bytes,original.physical_bytes());
    let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let reg=p.journal_registration();
    let ok=|o:&Origin|s.check_new_lifecycle_origin(&reg,o);
    assert!(ok(&proposal).is_ok());assert_eq!(proposal.purpose.call(),Call::Delete);
    let mut bad=proposal.clone();bad.to=f.to;bad.dst_root={
        let v=p.ordinary(f.to).unwrap();crate::ordinary::wire::NativeObject{entity:v.root.entity,birth:crate::fs::Birth::read(&v.root.file).unwrap()}
    };assert!(ok(&bad).is_err()); // actual ordinary target is not the original dedicated R
    let mut bad=proposal.clone();bad.purpose=Purpose::Restore{deleted_by:op(),previous:proposal.creation};assert!(ok(&bad).is_err());
    let mut bad=proposal.clone();bad.epoch+=1;assert!(ok(&bad).is_err());
    let mut bad=proposal.clone();bad.last=None;assert!(ok(&bad).is_err());
    let mut bad=proposal.clone();bad.uid=bad.uid.wrapping_add(1);assert!(ok(&bad).is_err());
    let mut bad=proposal.clone();bad.ticket=f.last.created_by;assert!(ok(&bad).is_err());
    let mut bad=proposal.clone();if let Purpose::Delete{at,..}=bad.purpose{bad.purpose=Purpose::Delete{previous:[0;32],at};}assert!(ok(&bad).is_err());
    assert!(s.decided(r.op).is_none()&&s.moving.is_none());assert!(fs::read_dir(&rr.volume.path).unwrap().next().is_none());
}

#[test]
#[ignore="isolated real roots only; private original reservation/cold reprepare, no lifecycle decision"]
fn the_same_operation_reopens_its_unbound_reservation_without_choosing_a_new_delete_time_or_hiding_the_source(){
    use crate::ordinary::custody::{allocation::MoveAllocation,incoming::IncomingSelection,purpose::Call,source::Clean};
    let f=Fixture::build(true,false);let rbin=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap();
    let r=Request{op:op(),id:f.file.file,epoch:0,call:Call::Delete,from:f.from,to:rbin.id};
    let reserve=||{let src=Clean::capture_lifecycle(&f.sys,r)?.verify()?;
        let dst=IncomingSelection::prepare(&f.sys,&src,r)?;MoveAllocation::reserve_lifecycle(&f.sys,&dst)};
    let issued=reserve().unwrap();
    let original_cut=||{let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
        let a=s.moving.as_ref().unwrap();assert_eq!(a.original,issued);assert!(a.stage().is_none()&&!a.opened()&&s.pending.is_none());
        assert!(s.decided(r.op).is_none());assert_eq!(s.placement(r.id).unwrap().volume(),r.from);
        (a.seq,a.previous,a.digest,a.stamp.clone())};
    let held=original_cut();f.all_history();assert!(Clean::capture_lifecycle(&f.sys,Request{op:op(),..r}).is_err());
    let blocked=op();assert!(z::create_empty(&f.sys,blocked,f.to,"sibling").is_err());
    // No named target ever existed. Reopening rediscovers only the original
    // native SYSTEM allocation; it must re-verify/reallocate all anonymous roles.
    assert_eq!(issued,reserve().unwrap());assert_eq!(original_cut(),held);
    assert_eq!(z::list_initial_files(&f.sys,f.from).unwrap().files[0].file,r.id);
    assert!(fs::read_dir(&rbin.volume.path).unwrap().next().is_none());
    let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
    assert!(s.decided(blocked).is_none()&&s.decided(r.op).is_none());
}

#[test]
#[ignore="three private filesystems; a genuine B-to-R original reservation, its unbound gap and exact bound private directory only"]
fn a_dedicated_stage_never_adopts_the_unbound_directory_or_replaces_a_missing_bound_one(){
    use std::os::unix::ffi::OsStrExt;
    use crate::ordinary::custody::{allocation::MoveAllocation,incoming::IncomingSelection,purpose::Call,source::Clean};
    let f=Fixture::build(true,false);f.call(f.cmd()).unwrap();
    let rr=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap();
    let r=Request{op:op(),id:f.file.file,epoch:1,call:Call::Delete,from:f.to,to:rr.id};
    let ticket={
        let source=Clean::capture_lifecycle(&f.sys,r).unwrap().verify().unwrap();
        let ready=IncomingSelection::prepare(&f.sys,&source,r).unwrap();
        assert!(ready.ensure_private_stage(&f.sys,||panic!("unissued mkdir")).is_err());
        let o=MoveAllocation::reserve_lifecycle(&f.sys,&ready).unwrap();
        assert!(ready.ensure_private_stage(&f.sys,||Err(io::Error::other("crash after durable mkdir, before binding"))).is_err());o.ticket
    };
    let path=|i|rr.volume.path.join(format!(".zxs-new-{ticket}-{i}"));
    let residue=fs::metadata(path(0)).unwrap().ino();assert!(!path(1).exists());
    let original_bound={
        let source=Clean::capture_lifecycle(&f.sys,r).unwrap().verify().unwrap();
        let ready=IncomingSelection::prepare(&f.sys,&source,r).unwrap();
        assert_eq!(ready.ensure_private_stage(&f.sys,||Ok(())).unwrap(),1);
        assert_eq!(ready.ensure_private_stage(&f.sys,||panic!("must not remake a known stage")).unwrap(),1);
        let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
        let a=s.moving.as_ref().unwrap();assert_eq!(a.original.ticket,ticket);assert_eq!(a.stage().unwrap().slot,1);
        a.stage().unwrap().directory.clone()
    };
    assert_ne!(original_bound.entity.inode,residue);assert_eq!(fs::metadata(path(0)).unwrap().ino(),residue);
    let current=z::list_initial_files(&f.sys,f.to).unwrap();
    assert!(current.files.iter().any(|x|x.file==r.id));assert!(current.pending_moves.is_empty());
    {let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
        let receipt=r.observe(&p,&s).unwrap().unwrap();assert_eq!((receipt.from,receipt.to,receipt.name.as_str(),receipt.source_epoch,receipt.destination_epoch),
            (f.to,rr.id,"name",1,2));assert_eq!(receipt.status,MoveStatus::Reserved);assert!(s.decided(r.op).is_none());}
    let leaf=r.call.leaves(r.id,r.op,"name").unwrap().1;assert!(!rr.volume.path.join(std::ffi::OsStr::from_bytes(leaf.to_bytes())).exists());
    let saved=rr.volume.path.with_file_name("elsewhere-unrelated-to-registered-R");
    fs::rename(path(1),&saved).unwrap();
    let inspect=||{let held=Clean::capture_lifecycle(&f.sys,r)?.verify()?;IncomingSelection::prepare(&f.sys,&held,r).map(|_|())};
    assert!(inspect().is_err());assert!(!path(1).exists());
    std::os::unix::fs::symlink(&saved,path(1)).unwrap();assert!(inspect().is_err());
    assert!(fs::symlink_metadata(path(1)).unwrap().file_type().is_symlink());fs::remove_file(path(1)).unwrap();
    fs::rename(&saved,path(1)).unwrap();inspect().unwrap();
    fs::write(path(0).join("keep"),b"unattributed").unwrap();assert!(inspect().is_err());
    assert_eq!(fs::read(path(0).join("keep")).unwrap(),b"unattributed");f.all_history();
}

#[test]
#[ignore="three private native filesystems; original linked-prefix and independently verified private target, not a lifecycle decision"]
fn a_true_private_target_resumes_only_a_contiguous_original_prefix_and_reopens_every_named_history_role(){
    use crate::ordinary::custody::{admission::{LifecycleGrant,Side},allocation::MoveAllocation,incoming::IncomingSelection,purpose::Call,source::Clean};
    let f=Fixture::build(true,false);let rr=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap();
    let r=Request{op:op(),id:f.file.file,epoch:0,call:Call::Delete,from:f.from,to:rr.id};
    let stage={
        let src=Clean::capture_lifecycle(&f.sys,r).unwrap().verify().unwrap();let out=IncomingSelection::prepare(&f.sys,&src,r).unwrap();
        let o=MoveAllocation::reserve_lifecycle(&f.sys,&out).unwrap();let i=out.ensure_private_stage(&f.sys,||Ok(())).unwrap();
        let mut n=0;assert!(out.name_and_reopen(&f.sys,||{n+=1;if n==2{Err(io::Error::other("stop after the second original is linked and synced"))}else{Ok(())}}).is_err());
        assert_eq!(n,2);rr.volume.path.join(format!(".zxs-new-{}-{i}",o.ticket))
    };
    let first=inventory_ids(&stage);assert_eq!(first.len(),2);let kept=stage.join("FILE");
    let away=rr.volume.path.with_file_name("old-held-private-role");fs::rename(&kept,&away).unwrap();
    let prepare=||{let src=Clean::capture_lifecycle(&f.sys,r)?.verify()?;IncomingSelection::prepare(&f.sys,&src,r).map(|_|())};
    assert!(prepare().is_err());assert_eq!(fs::read_dir(&stage).unwrap().count(),1);assert!(!kept.exists());
    fs::rename(&away,&kept).unwrap();
    let verified={let src=Clean::capture_lifecycle(&f.sys,r).unwrap().verify().unwrap();let out=IncomingSelection::prepare(&f.sys,&src,r).unwrap();
        assert_eq!(out.inspection().unwrap().len(),2); // only the two never-named originals were allocated anew
        let mut n=0;let target=out.name_and_reopen(&f.sys,||{n+=1;Ok(())}).unwrap();assert_eq!(n,2);target.verify().unwrap()};
    let after=inventory_ids(&stage);assert_eq!(after.len(),4);for(k,v)in first{assert_eq!(after[&k],v);}
    {let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Incoming).unwrap();let g=LifecycleGrant::capture(&p,r).unwrap();
        let endpoint=p.custody_end(&g,Side::Target).unwrap();verified.require(&p,&g,&endpoint).unwrap();
        assert!(s.decided(r.op).is_none()&&s.pending.is_none());assert!(!endpoint.target_present().unwrap());}
    drop(verified);f.all_history();
    let raw=stage.join(format!("{}-name",f.first.storage));let mut body=fs::read(&raw).unwrap();body[0]^=31;fs::write(&raw,&body).unwrap();
    assert!(prepare().is_err());assert_eq!(fs::read(&raw).unwrap(),body);f.all_history();
    let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();assert_eq!(s.placement(r.id).unwrap().volume(),f.from);
    assert!(s.decided(r.op).is_none()&&s.pending.is_none());
}

#[test]
#[ignore="private genuine FILE+SYSTEM and dedicated recycle filesystem; original decision and independent installed target only"]
fn delete_cold_frontier_hides_only_the_new_decision_and_its_exact_recycle_can_reopen_without_the_source(){
    use crate::ordinary::custody::{lifecycle,purpose::Call};
    let f=Fixture::build(true,false);f.call(f.cmd()).unwrap();let rr=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap();
    let r=Request{op:op(),id:f.file.file,epoch:1,call:Call::Delete,from:f.to,to:rr.id};
    let sibling=z::create_empty(&f.sys,op(),f.to,"x").unwrap();
    for stop in [Cut::Intent,Cut::FrameSynced]{
        assert!(lifecycle::decide(&f.sys,r,&mut |at|if at==stop{Err(io::Error::other("stop original deletion"))}else{Ok(())}).is_err());
        let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let before=s.placement(r.id).unwrap();
        assert_eq!((before.volume(),before.epoch()),(f.to,1));assert!(before.retained().is_none());
        let a=s.moving.as_ref().unwrap();assert!(s.pending.as_ref().unwrap().frame.moved().is_ok()&&!a.opened()&&s.decided(r.op).is_none());
        assert_eq!(r.observe(&p,&s).unwrap().unwrap().status,if stop==Cut::Intent{MoveStatus::Prepared}else{MoveStatus::AwaitingDecisionConfirmation});
        drop(p);f.all_history();
    }
    let m=lifecycle::decide(&f.sys,r,&mut |_|Ok(())).unwrap();
    let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
    let now=s.placement(r.id).unwrap();assert_eq!((now.volume(),now.epoch()),(rr.id,2));
    assert_eq!(now.retained().unwrap().decision,m.digest);assert_eq!(s.before_handoff(&m).unwrap().volume(),f.to);
    assert_eq!(r.observe(&p,&s).unwrap().unwrap().status,MoveStatus::DecidedTargetPending);drop(p);
    for n in [&f.first,&f.last]{assert!(z::read_node_to(&f.sys,r.id,n.node,&mut Vec::new()).is_err());}
    assert!(z::current_info(&f.sys,r.id).is_err());assert!(f.new.join("name").is_dir());
    let listed=z::list_initial_files(&f.sys,f.to).unwrap();assert_eq!(listed.files,[sibling]);assert!(listed.pending_moves.is_empty());
    let offline=f.new.with_file_name("source-disconnected-after-original-decision");fs::rename(&f.new,&offline).unwrap();
    assert!(lifecycle::installed(&f.sys,r,&m,&mut |at|if at==Cut::MoveInstalled{Err(io::Error::other("rename completed before parent sync"))}else{Ok(())}).is_err());
    let checked=lifecycle::installed(&f.sys,r,&m,&mut |_|Ok(())).unwrap();assert!(checked.installed());
    let old=offline.join("name");assert_eq!(fs::read_dir(old).unwrap().count(),4); // installed is not a completed retirement
    let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let receipt=r.observe(&p,&s).unwrap().unwrap();
    assert_eq!((receipt.status,receipt.decision_sequence),(MoveStatus::TargetReopenedSourcePending,Some(m.seq)));
    assert!(r.observe(&p,&s).is_ok());assert!(Request{op:r.op,epoch:0,..r}.observe(&p,&s).is_err());
}

#[test]
#[ignore="isolated private native roots; change named RAW after its link but before its first target read"]
fn even_same_inode_same_size_corruption_before_the_first_named_scan_cannot_seal_a_deletion(){
    use crate::ordinary::custody::{allocation::MoveAllocation,incoming::IncomingSelection,purpose::Call,source::Clean};
    let f=Fixture::build(true,false);let rr=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap();
    let r=Request{op:op(),id:f.file.file,epoch:0,call:Call::Delete,from:f.from,to:rr.id};
    let src=Clean::capture_lifecycle(&f.sys,r).unwrap().verify().unwrap();let target=IncomingSelection::prepare(&f.sys,&src,r).unwrap();
    let orig=MoveAllocation::reserve_lifecycle(&f.sys,&target).unwrap();
    let slot=target.ensure_private_stage(&f.sys,||Ok(())).unwrap();
    let raw=rr.volume.path.join(format!(".zxs-new-{}-{slot}",orig.ticket)).join(format!("{}-name",f.first.storage));
    let mut count=0;let mut wrong=BEFORE.to_vec();wrong[0]^=97;
    let seen=target.name_and_reopen(&f.sys,||{count+=1;if count==3{let ino=fs::metadata(&raw)?.ino();fs::write(&raw,&wrong)?;
        assert_eq!(fs::metadata(&raw)?.ino(),ino);}Ok(())});
    assert!(seen.and_then(|scan|scan.verify()).is_err());drop(target);drop(src);assert_eq!(fs::read(raw).unwrap(),wrong);
    let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();assert!(s.decided(r.op).is_none()&&s.pending.is_none());
    assert!(s.moving.as_ref().unwrap().stage().is_some());f.all_history();
}

#[test]
#[ignore="private original root and dedicated recycle filesystem; conflict between frame sync and lower-bound acknowledgement"]
fn a_warm_complete_but_unconfirmed_delete_refuses_the_same_target_conflict_as_a_cold_retry(){
    use crate::ordinary::custody::{lifecycle,purpose::Call};use std::os::unix::ffi::OsStrExt;
    let f=Fixture::build(true,false);let rr=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap();
    let r=Request{op:op(),id:f.file.file,epoch:0,call:Call::Delete,from:f.from,to:rr.id};
    let n=r.call.leaves(r.id,r.op,&f.file.name).unwrap().1;let competing=rr.volume.path.join(std::ffi::OsStr::from_bytes(n.to_bytes()));
    let mut arrived=false;
    assert!(lifecycle::decide(&f.sys,r,&mut |c|{if c==Cut::FrameSynced{
        fs::write(&competing,b"not this FILE")?;arrived=true;}Ok(())}).is_err());
    assert!(arrived);assert!(lifecycle::decide(&f.sys,r,&mut |_|Ok(())).is_err());
    {let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let old=s.placement(r.id).unwrap();
        assert_eq!((old.volume(),old.epoch(),old.retained()),(f.from,0,None));assert!(s.decided(r.op).is_none());
        assert_eq!(r.observe(&p,&s).unwrap().unwrap().status,MoveStatus::AwaitingDecisionConfirmation);}
    assert_eq!(fs::read(&competing).unwrap(),b"not this FILE");f.all_history();
    fs::remove_file(competing).unwrap();let m=lifecycle::decide(&f.sys,r,&mut |_|Ok(())).unwrap();
    assert_eq!(m.original.op,r.op);let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
    assert_eq!(s.placement(r.id).unwrap().retained().unwrap().decision,m.digest);
}

#[test]
#[ignore="three private registered filesystems; actual DELETE/RESTORE native pair, no permanent deletion"]
fn later_b_native_child_goes_to_recycle_by_its_own_original_and_explicit_restore_does_not_open_offline_b(){
    use crate::ordinary::custody::{lifecycle,purpose::Call};use std::os::unix::ffi::OsStrExt;
    let f=Fixture::build(true,false);let(file,parent,moved)=seed(&f);f.call(moved).unwrap();
    let before_b=inventory_ids(&f.new.join("one"));assert_eq!(before_b.len(),3);
    let bbytes=b"real original later B-native INCR";
    z::save_interval(&f.sys,op(),file.file,1,edit(0,bbytes),Cursor::new(bbytes)).unwrap();
    let child=z::commit_working(&f.sys,op(),file.file,2).unwrap();
    let outsider=z::create_empty(&f.sys,op(),f.to,"x").unwrap();let rr=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap();
    let del=Request{op:op(),call:Call::Delete,id:file.file,from:f.to,to:rr.id,epoch:1};
    let kept=rpath(&rr.volume.path,&del,&file.name);
    fn rpath(root:&Path,del:&Request,name:&str)->PathBuf{let n=del.call.leaves(del.id,del.op,name).unwrap().1;
        root.join(std::ffi::OsStr::from_bytes(n.to_bytes()))}
    let at_b=f.new.join("one");
    assert!(lifecycle::perform(&f.sys,del,&mut |c|if c==Cut::MoveSourceIsolated{Err(io::Error::other("source isolated"))}else{Ok(())}).is_err());
    assert!(!at_b.exists());assert_eq!(inventory_ids(&kept).len(),4);
    assert_eq!(z::list_initial_files(&f.sys,f.to).unwrap().files,[outsider.clone()]);
    assert_eq!(lifecycle::perform(&f.sys,del,&mut |_|Ok(())).unwrap().status,MoveStatus::CompleteCheckedNow);
    let(frame,retained)={let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let m=s.placed(file.file).unwrap();
        let before=s.before_handoff(m).unwrap().transfer().unwrap();assert_eq!(before.original.op,moved.op);assert!(before.target.incr.is_none());
        assert_eq!(m.original.last,s.delta(file.file).map(|d|d.digest));assert!(m.target.incr.is_some());
        (m.digest,s.placement(file.file).unwrap().retained().unwrap())};
    assert_eq!(retained.operation,del.op);assert_eq!(retained.decision,frame);
    // Same human name, different, later FILE. Its existence is not evidence
    // for the retained original and must not be a clobber target.
    let replacement=z::create_empty(&f.sys,op(),f.to,"one").unwrap();let clean_replacement=inventory_ids(&at_b);
    let restore=Request{op:op(),call:Call::Restore{deleted_by:del.op,decision:frame},id:file.file,from:rr.id,to:f.to,epoch:2};
    assert!(lifecycle::perform(&f.sys,restore,&mut |_|Ok(())).is_err());assert_eq!(inventory_ids(&at_b),clean_replacement);
    let alternate=Request{op:op(),to:f.from,..restore};
    let offline=f.new.with_file_name("offline-original-B");fs::rename(&f.new,&offline).unwrap();
    let a_sibling=z::create_empty(&f.sys,op(),f.from,"other").unwrap();
    let mut visits=0;
    assert!(lifecycle::perform(&f.sys,alternate,&mut |c|if c==Cut::Decision{visits+=1;Err(io::Error::other("before installation"))}else{Ok(())}).is_err());
    assert_eq!(visits,1);assert!(z::current_info(&f.sys,file.file).is_err());
    let list=z::list_initial_files(&f.sys,f.from).unwrap();
    assert!(list.files.iter().any(|x|x.file==a_sibling.file)&&list.files.iter().all(|x|x.file!=file.file));
    assert!(lifecycle::perform(&f.sys,alternate,&mut |c|if c==Cut::MoveInstalled{Err(io::Error::other("rename before sync"))}else{Ok(())}).is_err());
    assert!(z::list_initial_files(&f.sys,f.from).unwrap().files.iter().all(|x|x.file!=file.file));
    assert!(lifecycle::perform(&f.sys,alternate,&mut |c|if c==Cut::MoveTargetChecked{Err(io::Error::other("opened destination before old recycle cleanup"))}else{Ok(())}).is_err());
    node(&f,file.file,&parent,BEFORE);node(&f,file.file,&child,bbytes);
    assert!(z::list_initial_files(&f.sys,f.from).unwrap().files.iter().any(|x|x.file==file.file));
    assert!(kept.is_dir());let restored=lifecycle::perform(&f.sys,alternate,&mut |_|Ok(())).unwrap();assert!(!kept.exists());
    assert_eq!((restored.file,restored.entry,restored.history,restored.destination_epoch),(file.file,file.entry,file.history,3));
    assert_eq!(lifecycle::perform(&f.sys,del,&mut |_|Ok(())).unwrap().status,MoveStatus::OriginalPublished);
    let again=Request{op:op(),call:Call::Delete,id:file.file,from:f.from,to:rr.id,epoch:3};
    lifecycle::perform(&f.sys,again,&mut |_|Ok(())).unwrap();
    let current=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap().2.placement(file.file).unwrap().retained().unwrap();
    assert_eq!(current.operation,again.op);assert_ne!(current.decision,frame);
    assert!(lifecycle::perform(&f.sys,Request{op:op(),epoch:4,..alternate},&mut |_|Ok(())).is_err());
    assert_eq!(lifecycle::perform(&f.sys,alternate,&mut |_|Ok(())).unwrap().status,MoveStatus::OriginalPublished);
    fs::rename(&offline,&f.new).unwrap();assert_eq!(inventory_ids(&at_b),clean_replacement);
    let listed=z::list_initial_files(&f.sys,f.to).unwrap();assert!(listed.files.contains(&replacement)&&listed.files.contains(&outsider));
}

#[test]
#[ignore="isolated ordinary/recycle roots; exact retained source cannot vanish before authenticated empty quarantine"]
fn restoring_after_armed_refuses_a_relocated_whole_retained_original_but_resumes_after_the_true_final_rmdir(){
    use crate::ordinary::custody::{lifecycle,purpose::Call};use std::os::unix::ffi::OsStrExt;
    let f=Fixture::build(true,false);let(file,parent,_)=seed(&f);
    let rr=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap();
    let del=Request{op:op(),id:file.file,epoch:0,call:Call::Delete,from:f.from,to:rr.id};
    lifecycle::perform(&f.sys,del,&mut |_|Ok(())).unwrap();
    let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let deleted=s.placed(del.id).unwrap().digest;
    let kept=rr.volume.path.join(std::ffi::OsStr::from_bytes(del.call.leaves(file.file,del.op,"one").unwrap().1.to_bytes()));
    let originals=inventory_ids(&kept);assert_eq!(originals.len(),3);
    let r=Request{op:op(),id:del.id,epoch:1,from:rr.id,to:f.to,call:Call::Restore{deleted_by:del.op,decision:deleted}};
    assert!(lifecycle::perform(&f.sys,r,&mut |c|if c==Cut::MoveSourceAdmitted{Err(io::Error::other("armed only"))}else{Ok(())}).is_err());
    let unexpected=kept.with_file_name("unattributed-whole-original");fs::rename(&kept,&unexpected).unwrap();
    assert!(lifecycle::perform(&f.sys,r,&mut |_|Ok(())).is_err());node(&f,r.id,&parent,BEFORE);
    let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
    assert_eq!(r.observe(&p,&s).unwrap().unwrap().status,MoveStatus::TargetReopenedSourcePending);
    assert!(s.moving.is_some()&&s.pending.is_some());drop(p);assert_eq!(inventory_ids(&unexpected),originals);
    fs::rename(&unexpected,&kept).unwrap();let mut last=false;
    assert!(lifecycle::perform(&f.sys,r,&mut |c|if c==Cut::MoveDirectoryUnlinked{last=true;Err(io::Error::other("rmdir before parent sync"))}else{Ok(())}).is_err());
    assert!(last&&!kept.exists());node(&f,r.id,&parent,BEFORE);
    assert_eq!(lifecycle::perform(&f.sys,r,&mut |_|Ok(())).unwrap().status,MoveStatus::CompleteCheckedNow);
    let restored=z::save_interval(&f.sys,op(),r.id,1,edit(0,b"new"),Cursor::new(b"new")).unwrap();
    let _=restored;let child=z::commit_working(&f.sys,op(),r.id,2).unwrap();
    node(&f,r.id,&parent,BEFORE);node(&f,r.id,&child,AFTER);
}

// Test-source fixtures only. All paths are isolated test roots; no production
// directory is scanned or borrowed by any of these scenarios.
use std::io::Read;
use crate::pool::lease::{Access,LocalPool};

fn seed(f:&Fixture)->(FileInfo,FormalNode,Request){
    let item=z::create_empty(&f.sys,op(),f.from,"one").unwrap();
    z::save_first(&f.sys,op(),item.file,claim(BEFORE),Cursor::new(BEFORE)).unwrap();
    let first=z::commit_working(&f.sys,op(),item.file,1).unwrap();
    let r=Request{op:op(),id:item.file,from:f.from,to:f.to,epoch:0};
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

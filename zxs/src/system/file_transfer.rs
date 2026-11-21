//! Durable handoff of a single authorized FILE between its actual F and R.
//! The immutable birth is never reissued. Slow data, capacity and witness
//! checks are outside SYSTEM; only the selected receipt switches placement.
use super::{Tree,Catalog,Connection,directory,file_create,ordinary_directory::{self,Additional,Route as DirectoryRoute}};
use super::super::{file_location::{Place,Placed},transfer_material::Manifest,volume::{OrdinaryRoot,RecycleRoot},work_record::{self,ReadRecord}};
use crate::{OperationId,Role,VolumeId,display,fs::{HeldDir,invalid,directory_birth::DirectoryParent},hash::Sha256,
    identity::{AccountId,ShareId},ordinary::portable_birth::{Birth,MoveInstall,MoveRoom}};
use std::io;

#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) enum Handoff{
    Delete(Place),
    #[cfg(test)]
    Restore{deleted:OperationId,name:String,target:String},
    RestoreBound{source:Place,target:String},
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Direction{Delete=1,Restore=2}
#[derive(Clone,Debug,Eq,PartialEq)]
struct Route{
    from:Placed,kind:Direction,to:VolumeId,parent:DirectoryParent,name:String,work:Option<ReadRecord>,head:Option<OperationId>,
    share:ShareId,setting:OperationId,auth:OperationId,actor:AccountId,epoch:i64,
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) struct Moved{pub(super) op:OperationId,pub(super) from:Place,pub(super) to:Place}

fn occupied()->io::Error{io::Error::new(io::ErrorKind::AlreadyExists,"destination name is already selected")}
fn pending()->io::Error{io::Error::new(io::ErrorKind::WouldBlock,"another original transfer for this FILE is unfinished")}
impl Route{
    fn select(tree:&Tree<'_,'_>,db:&Connection,c:&Catalog,ask:&Handoff)->io::Result<Self>{
        tree.require(db,c,true)?;
        let r=c.rows().get(1).filter(|v|v.role==Role::Recycle&&v.ordinal==1)
            .ok_or_else(||invalid("missing original retained role"))?;
        let f=tree.setting.volume;
        let (place,kind,to,name)=match ask{
            Handoff::Delete(native)=>{
                if native.role!=Role::File||native.volume!=f||native.parent!=tree.parent||Place::current(db,c,native.file)?!=*native{
                    return Err(io::ErrorKind::NotFound.into());
                }
                Place::require_no_rename(db,native.file)?;
                (native.clone(),Direction::Delete,r.volume,native.name.clone())
            }
            #[cfg(test)]
            Handoff::Restore{deleted,name,target}=>{
                display::component(target.as_bytes(),222)?;
                (Place::retained(db,c,r.volume,*deleted,name)?,Direction::Restore,f,target.clone())
            }
            Handoff::RestoreBound{source,target}=>{
                display::component(target.as_bytes(),222)?;
                let current=Place::retained(db,c,r.volume,source.op,&source.name)?;
                if current!=*source{return Err(io::ErrorKind::NotFound.into());}
                (current,Direction::Restore,f,target.clone())
            }
        };
        let from=place.bind(db,c)?;
        let work=work_record::selected(db,&from.birth)?;
        let head=work_record::latest_commit(db,&from.birth)?.map(|n|n.0);
        let a=&tree.session.original;
        let parent=if kind==Direction::Restore{tree.parent}else{DirectoryParent::Root};
        Ok(Self{from,kind,to,parent,name,work,head,share:tree.setting.share,setting:tree.setting.op,auth:a.op,actor:a.account,epoch:a.generation})
    }
    fn receipt(tree:&Tree<'_,'_>,db:&Connection,c:&Catalog,ask:&Handoff,op:OperationId)->io::Result<Option<Moved>>{
        tree.require(db,c,true)?;
        let q=db.prepare(c"SELECT m.file_id,m.direction,m.source_op,m.source_gen,m.source_id,m.source_name,
               m.target_id,m.target_name,m.save_op,m.manual_op,m.source_parent,m.target_parent,m.source_backing,m.source_terminal
               FROM main.operation o JOIN main.file_transfer m ON m.transfer_op=o.op_id
               WHERE o.op_id=?1 AND o.kind=12 AND o.state=1")?;
        q.bind_bytes(1,&op.0)?;
        if !work_record::start_one(&q)?{return Ok(None);}
        let file=crate::FileId(q.array(0)?);
        let (kind,role)=match q.byte(1)?{1=>(Direction::Delete,Role::File),2=>(Direction::Restore,Role::Recycle),
            _=>return Err(invalid("wrong original transfer direction"))};
        let name=display::component(q.blob(5,222)?,222)?.to_owned();
        let backing=Place::read_backing(&q,12,13,file,&name)?;
        let src=Place{file,op:OperationId(q.array(2)?),generation:q.integer(3)?,volume:VolumeId(q.array(4)?),
            role,name,parent:DirectoryParent::from_bytes(q.array(10)?),backing};
        let to=VolumeId(q.array(6)?);let name=display::component(q.blob(7,222)?,222)?.to_owned();
        let save=work_record::optional(&q,8)?;let head=work_record::optional(&q,9)?;
        let parent=DirectoryParent::from_bytes(q.array(11)?);q.done()?;
        let allowed=match ask{
            Handoff::Delete(expected)=>kind==Direction::Delete&&*expected==src&&src.volume==tree.setting.volume&&src.parent==tree.parent&&parent==DirectoryParent::Root,
            #[cfg(test)]
            Handoff::Restore{deleted,name:show,target}=>kind==Direction::Restore&&*deleted==src.op&&*show==src.name&&
                target==&name&&to==tree.setting.volume&&parent==tree.parent,
            Handoff::RestoreBound{source,target}=>kind==Direction::Restore&&*source==src&&target==&name&&to==tree.setting.volume&&parent==tree.parent,
        };
        if !allowed{return Err(io::ErrorKind::PermissionDenied.into());}
        let birth=file_create::committed_file(db,c,file)?;
        let work=save.map(|v|work_record::confirmed(db,&birth,v)).transpose()?;
        let a=&tree.session.original;
        let original=Self{from:Placed{birth,here:src.clone()},kind,to,parent,name,work,head,
            share:tree.setting.share,setting:tree.setting.op,auth:a.op,actor:a.account,epoch:a.generation};
        if !original.exists_at(db,op,true)?{return Err(invalid("no exact result for this original transfer"));}
        Ok(Some(Moved{op,from:src,to:original.target(op)}))
    }
    fn target(&self,op:OperationId)->Place{
        Place{file:self.from.here.file,op,generation:self.from.here.generation+1,volume:self.to,
            role:if self.kind==Direction::Delete{Role::Recycle}else{Role::File},name:self.name.clone(),parent:self.parent,backing:Some(op)}
    }
    fn geometry(&self,c:&Catalog,op:OperationId,dest:&DirectoryRoute)->io::Result<()>{
        if self.from.here.generation>i64::MAX-2{return Err(invalid("placement sequence exhausted"));}
        let r=&c.rows()[1];
        self.from.birth.validate_transfer_geometry(&self.name,(&r.path,r.limit),op)?;
        if self.kind==Direction::Restore{
            let f=c.rows().iter().find(|r|r.role==Role::File&&r.volume==self.to)
                .ok_or_else(||invalid("restore destination is not an ordinary enrolled volume"))?;
            let path=dest.geometry(c)?;
            if dest.parent()!=self.parent||dest.volume()!=self.to{return Err(invalid("restore parent differs"));}
            self.from.birth.validate_transfer_geometry(&self.name,(&path,f.limit),op)?;
        }
        Ok(())
    }
    fn digest(&self,op:OperationId)->io::Result<[u8;32]>{
        let b=&self.from.birth;let f=&self.from.here;
        let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/FILE-TRANSFER-BACKING\0")?;
        for v in [b.pool.0,b.operation.0,b.file.0,b.entry.0,b.history.0,b.branch.0,
            op.0,f.op.0,f.volume.0,self.to.0,f.parent.bytes(),self.parent.bytes(),self.share.0,self.setting.0,self.auth.0,self.actor.0]{h.update(&v)?;}
        h.update(&[self.kind as u8])?;h.update(&f.generation.to_be_bytes())?;h.update(&self.epoch.to_be_bytes())?;
        for n in [&f.name,&self.name]{h.update(&[n.len() as u8])?;h.update(n.as_bytes())?;}
        for k in [f.backing,self.work.map(|v|v.record.key().save),self.head]{
            h.update(&[u8::from(k.is_some())])?;if let Some(k)=k{h.update(&k.0)?;}
        }
        Ok(h.finish())
    }
    fn exists_at(&self,db:&Connection,op:OperationId,committed:bool)->io::Result<bool>{
        let q=db.prepare(c"SELECT o.kind,o.state,o.actor_id,o.request_sha,m.file_id,m.direction,m.source_op,m.source_gen,
          m.source_id,m.source_name,m.target_id,m.target_name,m.share_id,m.setting_op,m.auth_op,m.save_op,m.manual_op,m.source_parent,m.target_parent,m.source_backing,m.source_terminal
          FROM main.operation o LEFT JOIN main.file_transfer m ON m.transfer_op=o.op_id WHERE o.op_id=?1")?;
        q.bind_bytes(1,&op.0)?;
        if !work_record::start_one(&q)?{return Ok(false);}
        let f=&self.from.here;
        if q.byte(0)?!=12||q.byte(1)?!=u8::from(committed)||q.array::<16>(2)?!=self.actor.0||q.array::<32>(3)?!=self.digest(op)?||
            q.array::<16>(4)?!=f.file.0||q.byte(5)?!=self.kind as u8||q.array::<16>(6)?!=f.op.0||q.integer(7)?!=f.generation||
            q.array::<16>(8)?!=f.volume.0||q.blob(9,222)?!=f.name.as_bytes()||q.array::<16>(10)?!=self.to.0||q.blob(11,222)?!=self.name.as_bytes()||
            q.array::<16>(12)?!=self.share.0||q.array::<16>(13)?!=self.setting.0||q.array::<16>(14)?!=self.auth.0||
            work_record::optional(&q,15)?!=self.work.map(|r|r.record.key().save)||work_record::optional(&q,16)?!=self.head||
            q.array::<16>(17)?!=f.parent.bytes()||q.array::<16>(18)?!=self.parent.bytes()||
            Place::read_backing(&q,19,20,f.file,&f.name)?!=f.backing{
            return Err(invalid("recorded transfer differs from its original scope"));
        }
        q.done()?;Ok(true)
    }
    fn still_live(&self,db:&Connection,op:OperationId)->io::Result<()>{
        if !self.exists_at(db,op,false)?{return Err(pending());}
        let q=db.prepare(c"SELECT actor_id FROM main.live_transfer WHERE transfer_op=?1")?;
        q.bind_bytes(1,&op.0)?;
        if !work_record::start_one(&q)?||q.array::<16>(0)?!=self.actor.0{return Err(io::ErrorKind::PermissionDenied.into());}
        q.done()
    }
    fn reservation(&self,db:&Connection)->io::Result<Option<(OperationId,bool)>>{
        let q=db.prepare(c"SELECT m.transfer_op,EXISTS(SELECT 1 FROM main.live_transfer v WHERE v.transfer_op=m.transfer_op)
            FROM main.pending_transfer m WHERE m.file_id=?1")?;
        q.bind_bytes(1,&self.from.here.file.0)?;
        if !work_record::start_one(&q)?{return Ok(None);}
        let x=(OperationId(q.array(0)?),q.byte(1)?==1);q.done()?;Ok(Some(x))
    }
    fn release_unselected(&self,db:&Connection,op:OperationId,why:u8)->io::Result<()>{
        let q=db.prepare(c"INSERT INTO main.file_transfer_void(transfer_op,setting_op,auth_op,reason) VALUES(?1,?2,?3,?4)")?;
        q.bind_bytes(1,&op.0)?;q.bind_bytes(2,&self.setting.0)?;q.bind_bytes(3,&self.auth.0)?;
        q.bind_integer(4,why.into())?;q.done()
    }
    fn vacant(&self,db:&Connection,except:Option<OperationId>)->io::Result<()>{
        if self.kind!=Direction::Restore{return Ok(());}
        if Place::renaming_name(db,self.to,self.parent,&self.name)?{return Err(pending());}
        let q=db.prepare(c"SELECT 1 FROM main.file_location WHERE role=2 AND volume_id=?1 AND display=?2 AND parent_id=?3
          UNION ALL SELECT 1 FROM main.directory_slot WHERE volume_id=?1 AND display=?2 AND parent_id=?3 LIMIT 1")?;
        q.bind_bytes(1,&self.to.0)?;q.bind_bytes(2,self.name.as_bytes())?;q.bind_bytes(3,&self.parent.bytes())?;
        if work_record::start_one(&q)?{return Err(occupied());}
        let q=db.prepare(c"SELECT transfer_op FROM main.pending_transfer WHERE direction=2 AND target_id=?1 AND target_name=?2 AND target_parent=?3")?;
        q.bind_bytes(1,&self.to.0)?;q.bind_bytes(2,self.name.as_bytes())?;q.bind_bytes(3,&self.parent.bytes())?;
        if work_record::start_one(&q)?&&Some(OperationId(q.array(0)?))!=except{return Err(pending());}
        Ok(())
    }
    fn issue(&self,db:&Connection,op:OperationId)->io::Result<()>{
        let p=&self.from.here;
        let q=db.prepare(c"INSERT INTO main.file_transfer(transfer_op,file_id,direction,source_op,source_gen,source_id,source_name,
            target_id,target_name,share_id,setting_op,auth_op,save_op,manual_op,source_parent,target_parent,source_backing,source_terminal)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)")?;
        q.bind_bytes(1,&op.0)?;q.bind_bytes(2,&p.file.0)?;q.bind_integer(3,self.kind as i64)?;
        q.bind_bytes(4,&p.op.0)?;q.bind_integer(5,p.generation)?;q.bind_bytes(6,&p.volume.0)?;q.bind_bytes(7,p.name.as_bytes())?;
        q.bind_bytes(8,&self.to.0)?;q.bind_bytes(9,self.name.as_bytes())?;
        for (i,x) in [self.share.0,self.setting.0,self.auth.0].iter().enumerate(){q.bind_bytes(i as i32+10,x)?;}
        if let Some(work)=self.work{q.bind_bytes(13,&work.record.key().save.0)?;}if let Some(head)=self.head{q.bind_bytes(14,&head.0)?;}
        q.bind_bytes(15,&p.parent.bytes())?;q.bind_bytes(16,&self.parent.bytes())?;
        if let Some(op)=p.backing{q.bind_bytes(17,&op.0)?;}else{q.bind_bytes(18,p.leaf()?.to_bytes())?;}
        q.done()?;drop(q);
        let st=db.prepare(c"INSERT INTO main.operation VALUES(?1,12,?2,?3,0)")?;
        st.bind_bytes(1,&op.0)?;st.bind_bytes(2,&self.actor.0)?;st.bind_bytes(3,&self.digest(op)?)?;st.done()
    }
}

fn space(dir:&HeldDir)->io::Result<u64>{
    use std::os::{fd::AsRawFd,raw::c_int};
    unsafe extern "C"{fn zxs_storage_capacity(fd:c_int,total:*mut u64,available:*mut u64)->c_int;}
    let(mut total,mut available)=(0,0);
    // SAFETY: This sample is on the held actual destination volume root.
    if unsafe{zxs_storage_capacity(dir.file.as_raw_fd(),&mut total,&mut available)}<0{return Err(io::Error::last_os_error());}
    Ok(available)
}
#[derive(Clone,Copy,Eq,PartialEq)]
enum Cut{WaitingForSource,Reserved,CandidateSynced,GuardCaptured}
impl Tree<'_,'_>{
    pub(super) fn transfer(&self,ask:&Handoff,supplied:OperationId,live:impl Fn()->io::Result<()>)->io::Result<Moved>{
        self.transfer_inner(ask,supplied,live,false,|_|Ok(()))
    }
    pub(super) fn transfer_last_close(&self,source:Place,supplied:OperationId,live:impl Fn()->io::Result<()>)->io::Result<Moved>{
        self.transfer_inner(&Handoff::Delete(source),supplied,live,true,|_|Ok(()))
    }
    #[cfg(test)]
    fn transfer_after(&self,ask:&Handoff,supplied:OperationId,live:impl Fn()->io::Result<()>,
        point:impl FnMut(Cut)->io::Result<()>)->io::Result<Moved>{
        self.transfer_inner(ask,supplied,live,false,point)
    }
    fn transfer_inner(&self,ask:&Handoff,supplied:OperationId,live:impl Fn()->io::Result<()>,
        wait_last:bool,mut point:impl FnMut(Cut)->io::Result<()>)->io::Result<Moved>{
        if supplied.0==[0;16]||supplied.0==[0xff;16]{return Err(invalid("reserved transfer operation ID"));}
        let src=self.session.source;
        live()?;
        if let Some(done)=src.at(|db,c|Route::receipt(self,db,&c,ask,supplied))?{live()?;return Ok(done);}
        let (c,from_position,ordinary_parent)=src.at(|db,c|{
            let original=Route::select(self,db,&c,ask)?;
            let parent=DirectoryRoute::at(db,&c,self.setting.volume,self.parent)?;
            Ok((c,original.from,parent))
        })?;
        live()?;
        let ordinary=OrdinaryRoot::at_parent(src,&c,&ordinary_parent,None)?;
        let retained=RecycleRoot::at(src,&c)?;retained.separate_from(&ordinary)?;
        let gates=||->io::Result<()>{live()?;ordinary.require_streaming()?;retained.require()?;retained.separate_from(&ordinary)};
        let (from,to)=if from_position.here.role==Role::File{(ordinary.folder(),retained.root_for_read())}
            else{(retained.root_for_read(),ordinary.folder())};
        let source_names=from.names()?.into_iter().collect();
        let real=loop{
            gates()?;
            match from_position.open(from,&source_names,true,|_|Ok(())){
                Ok(held)=>break held,
                Err(e) if wait_last&&e.kind()==io::ErrorKind::WouldBlock=>{
                    // A nonlast native CLOSE may have dropped its share
                    // record immediately before the storage writer released
                    // the old FILE EX. Wait only on that FILE, never in SQL
                    // or under a native share lock, and abandon on peer stop.
                    point(Cut::WaitingForSource)?;
                    std::thread::sleep(std::time::Duration::from_millis(25));
                },
                Err(e)=>return Err(e),
            }
        };
        let (plan,op,retry,retire,alias,members,material)=src.at(|db,c|{
            let plan=Route::select(self,db,&c,ask)?;
            if plan.from!=from_position{return Err(io::ErrorKind::NotFound.into());}
            let existing=plan.reservation(db)?;
            let retry=existing.filter(|p|p.1).map(|p|p.0);
            let retire=existing.filter(|p|!p.1).map(|p|p.0);
            let op=retry.unwrap_or(supplied);
            if retire==Some(op){return Err(invalid("an invalidated transfer cannot be reissued under its original ID"));}
            if let Some(p)=retry{plan.still_live(db,p)?;}
            plan.vacant(db,existing.map(|p|p.0))?;plan.geometry(&c,op,&ordinary_parent)?;
            let alias=if plan.kind==Direction::Restore{file_create::display_owner_at(db,&c,plan.to,plan.parent,&plan.name)?}else{None};
            let members=work_record::members(db,&c,&plan.from.birth)?;
            let material=Manifest::select(db,&plan.from,plan.work)?;
            Ok((plan,op,retry.is_some(),retire,alias,members,material))
        })?;
        let unchanged=|db:&Connection,c:&Catalog|->io::Result<()>{
            if Route::select(self,db,c,ask)?!=plan{return Err(io::ErrorKind::PermissionDenied.into());}
            Ok(())
        };
        members.require(&crate::fs::WitnessRead::at(real.directory())?)?;
        if let Some(old)=retire{
            // Keep potentially real old stage bytes untouched. Only the
            // exact reservation from an invalidated grant can be released.
            gates()?;real.require(from)?;
            let w=Connection::open(&src.dir,true)?;
            src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
                unchanged(db,&c)?;live()?;plan.release_unselected(db,old,1)
            })?;
        }
        let within_authority=|db:&Connection,c:&Catalog|self.require(db,c,true);
        let need=material.space(src,&plan.from,within_authority,&live)?;
        gates()?;
        let alias_at=||{
            if plan.kind==Direction::Restore{directory::check_alias_name(&ordinary,&plan.name,alias.as_ref(),None)}
            else{Ok(None)}
        };
        alias_at()?;
        let locked=HeldDir::trusted_storage(&to.path)?;locked.exclusive()?;
        if locked.entity!=to.entity||!locked.same_mount(to)?{return Err(invalid("transfer destination changed before acquiring namespace"));}
        gates()?;real.require(from)?;alias_at()?;
        let room=MoveInstall::room(&locked,op)?;
        if !retry&&room==MoveRoom::Present{return Err(io::ErrorKind::AlreadyExists.into());}
        if room==MoveRoom::Full||(room==MoveRoom::Vacant&&space(&locked)?<need){
            if retry{
                let w=Connection::open(&src.dir,true)?;
                src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
                    unchanged(db,&c)?;plan.still_live(db,op)?;live()?;plan.release_unselected(db,op,2)
                })?;
            }
            return Err(io::ErrorKind::StorageFull.into());
        }
        // In particular no CREATE, syscall reaching F/R, or peer wait
        // takes place in either short SQLite writer section.
        if !retry{
            let census=if plan.kind==Direction::Restore{Some(locked.names()?.into_iter().collect())}else{None};
            let pocket=Birth::placement_parent(op)?;
            let w=Connection::open(&src.dir,true)?;
            let issued=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
                unchanged(db,&c)?;plan.vacant(db,None)?;
                if plan.reservation(db)?.is_some(){return Err(pending());}
                if let Some(names)=&census{ordinary_directory::budget(db,plan.to,plan.parent,names,Some(Additional::Pocket(&pocket)))?;}
                live()?;plan.issue(db,op)
            });
            drop(w);
            if let Err(err)=issued{
                if !src.at(|db,c|{unchanged(db,&c)?;plan.exists_at(db,op,false)})?{return Err(err);}
            }
        }
        src.at(|db,c|{unchanged(db,&c)?;plan.still_live(db,op)})?;
        point(Cut::Reserved)?;
        let candidate=match MoveInstall::attach(&locked,&plan.from.birth,op,&plan.name){
            Ok(candidate)=>candidate,
            Err(err)=>{
                // A syscall can fail before there is any actual stage.
                // A trusted absence of the *original* pocket is needed;
                // any present or uncertain physical object is preserved.
                if MoveInstall::outer_incomplete(&err)||MoveInstall::room(&locked,op).is_ok_and(|room|room!=MoveRoom::Present){
                    gates()?;real.require(from)?;
                    let w=Connection::open(&src.dir,true)?;
                    src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
                        unchanged(db,&c)?;plan.still_live(db,op)?;live()?;plan.release_unselected(db,op,2)
                    })?;
                }
                return Err(err);
            }
        };
        drop(locked);
        let (source_used,target_used,names)=match material.transfer(src,&plan.from,real.directory(),&candidate,within_authority,&live){
            Ok(x)=>x,
            Err(error)=>{
                if candidate.pair_incomplete(&error,to)?{
                    // Use the exact chosen root and source FILE lock again.
                    // An unpaired name is never adopted, repaired, or removed.
                    let root=HeldDir::trusted_storage(&to.path)?;root.exclusive()?;
                    if root.entity!=to.entity||!root.same_mount(to)?{return Err(invalid("changed incomplete placement root"));}
                    gates()?;real.require(from)?;
                    if candidate.pair_incomplete(&error,&root)?{
                        let w=Connection::open(&src.dir,true)?;
                        src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
                            unchanged(db,&c)?;plan.still_live(db,op)?;live()?;plan.release_unselected(db,op,2)
                        })?;
                    }
                }
                return Err(error);
            }
        };
        candidate.finish(to,&names)?;point(Cut::CandidateSynced)?;
        members.require(&crate::fs::WitnessRead::at(real.directory())?)?;
        // Capture actual two-link source proofs before the switch. An
        // unpaired old residue is preserved, never made part of the target.
        let retirement=real.capture_retirement(from).ok().map(|p|p.encode());
        let target_proof=candidate.retirement_target(to,&target_used)?.encode();
        point(Cut::GuardCaptured)?;
        source_used.require(real.directory())?;target_used.require(candidate.body())?;
        // Neither target occupancy nor membership was held across the
        // bulk copy. Reacquire its short namespace lock at the final cut.
        let final_root=HeldDir::trusted_storage(&to.path)?;final_root.exclusive()?;
        if final_root.entity!=to.entity||!final_root.same_mount(to)?{return Err(invalid("target changed before final transfer selection"));}
        gates()?;real.require(from)?;candidate.require_members(&final_root,&names)?;
        let _alias=alias_at()?; // original held native alias until exact SQL switch
        let w=Connection::open(&src.dir,true)?;
        let result=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
            unchanged(db,&c)?;plan.still_live(db,op)?;plan.vacant(db,Some(op))?;
            if plan.kind==Direction::Restore&&file_create::display_owner_at(db,&c,plan.to,plan.parent,&plan.name)?!=alias{
                return Err(invalid("restore alias evidence changed"));
            }
            live()?;
            let st=db.prepare(c"UPDATE main.operation SET state=1 WHERE op_id=?1 AND kind=12 AND state=0")?;
            st.bind_bytes(1,&op.0)?;st.done()?;drop(st);
            super::retirement::record(db,op,retirement.as_deref(),&target_proof)?;
            if !plan.exists_at(db,op,true)?{return Err(invalid("transfer did not select the original receipt"));}
            Ok(())
        });
        drop(w);
        if let Err(error)=result{
            // A failing COMMIT need not mean it was rolled back. Never move
            // again on that assumption. Only the exact receipt can settle it.
            if !src.at(|db,c|{self.require(db,&c,true)?;plan.exists_at(db,op,true)}).unwrap_or(false){
                if src.at(|db,c|{self.require(db,&c,true)?;plan.exists_at(db,op,false)}).unwrap_or(false){
                    return Err(error);
                }
                return Err(io::Error::new(io::ErrorKind::ConnectionAborted,"transfer outcome unknown at the original SQL boundary"));
            }
        }
        Ok(Moved{op,from:plan.from.here.clone(),to:plan.target(op)})
    }
}

#[cfg(test)]
#[path="../../tests/unit/sqlite_transfer_io.rs"]
mod tests;

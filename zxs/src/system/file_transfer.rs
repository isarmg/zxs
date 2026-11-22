//! Durable handoff of a single authorized FILE between actual registered roots.
//! The immutable birth is never reissued. Slow data, capacity and witness
//! checks are outside SYSTEM; only the selected receipt switches placement.
use super::{Tree,Catalog,Connection,InitialReader,directory,file_create,ordinary_directory::{self,Additional,Route as DirectoryRoute}};
use super::super::{account::share_mode::Setting,file_location::{Place,Placed},transfer_material::Manifest,volume::{OrdinaryRoot,RecycleRoot},work_record::{self,ReadRecord}};
use crate::{OperationId,Role,VolumeId,display,fs::{HeldDir,invalid,directory_birth::DirectoryParent},hash::Sha256,
    identity::{AccountId,ShareId},ordinary::portable_birth::{Birth,Installed,MoveInstall,MoveRoom}};
use std::{cell::Cell,io};

#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) enum Handoff{
    Delete(Place),
    #[cfg(test)]
    Restore{deleted:OperationId,name:String,target:String},
    RestoreBound{source:Place,target:String},
    MoveOrdinary{source:Place,target:String},
}
/// The exact kind-12 SQLite direction byte and its bound volume roles.
/// The transfer and the old-source retirement must interpret these alike.
#[repr(u8)]
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) enum Direction{Delete=1,Restore=2,Ordinary=3}
impl Direction{
    pub(super) fn from_stored(tag:u8)->Option<Self>{
        match tag{
            n if n==Self::Delete.stored()=>Some(Self::Delete),
            n if n==Self::Restore.stored()=>Some(Self::Restore),
            n if n==Self::Ordinary.stored()=>Some(Self::Ordinary),
            _=>None,
        }
    }
    pub(super) fn stored(self)->u8{self as u8}
    pub(super) fn source_role(self)->Role{
        match self{Self::Delete|Self::Ordinary=>Role::File,Self::Restore=>Role::Recycle}
    }
    fn target_role(self)->Role{
        match self{Self::Delete=>Role::Recycle,Self::Restore|Self::Ordinary=>Role::File}
    }
    fn ordinary_destination(self)->bool{self.target_role()==Role::File}
}
#[derive(Clone,Debug,Eq,PartialEq)]
struct Route{
    from:Placed,kind:Direction,to:VolumeId,parent:DirectoryParent,name:String,work:Option<ReadRecord>,head:Option<OperationId>,
    share:ShareId,setting:OperationId,auth:OperationId,actor:AccountId,epoch:i64,
    destination:Option<Setting>,
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) struct Moved{pub(super) op:OperationId,pub(super) from:Place,pub(super) to:Place}
pub(super) struct OrdinaryMoveSource{
    pub(super) place:Place,pub(super) physical:Installed,pub(super) view:Option<ReadRecord>,pub(super) grant:Setting,
}
/// A decision for this held ordinary FILE only, consumed at the final
/// selected-placement COMMIT hook. Earlier durable reservations are not
/// decisions. The caller must not do filesystem, socket or SQL I/O here
/// except its existing narrow live checks; no waits may be introduced.
struct OpenedOrdinaryMove<'a>{
    original:&'a Installed,select:&'a dyn Fn()->io::Result<()>,
}
impl OpenedOrdinaryMove<'_>{
    fn before_selection(self)->io::Result<()>{(self.select)()}
}

/// A fresh ordinary request is not an invitation to adopt another wire
/// owner's durable reservation or return an old receipt. One bound request
/// may consume this ID in the common engine; there is no Clone or public ID
/// constructor. Existing callers keep their current continuation policy.
struct FreshOrdinaryOp{op:OperationId,selected:Route,from:DirectoryRoute,to:DirectoryRoute}
enum TransferStart{ContinueExisting(OperationId),Fresh(FreshOrdinaryOp)}
impl TransferStart{
    fn op(&self)->OperationId{match self{Self::ContinueExisting(op)=>*op,Self::Fresh(f)=>f.op}}
    fn is_fresh(&self)->bool{matches!(self,Self::Fresh(_))}
    fn require_selection(&self,db:&Connection,selected:&Route,from:&DirectoryRoute,to:Option<&DirectoryRoute>)->io::Result<()>{
        let Self::Fresh(bound)=self else{return Ok(());};
        if selected!=&bound.selected||from!=&bound.from||to!=Some(&bound.to){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        selected.require_fresh(db,bound.op)
    }
}
impl Moved{
    /// Rebinding the old OPEN is part of this same receipt, not an offer to
    /// upgrade it to a later credential or to another grant on the volume.
    pub(super) fn require_ordinary_origin(&self,db:&Connection,from:Setting,to:Setting,auth:OperationId,actor:AccountId)->io::Result<()>{
        if from.volume!=self.from.volume||to.volume!=self.to.volume||from.volume==to.volume||
           from.mode!=super::share_mode::Mode::ReadWrite||to.mode!=super::share_mode::Mode::ReadWrite{
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let q=db.prepare(c"SELECT m.source_backing,m.source_terminal FROM main.file_transfer m
            JOIN main.operation o ON o.op_id=m.transfer_op AND o.kind=12 AND o.state=1
            WHERE m.transfer_op=?1 AND o.actor_id=?2 AND m.auth_op=?3 AND m.direction=3
              AND m.file_id=?4 AND m.source_op=?5 AND m.source_gen=?6 AND m.source_id=?7
              AND m.source_name=?8 AND m.source_parent=?9 AND m.share_id=?10 AND m.setting_op=?11
              AND m.target_id=?12 AND m.target_name=?13 AND m.target_parent=?14 AND m.target_setting_op=?15")?;
        for (index,id) in [(1,self.op.0),(2,actor.0),(3,auth.0),(4,self.from.file.0),(5,self.from.op.0),
            (7,self.from.volume.0),(9,self.from.parent.bytes()),(10,from.share.0),(11,from.op.0),
            (12,self.to.volume.0),(14,self.to.parent.bytes()),(15,to.op.0)]{q.bind_bytes(index,&id)?;}
        q.bind_integer(6,self.from.generation)?;q.bind_bytes(8,self.from.name.as_bytes())?;
        q.bind_bytes(13,self.to.name.as_bytes())?;
        if !work_record::start_one(&q)?||Place::read_backing(&q,0,1,self.from.file,&self.from.name)?!=self.from.backing{
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        q.done()
    }
}

// Original database cut, never a callable authority from numeric inputs.
// This engine issues both native selections and updates on the same
// source; the original caller still checks its scope in every cut.
pub(super) struct TransferCut<'a>{source:&'a InitialReader,db:&'a Connection,catalog:&'a Catalog}
impl<'a> TransferCut<'a>{
    pub(super) fn parts(&self)->(&'a InitialReader,&'a Connection,&'a Catalog){
        (self.source,self.db,self.catalog)
    }
}
#[derive(Clone,Copy)]
struct Destination{setting:Setting,parent:DirectoryParent}
struct TransferAt<'a>{
    source:&'a InitialReader,setting:Setting,parent:DirectoryParent,
    destination:Option<Destination>,
    actor:AccountId,auth:OperationId,epoch:i64,
    require:&'a dyn Fn(&TransferCut<'_>)->io::Result<()>,
}
impl TransferAt<'_>{
    fn authorized(&self,db:&Connection,catalog:&Catalog)->io::Result<()>{
        (self.require)(&TransferCut{source:self.source,db,catalog})
    }
}

fn occupied()->io::Error{io::Error::new(io::ErrorKind::AlreadyExists,"destination name is already selected")}
fn pending()->io::Error{io::Error::new(io::ErrorKind::WouldBlock,"another original transfer for this FILE is unfinished")}
impl Route{
    fn select(tree:&TransferAt<'_>,db:&Connection,c:&Catalog,ask:&Handoff)->io::Result<Self>{
        tree.authorized(db,c)?;
        if tree.destination.is_some()!=matches!(ask,Handoff::MoveOrdinary{..}){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
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
            Handoff::MoveOrdinary{source,target}=>{
                display::component(target.as_bytes(),222)?;
                let dest=tree.destination.ok_or(io::ErrorKind::PermissionDenied)?;
                if source.role!=Role::File||source.volume!=f||source.parent!=tree.parent||dest.setting.volume==f||
                    Place::current(db,c,source.file)?!=*source{return Err(io::ErrorKind::NotFound.into());}
                Place::require_no_rename(db,source.file)?;
                (source.clone(),Direction::Ordinary,dest.setting.volume,target.clone())
            }
        };
        let from=place.bind(db,c)?;
        let work=work_record::selected(db,&from.birth)?;
        let head=work_record::latest_commit(db,&from.birth)?.map(|n|n.0);
        let parent=match kind{Direction::Delete=>DirectoryParent::Root,Direction::Restore=>tree.parent,
            Direction::Ordinary=>tree.destination.ok_or(io::ErrorKind::PermissionDenied)?.parent};
        Ok(Self{from,kind,to,parent,name,work,head,share:tree.setting.share,setting:tree.setting.op,auth:tree.auth,
            actor:tree.actor,epoch:tree.epoch,destination:tree.destination.map(|d|d.setting)})
    }
    fn receipt(tree:&TransferAt<'_>,db:&Connection,c:&Catalog,ask:&Handoff,op:OperationId)->io::Result<Option<Moved>>{
        tree.authorized(db,c)?;
        let q=db.prepare(c"SELECT m.file_id,m.direction,m.source_op,m.source_gen,m.source_id,m.source_name,
               m.target_id,m.target_name,m.save_op,m.manual_op,m.source_parent,m.target_parent,m.source_backing,m.source_terminal,m.target_setting_op
               FROM main.operation o JOIN main.file_transfer m ON m.transfer_op=o.op_id
               WHERE o.op_id=?1 AND o.kind=12 AND o.state=1")?;
        q.bind_bytes(1,&op.0)?;
        if !work_record::start_one(&q)?{return Ok(None);}
        let file=crate::FileId(q.array(0)?);
        let kind=Direction::from_stored(q.byte(1)?).ok_or_else(||invalid("wrong original transfer direction"))?;
        let name=display::component(q.blob(5,222)?,222)?.to_owned();
        let backing=Place::read_backing(&q,12,13,file,&name)?;
        let src=Place{file,op:OperationId(q.array(2)?),generation:q.integer(3)?,volume:VolumeId(q.array(4)?),
            role:kind.source_role(),name,parent:DirectoryParent::from_bytes(q.array(10)?),backing};
        let to=VolumeId(q.array(6)?);let name=display::component(q.blob(7,222)?,222)?.to_owned();
        let save=work_record::optional(&q,8)?;let head=work_record::optional(&q,9)?;
        let parent=DirectoryParent::from_bytes(q.array(11)?);let target_op=work_record::optional(&q,14)?;q.done()?;
        let allowed=match ask{
            Handoff::Delete(expected)=>kind==Direction::Delete&&*expected==src&&src.volume==tree.setting.volume&&src.parent==tree.parent&&parent==DirectoryParent::Root,
            #[cfg(test)]
            Handoff::Restore{deleted,name:show,target}=>kind==Direction::Restore&&*deleted==src.op&&*show==src.name&&
                target==&name&&to==tree.setting.volume&&parent==tree.parent,
            Handoff::RestoreBound{source,target}=>kind==Direction::Restore&&*source==src&&target==&name&&to==tree.setting.volume&&parent==tree.parent,
            Handoff::MoveOrdinary{source,target}=>kind==Direction::Ordinary&&*source==src&&src.volume==tree.setting.volume&&
                src.parent==tree.parent&&target==&name&&tree.destination.is_some_and(|d|
                    (d.setting.volume,d.parent,Some(d.setting.op))==(to,parent,target_op)),
        };
        if !allowed{return Err(io::ErrorKind::PermissionDenied.into());}
        let birth=file_create::committed_file(db,c,file)?;
        let work=save.map(|v|work_record::confirmed(db,&birth,v)).transpose()?;
        let original=Self{from:Placed{birth,here:src.clone()},kind,to,parent,name,work,head,
            share:tree.setting.share,setting:tree.setting.op,auth:tree.auth,actor:tree.actor,epoch:tree.epoch,
            destination:tree.destination.map(|d|d.setting)};
        if !original.exists_at(db,op,true)?{return Err(invalid("no exact result for this original transfer"));}
        Ok(Some(Moved{op,from:src,to:original.target(op)}))
    }
    fn target(&self,op:OperationId)->Place{
        Place{file:self.from.here.file,op,generation:self.from.here.generation+1,volume:self.to,
            role:self.kind.target_role(),name:self.name.clone(),parent:self.parent,backing:Some(op)}
    }
    fn geometry(&self,c:&Catalog,op:OperationId,dest:&DirectoryRoute)->io::Result<()>{
        if self.from.here.generation>i64::MAX-2{return Err(invalid("placement sequence exhausted"));}
        let r=&c.rows()[1];
        self.from.birth.validate_transfer_geometry(&self.name,(&r.path,r.limit),op)?;
        if self.kind.ordinary_destination(){
            let f=c.rows().iter().find(|r|r.role==Role::File&&r.volume==self.to)
                .ok_or_else(||invalid("handoff destination is not an ordinary enrolled volume"))?;
            let path=dest.geometry(c)?;
            if dest.parent()!=self.parent||dest.volume()!=self.to{return Err(invalid("ordinary destination parent differs"));}
            self.from.birth.validate_transfer_geometry(&self.name,(&path,f.limit),op)?;
        }
        Ok(())
    }
    fn digest(&self,op:OperationId)->io::Result<[u8;32]>{
        let b=&self.from.birth;let f=&self.from.here;
        let mut h=Sha256::new();
        if let Some(dest)=self.destination{
            h.update(b"ZXS-SQLITE-1/FILE-ORDINARY-TRANSFER\0")?;
            h.update(&dest.share.0)?;h.update(&dest.op.0)?;h.update(&dest.revision.to_be_bytes())?;
        }else{h.update(b"ZXS-SQLITE-1/FILE-TRANSFER-BACKING\0")?;}
        for v in [b.pool.0,b.operation.0,b.file.0,b.entry.0,b.history.0,b.branch.0,
            op.0,f.op.0,f.volume.0,self.to.0,f.parent.bytes(),self.parent.bytes(),self.share.0,self.setting.0,self.auth.0,self.actor.0]{h.update(&v)?;}
        h.update(&[self.kind.stored()])?;h.update(&f.generation.to_be_bytes())?;h.update(&self.epoch.to_be_bytes())?;
        for n in [&f.name,&self.name]{h.update(&[n.len() as u8])?;h.update(n.as_bytes())?;}
        for k in [f.backing,self.work.map(|v|v.record.key().save),self.head]{
            h.update(&[u8::from(k.is_some())])?;if let Some(k)=k{h.update(&k.0)?;}
        }
        Ok(h.finish())
    }
    fn exists_at(&self,db:&Connection,op:OperationId,committed:bool)->io::Result<bool>{
        let q=db.prepare(c"SELECT o.kind,o.state,o.actor_id,o.request_sha,m.file_id,m.direction,m.source_op,m.source_gen,
          m.source_id,m.source_name,m.target_id,m.target_name,m.share_id,m.setting_op,m.auth_op,m.save_op,m.manual_op,m.source_parent,m.target_parent,m.source_backing,m.source_terminal,m.target_setting_op
          FROM main.operation o LEFT JOIN main.file_transfer m ON m.transfer_op=o.op_id WHERE o.op_id=?1")?;
        q.bind_bytes(1,&op.0)?;
        if !work_record::start_one(&q)?{return Ok(false);}
        let f=&self.from.here;
        if q.byte(0)?!=12||q.byte(1)?!=u8::from(committed)||q.array::<16>(2)?!=self.actor.0||q.array::<32>(3)?!=self.digest(op)?||
            q.array::<16>(4)?!=f.file.0||q.byte(5)?!=self.kind.stored()||q.array::<16>(6)?!=f.op.0||q.integer(7)?!=f.generation||
            q.array::<16>(8)?!=f.volume.0||q.blob(9,222)?!=f.name.as_bytes()||q.array::<16>(10)?!=self.to.0||q.blob(11,222)?!=self.name.as_bytes()||
            q.array::<16>(12)?!=self.share.0||q.array::<16>(13)?!=self.setting.0||q.array::<16>(14)?!=self.auth.0||
            work_record::optional(&q,15)?!=self.work.map(|r|r.record.key().save)||work_record::optional(&q,16)?!=self.head||
            q.array::<16>(17)?!=f.parent.bytes()||q.array::<16>(18)?!=self.parent.bytes()||
            Place::read_backing(&q,19,20,f.file,&f.name)?!=f.backing||
            work_record::optional(&q,21)?!=self.destination.map(|d|d.op){
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
    fn require_fresh(&self,db:&Connection,op:OperationId)->io::Result<()>{
        if self.kind!=Direction::Ordinary{return Err(io::ErrorKind::PermissionDenied.into());}
        if self.reservation(db)?.is_some(){return Err(pending());}
        let q=db.prepare(c"SELECT 1 FROM main.operation WHERE op_id=?1")?;
        q.bind_bytes(1,&op.0)?;
        if work_record::start_one(&q)?{return Err(io::Error::new(io::ErrorKind::WouldBlock,"this ordinary move Op is already recorded"));}
        Ok(())
    }
    fn release_unselected(&self,db:&Connection,op:OperationId,why:u8)->io::Result<()>{
        let q=db.prepare(c"INSERT INTO main.file_transfer_void(transfer_op,setting_op,auth_op,reason) VALUES(?1,?2,?3,?4)")?;
        q.bind_bytes(1,&op.0)?;q.bind_bytes(2,&self.setting.0)?;q.bind_bytes(3,&self.auth.0)?;
        q.bind_integer(4,why.into())?;q.done()
    }
    fn vacant(&self,db:&Connection,except:Option<OperationId>)->io::Result<()>{
        if !self.kind.ordinary_destination(){return Ok(());}
        if Place::renaming_name(db,self.to,self.parent,&self.name)?{return Err(pending());}
        let q=db.prepare(c"SELECT 1 FROM main.file_location WHERE role=2 AND volume_id=?1 AND display=?2 AND parent_id=?3
          UNION ALL SELECT 1 FROM main.directory_slot WHERE volume_id=?1 AND display=?2 AND parent_id=?3 LIMIT 1")?;
        q.bind_bytes(1,&self.to.0)?;q.bind_bytes(2,self.name.as_bytes())?;q.bind_bytes(3,&self.parent.bytes())?;
        if work_record::start_one(&q)?{return Err(occupied());}
        let q=db.prepare(c"SELECT transfer_op FROM main.pending_transfer WHERE direction IN (2,3) AND target_id=?1 AND target_name=?2 AND target_parent=?3")?;
        q.bind_bytes(1,&self.to.0)?;q.bind_bytes(2,self.name.as_bytes())?;q.bind_bytes(3,&self.parent.bytes())?;
        if work_record::start_one(&q)?&&Some(OperationId(q.array(0)?))!=except{return Err(pending());}
        Ok(())
    }
    fn issue(&self,db:&Connection,op:OperationId)->io::Result<()>{
        let p=&self.from.here;
        let q=db.prepare(c"INSERT INTO main.file_transfer(transfer_op,file_id,direction,source_op,source_gen,source_id,source_name,
            target_id,target_name,share_id,setting_op,auth_op,save_op,manual_op,source_parent,target_parent,source_backing,source_terminal,target_setting_op)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)")?;
        q.bind_bytes(1,&op.0)?;q.bind_bytes(2,&p.file.0)?;q.bind_integer(3,i64::from(self.kind.stored()))?;
        q.bind_bytes(4,&p.op.0)?;q.bind_integer(5,p.generation)?;q.bind_bytes(6,&p.volume.0)?;q.bind_bytes(7,p.name.as_bytes())?;
        q.bind_bytes(8,&self.to.0)?;q.bind_bytes(9,self.name.as_bytes())?;
        for (i,x) in [self.share.0,self.setting.0,self.auth.0].iter().enumerate(){q.bind_bytes(i as i32+10,x)?;}
        if let Some(work)=self.work{q.bind_bytes(13,&work.record.key().save.0)?;}if let Some(head)=self.head{q.bind_bytes(14,&head.0)?;}
        q.bind_bytes(15,&p.parent.bytes())?;q.bind_bytes(16,&self.parent.bytes())?;
        if let Some(op)=p.backing{q.bind_bytes(17,&op.0)?;}else{q.bind_bytes(18,p.leaf()?.to_bytes())?;}
        if let Some(dest)=self.destination{q.bind_bytes(19,&dest.op.0)?;}
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

/// Each endpoint is the held exact original marker and parent chain.
/// Ordinary moves don't open R or silently copy between two views of one
/// mounted backing. The existing independent R assertions are unchanged.
enum Passage<'r>{
    Retained{ordinary:OrdinaryRoot<'r>,recycle:RecycleRoot<'r>,from_ordinary:bool},
    Ordinary{source:OrdinaryRoot<'r>,target:OrdinaryRoot<'r>},
}
impl<'r> Passage<'r>{
    fn open(src:&'r InitialReader,c:&Catalog,from_role:Role,first:&DirectoryRoute,second:Option<&DirectoryRoute>)->io::Result<Self>{
        let ordinary=OrdinaryRoot::at_parent(src,c,first,None)?;
        if let Some(second)=second{
            if from_role!=Role::File||first.volume()==second.volume(){return Err(invalid("not two original ordinary volumes"));}
            let target=OrdinaryRoot::at_parent(src,c,second,None)?;
            let a=ordinary.folder();let b=target.folder();
            if a.same_mount(b)?||a.entity.device==b.entity.device||!a.distinct_filesystems(b)?{
                return Err(io::Error::new(io::ErrorKind::Unsupported,"distinct ordinary filesystem mounts required for this indexed copy"));
            }
            return Ok(Self::Ordinary{source:ordinary,target});
        }
        let recycle=RecycleRoot::at(src,c)?;recycle.separate_from(&ordinary)?;
        Ok(Self::Retained{ordinary,recycle,from_ordinary:from_role==Role::File})
    }
    fn require(&self)->io::Result<()>{match self{
        Self::Ordinary{source,target}=>{
            // Both held mount identities must still be the exact named
            // roots/chains. An open mount cannot acquire a new superblock.
            source.require_streaming()?;target.require_streaming()?;
            if super::super::volume::measure(source.folder())?.read_only||super::super::volume::measure(target.folder())?.read_only{
                return Err(io::Error::new(io::ErrorKind::PermissionDenied,"ordinary movement needs both mounted sides writable"));
            }
            Ok(())
        },
        Self::Retained{ordinary,recycle,..}=>{
            ordinary.require_streaming()?;recycle.require()?;recycle.separate_from(ordinary)
        },
    }}
    fn physical(&self)->(&HeldDir,&HeldDir){match self{
        Self::Ordinary{source,target}=>(source.folder(),target.folder()),
        Self::Retained{ordinary,recycle,from_ordinary:true}=>(ordinary.folder(),recycle.root_for_read()),
        Self::Retained{ordinary,recycle,from_ordinary:false}=>(recycle.root_for_read(),ordinary.folder()),
    }}
    fn ordinary_destination(&self)->Option<&OrdinaryRoot<'r>>{match self{
        Self::Ordinary{target,..}=>Some(target),
        Self::Retained{ordinary,from_ordinary:false,..}=>Some(ordinary),
        Self::Retained{from_ordinary:true,..}=>None,
    }}
}
impl Tree<'_,'_>{
    fn with_transfer<T>(&self,target:Option<&Tree<'_,'_>>,act:impl FnOnce(TransferAt<'_>)->io::Result<T>)->io::Result<T>{
        if target.is_some_and(|t|!std::ptr::eq(t.session,self.session)||t.setting.volume==self.setting.volume){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let source=self.session.source;
        let prior=&self.session.original;
        let require=|cut:&TransferCut<'_>|{
            debug_assert!(std::ptr::eq(cut.source,source)); // the same owner supplies both connections
            self.require(cut.db,cut.catalog,true)?;
            if let Some(t)=target{t.require(cut.db,cut.catalog,true)?;}
            Ok(())
        };
        act(TransferAt{source,setting:self.setting,parent:self.parent,destination:target.map(|t|Destination{setting:t.setting,parent:t.parent}),
            actor:prior.account,auth:prior.op,epoch:prior.generation,require:&require})
    }
    pub(super) fn transfer(&self,ask:&Handoff,supplied:OperationId,live:impl Fn()->io::Result<()>)->io::Result<Moved>{
        self.with_transfer(None,|at|at.transfer_inner(ask,supplied,None,live,false,|_|Ok(())))
    }
    pub(super) fn transfer_last_close(&self,source:Place,supplied:OperationId,live:impl Fn()->io::Result<()>)->io::Result<Moved>{
        self.with_transfer(None,|at|at.transfer_inner(&Handoff::Delete(source),supplied,None,live,true,|_|Ok(())))
    }
    pub(super) fn transfer_ordinary_to(&self,target:&Tree<'_,'_>,opened:&OrdinaryMoveSource,new_name:String,op:OperationId,
        live:impl Fn()->io::Result<()>)->io::Result<Moved>{
        if self.setting!=opened.grant{return Err(io::ErrorKind::PermissionDenied.into());}
        // Native keeps the same original owner and endpoint check at each
        // pending cut and again immediately before selecting placement.
        let original=OpenedOrdinaryMove{original:&opened.physical,select:&live};
        self.with_transfer(Some(target),|at|at.transfer_inner(
            &Handoff::MoveOrdinary{source:opened.place.clone(),target:new_name},op,Some(original),&live,false,|_|Ok(())))
    }
    #[cfg(test)]
    fn transfer_after(&self,ask:&Handoff,supplied:OperationId,live:impl Fn()->io::Result<()>,
        point:impl FnMut(Cut)->io::Result<()>)->io::Result<Moved>{
        self.with_transfer(None,|at|at.transfer_inner(ask,supplied,None,live,false,point))
    }
    #[cfg(test)]
    fn ordinary_after(&self,target:&Tree<'_,'_>,opened:&OrdinaryMoveSource,name:String,op:OperationId,
        point:impl FnMut(Cut)->io::Result<()>,select:impl Fn()->io::Result<()>)->io::Result<Moved>{
        if self.setting!=opened.grant{return Err(io::ErrorKind::PermissionDenied.into());}
        let original=OpenedOrdinaryMove{original:&opened.physical,select:&select};
        self.with_transfer(Some(target),|at|at.transfer_inner(
            &Handoff::MoveOrdinary{source:opened.place.clone(),target:name},op,Some(original),||Ok(()),false,point))
    }
}
impl TransferAt<'_>{
    fn transfer_inner(&self,ask:&Handoff,supplied:OperationId,opened:Option<OpenedOrdinaryMove<'_>>,live:impl Fn()->io::Result<()>,
        wait_last:bool,point:impl FnMut(Cut)->io::Result<()>)->io::Result<Moved>{
        self.transfer_started(ask,TransferStart::ContinueExisting(supplied),opened,live,wait_last,point)
    }
    fn transfer_started(&self,ask:&Handoff,start:TransferStart,opened:Option<OpenedOrdinaryMove<'_>>,live:impl Fn()->io::Result<()>,
        wait_last:bool,mut point:impl FnMut(Cut)->io::Result<()>)->io::Result<Moved>{
        let supplied=start.op();let fresh=start.is_fresh();
        if supplied.0==[0;16]||supplied.0==[0xff;16]{return Err(invalid("reserved transfer operation ID"));}
        if self.destination.is_some()!=opened.is_some()||fresh&&(opened.is_none()||wait_last){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let src=self.source;
        live()?;
        if !fresh&&let Some(done)=src.at(|db,c|Route::receipt(self,db,&c,ask,supplied))?{live()?;return Ok(done);}
        let (c,from_position,ordinary_parent,second_parent)=src.at(|db,c|{
            let original=Route::select(self,db,&c,ask)?;
            let parent=DirectoryRoute::at(db,&c,self.setting.volume,self.parent)?;
            let second=self.destination.map(|d|DirectoryRoute::at(db,&c,d.setting.volume,d.parent)).transpose()?;
            start.require_selection(db,&original,&parent,second.as_ref())?;
            Ok((c,original.from,parent,second))
        })?;
        live()?;
        let passage=Passage::open(src,&c,from_position.here.role,&ordinary_parent,second_parent.as_ref())?;
        let gates=||->io::Result<()>{live()?;passage.require()};
        let (from,to)=passage.physical();
        let target_parent=second_parent.as_ref().unwrap_or(&ordinary_parent);
        let require_routes=|db:&Connection,c:&Catalog|->io::Result<()>{
            // Both placements are selected prior to any filesystem I/O.
            if let Some(d)=self.destination{
                if DirectoryRoute::at(db,c,self.setting.volume,self.parent)?!=ordinary_parent||
                    DirectoryRoute::at(db,c,d.setting.volume,d.parent)?!=*target_parent{
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
            }
            Ok(())
        };
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
        let original_file=opened.as_ref().map(|m|m.original);
        let original_source=|| match original_file{
            Some(h)=>h.require_copy_source(from,&real),
            None=>real.require(from),
        };
        original_source()?;
        let (plan,op,retry,retire,alias,members,material)=src.at(|db,c|{
            let plan=Route::select(self,db,&c,ask)?;
            if plan.from!=from_position{return Err(io::ErrorKind::NotFound.into());}
            require_routes(db,&c)?;
            start.require_selection(db,&plan,&ordinary_parent,second_parent.as_ref())?;
            let existing=if fresh{None}else{plan.reservation(db)?};
            let retry=existing.filter(|p|p.1).map(|p|p.0);
            let retire=existing.filter(|p|!p.1).map(|p|p.0);
            let op=retry.unwrap_or(supplied);
            if retire==Some(op){return Err(invalid("an invalidated transfer cannot be reissued under its original ID"));}
            if let Some(p)=retry{plan.still_live(db,p)?;}
            plan.vacant(db,existing.map(|p|p.0))?;plan.geometry(&c,op,target_parent)?;
            let alias=if plan.kind.ordinary_destination(){file_create::display_owner_at(db,&c,plan.to,plan.parent,&plan.name)?}else{None};
            let members=work_record::members(db,&c,&plan.from.birth)?;
            let material=Manifest::select(db,&plan.from,plan.work)?;
            Ok((plan,op,retry.is_some(),retire,alias,members,material))
        })?;
        let unchanged=|db:&Connection,c:&Catalog|->io::Result<()>{
            if Route::select(self,db,c,ask)?!=plan{return Err(io::ErrorKind::PermissionDenied.into());}
            require_routes(db,c)
        };
        members.require(&crate::fs::WitnessRead::at(real.directory())?)?;
        // Do not reserve a target at all when an allowed unselected lone
        // source link makes retirement unprovable. This is held FILE EX
        // and does not read, alter or repair that old residue.
        let ordinary_source=if plan.kind==Direction::Ordinary{
            Some(real.capture_retirement(from)?.encode())
        }else{None};
        let pending_admission=||if plan.kind==Direction::Ordinary{live()}else{Ok(())};
        if let Some(old)=retire{
            // Keep potentially real old stage bytes untouched. Only the
            // exact reservation from an invalidated grant can be released.
            gates()?;original_source()?;
            let w=Connection::open(&src.dir,true)?;
            src.transaction_before_commit(&w,c"BEGIN IMMEDIATE",|db,c|{
                unchanged(db,&c)?;live()?;plan.release_unselected(db,old,1)
            },&pending_admission)?;
        }
        let within_authority=|db:&Connection,c:&Catalog|self.authorized(db,c);
        let need=material.space(src,&plan.from,within_authority,&live)?;
        gates()?;
        let alias_at=||{
            if let Some(target)=passage.ordinary_destination(){directory::check_alias_name(target,&plan.name,alias.as_ref(),None)}
            else{Ok(None)}
        };
        alias_at()?;
        let locked=HeldDir::trusted_storage(&to.path)?;locked.exclusive()?;
        if locked.entity!=to.entity||!locked.same_mount(to)?{return Err(invalid("transfer destination changed before acquiring namespace"));}
        gates()?;original_source()?;alias_at()?;
        let room=MoveInstall::room(&locked,op)?;
        if !retry&&room==MoveRoom::Present{return Err(io::ErrorKind::AlreadyExists.into());}
        if room==MoveRoom::Full||(room==MoveRoom::Vacant&&space(&locked)?<need){
            if retry{
                let w=Connection::open(&src.dir,true)?;
                src.transaction_before_commit(&w,c"BEGIN IMMEDIATE",|db,c|{
                    unchanged(db,&c)?;plan.still_live(db,op)?;live()?;plan.release_unselected(db,op,2)
                },&pending_admission)?;
            }
            return Err(io::ErrorKind::StorageFull.into());
        }
        // In particular no CREATE, syscall reaching F/R, or peer wait
        // takes place in either short SQLite writer section.
        if !retry{
            let census=if plan.kind.ordinary_destination(){Some(locked.names()?.into_iter().collect())}else{None};
            let pocket=Birth::placement_parent(op)?;
            let w=Connection::open(&src.dir,true)?;
            let issued_here=Cell::new(false);
            let issued=src.transaction_before_commit(&w,c"BEGIN IMMEDIATE",|db,c|{
                unchanged(db,&c)?;plan.vacant(db,None)?;
                if fresh{plan.require_fresh(db,op)?;}
                else if plan.reservation(db)?.is_some(){return Err(pending());}
                if let Some(names)=&census{ordinary_directory::budget(db,plan.to,plan.parent,names,Some(Additional::Pocket(&pocket)))?;}
                live()?;plan.issue(db,op)?;issued_here.set(true);Ok(())
            },&pending_admission);
            drop(w);
            if let Err(err)=issued{
                if fresh{
                    // Never treat an outside row observed after a failed new
                    // admission as ours. If this invocation passed issue,
                    // leave its possible reservation for exact reconciliation.
                    return Err(if issued_here.get(){io::Error::new(io::ErrorKind::ConnectionAborted,
                        "fresh ordinary move reservation outcome is unknown")}else{err});
                }
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
                    gates()?;original_source()?;
                    let w=Connection::open(&src.dir,true)?;
                    src.transaction_before_commit(&w,c"BEGIN IMMEDIATE",|db,c|{
                        unchanged(db,&c)?;plan.still_live(db,op)?;live()?;plan.release_unselected(db,op,2)
                    },&pending_admission)?;
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
                    gates()?;original_source()?;
                    if candidate.pair_incomplete(&error,&root)?{
                        let w=Connection::open(&src.dir,true)?;
                        src.transaction_before_commit(&w,c"BEGIN IMMEDIATE",|db,c|{
                            unchanged(db,&c)?;plan.still_live(db,op)?;live()?;plan.release_unselected(db,op,2)
                        },&pending_admission)?;
                    }
                }
                return Err(error);
            }
        };
        candidate.finish(to,&names)?;point(Cut::CandidateSynced)?;
        members.require(&crate::fs::WitnessRead::at(real.directory())?)?;
        // Capture actual two-link source proofs before the switch. An
        // unpaired old residue is preserved, never made part of the target.
        let retirement=if let Some(expected)=ordinary_source{
            if real.capture_retirement(from)?.encode()!=expected{
                return Err(invalid("old ordinary FILE retirement evidence changed during the handoff"));
            }
            Some(expected)
        }else{real.capture_retirement(from).ok().map(|p|p.encode())};
        let target_proof=candidate.retirement_target(to,&target_used)?.encode();
        point(Cut::GuardCaptured)?;
        source_used.require(real.directory())?;target_used.require(candidate.body())?;
        // Neither target occupancy nor membership was held across the
        // bulk copy. Reacquire its short namespace lock at the final cut.
        let final_root=HeldDir::trusted_storage(&to.path)?;final_root.exclusive()?;
        if final_root.entity!=to.entity||!final_root.same_mount(to)?{return Err(invalid("target changed before final transfer selection"));}
        gates()?;original_source()?;candidate.require_members(&final_root,&names)?;
        let _alias=alias_at()?; // original held native alias until exact SQL switch
        let w=Connection::open(&src.dir,true)?;
        // Only this transaction selects the new placement and publishes
        // old-source retirement evidence. A final pre-COMMIT refusal
        // rolls it back, without pretending any staged pocket is gone.
        let result=src.transaction_before_commit(&w,c"BEGIN IMMEDIATE",|db,c|{
            unchanged(db,&c)?;plan.still_live(db,op)?;plan.vacant(db,Some(op))?;
            if plan.kind.ordinary_destination()&&file_create::display_owner_at(db,&c,plan.to,plan.parent,&plan.name)?!=alias{
                return Err(invalid("ordinary destination alias evidence changed"));
            }
            live()?;
            let st=db.prepare(c"UPDATE main.operation SET state=1 WHERE op_id=?1 AND kind=12 AND state=0")?;
            st.bind_bytes(1,&op.0)?;st.done()?;drop(st);
            super::retirement::record(db,op,retirement.as_deref(),&target_proof)?;
            if !plan.exists_at(db,op,true)?{return Err(invalid("transfer did not select the original receipt"));}
            Ok(())
        },move||opened.map_or(Ok(()),OpenedOrdinaryMove::before_selection));
        drop(w);
        if let Err(error)=result{
            // A failing COMMIT need not mean it was rolled back. Never move
            // again on that assumption. Only the exact receipt can settle it.
            if !src.at(|db,c|{self.authorized(db,&c)?;plan.exists_at(db,op,true)}).unwrap_or(false){
                if src.at(|db,c|{self.authorized(db,&c)?;plan.exists_at(db,op,false)}).unwrap_or(false){
                    return Err(error);
                }
                return Err(io::Error::new(io::ErrorKind::ConnectionAborted,"transfer outcome unknown at the original SQL boundary"));
            }
        }
        Ok(Moved{op,from:plan.from.here.clone(),to:plan.target(op)})
    }
}


#[cfg(test)]
#[test]
fn stored_transfer_directions_bind_the_exact_source_and_target_roles(){
    use crate::Role::{File,Recycle};
    let expected=[
        (1,Direction::Delete,File,Recycle),
        (2,Direction::Restore,Recycle,File),
        (3,Direction::Ordinary,File,File),
    ];
    for (tag,direction,source,target) in expected{
        assert_eq!(Direction::from_stored(tag),Some(direction));
        assert_eq!(direction.stored(),tag);
        assert_eq!((direction.source_role(),direction.target_role()),(source,target));
        assert_eq!(direction.ordinary_destination(),target==File);
    }
    for tag in (u8::MIN..=u8::MAX).filter(|tag|!(1..=3).contains(tag)){
        assert_eq!(Direction::from_stored(tag),None);
    }
}

#[path="smb_rust_transfer.rs"]
mod rust;
pub(crate) use rust::{ConfirmedFileDelete,RustFileDelete,RustRetainedRestore};

#[path="smb_rust_ordinary_move.rs"]
mod ordinary_move;
pub(crate) use ordinary_move::RustOrdinaryMove;

#[cfg(test)]
#[path="../../tests/unit/sqlite_transfer_io.rs"]
mod tests;

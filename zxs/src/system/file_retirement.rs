//! Finish a prior confirmed transfer, not a new SMB or maintenance action.
//! The only source unlink plan was captured from the exact dual-witness
//! original at the moment that USER-selected transfer became durable.
use super::{Catalog,Connection,InitialReader,file_create,file_transfer::Direction,ordinary_directory::Route};
use super::super::{file_location::Place,volume::{OrdinaryRoot,RecycleRoot},work_record};
use crate::{FileId,OperationId,Role,VolumeId,display,fs::{HeldDir,RetireCut,RetireProof,RetireSite,MAX_RETIRE_PROOF,invalid,directory_birth::DirectoryParent},ordinary::portable_birth::Birth};
use std::{cell::Cell,io,thread,time::Duration};

pub(super) fn record(db:&Connection,op:OperationId,proof:Option<&[u8]>,target:&[u8])->io::Result<()>{
    let q=db.prepare(c"INSERT INTO main.file_transfer_source(transfer_op,guard,target_guard,status) VALUES(?1,?2,?3,?4)")?;
    q.bind_bytes(1,&op.0)?;
    if let Some(p)=proof{q.bind_bytes(2,p)?;}
    q.bind_bytes(3,target)?;
    q.bind_integer(4,if proof.is_some(){0}else{2})?;q.done()
}

struct Ticket{seq:i64,op:OperationId,source:Place,direction:Direction,target:VolumeId,parent:DirectoryParent,names:Vec<u8>}
fn next(src:&InitialReader,after:i64)->io::Result<Option<Ticket>>{
    src.at(|db,_|{
        let mut q=db.prepare(c"SELECT r.seq,r.transfer_op,r.guard,m.file_id,m.source_op,m.source_gen,m.source_id,m.source_name,
            m.direction,m.target_id,m.source_parent,m.target_parent,m.source_backing,m.source_terminal
            FROM main.file_transfer_source r JOIN main.file_transfer m ON m.transfer_op=r.transfer_op
            JOIN main.operation o ON o.op_id=r.transfer_op AND o.kind=12 AND o.state=1
            WHERE r.status=0 AND r.seq>?1 ORDER BY r.seq LIMIT 1")?;
        q.bind_integer(1,after)?;
        if !work_record::start_one(&q)?{return Ok(None);}
        let seq=q.integer(0)?;let op=OperationId(q.array(1)?);let row=q.row()?;
        let names=row.blob(2,MAX_RETIRE_PROOF)?.to_vec();
        let direction=Direction::from_stored(row.byte(8)?).ok_or_else(||invalid("wrong completed transfer direction"))?;
        let file=FileId(row.array(3)?);let name=display::component(row.blob(7,222)?,222)?.to_owned();
        let backing=Place::read_backing(&q,12,13,file,&name)?;
        let source=Place{file,op:OperationId(q.array(4)?),generation:q.integer(5)?,
            volume:VolumeId(q.array(6)?),name,role:direction.source_role(),parent:DirectoryParent::from_bytes(q.array(10)?),backing};
        let target=VolumeId(q.array(9)?);let parent=DirectoryParent::from_bytes(q.array(11)?);q.done()?;
        Ok(Some(Ticket{seq,op,source,direction,target,parent,names}))
    })
}
impl Ticket{
    fn current(&self,db:&Connection,c:&Catalog)->io::Result<Place>{
        let q=db.prepare(c"SELECT 1 FROM main.file_transfer_source r JOIN main.operation o ON o.op_id=r.transfer_op
          JOIN main.file_transfer m ON m.transfer_op=r.transfer_op
          WHERE r.seq=?1 AND r.transfer_op=?2 AND r.status=0 AND o.kind=12 AND o.state=1
            AND m.file_id=?3 AND m.source_op=?4 AND m.source_gen=?5 AND m.source_id=?6 AND m.source_name=?7
            AND m.target_id=?8 AND m.direction=?9 AND m.source_parent=?10 AND m.target_parent=?11")?;
        q.bind_integer(1,self.seq)?;q.bind_bytes(2,&self.op.0)?;q.bind_bytes(3,&self.source.file.0)?;
        q.bind_bytes(4,&self.source.op.0)?;q.bind_integer(5,self.source.generation)?;q.bind_bytes(6,&self.source.volume.0)?;
        q.bind_bytes(7,self.source.name.as_bytes())?;q.bind_bytes(8,&self.target.0)?;
        q.bind_integer(9,i64::from(self.direction.stored()))?;
        q.bind_bytes(10,&self.source.parent.bytes())?;q.bind_bytes(11,&self.parent.bytes())?;
        if !work_record::start_one(&q)?{return Err(invalid("retirement is no longer the original confirmed transfer"));}
        q.done()?;
        let p=Place::current(db,c,self.source.file)?;
        Place::require_no_rename(db,p.file)?;
        if p.generation<=self.source.generation||
           (p.generation==self.source.generation+1&&(p.op!=self.op||p.volume!=self.target||p.parent!=self.parent)){
            return Err(invalid("no descendant selected placement for retirement"));
        }
        Ok(p)
    }
}
pub(super) fn applied_target(db:&Connection,place:&Place)->io::Result<RetireProof>{
    let backing=place.backing.ok_or_else(||invalid("no physical transfer outputs this selected FILE"))?;
    let mut q=db.prepare(c"SELECT r.target_guard FROM main.file_transfer_source r
        JOIN main.file_transfer m ON m.transfer_op=r.transfer_op
        JOIN main.operation o ON o.op_id=m.transfer_op AND o.kind=12 AND o.state=1
        JOIN main.file_location l ON l.file_id=m.file_id AND l.backing_op=m.transfer_op
        JOIN main.operation h ON h.op_id=l.position_op AND h.state=1
        WHERE r.transfer_op=?1 AND l.position_op=?2 AND l.file_id=?3
          AND m.target_id=l.volume_id AND (CASE m.direction WHEN 1 THEN 1 ELSE 2 END)=l.role
          AND ((h.kind=12 AND m.target_name=l.display AND m.target_parent=l.parent_id AND m.source_gen+1=l.generation)
               OR (h.kind=14 AND l.role=2 AND m.source_gen+1<l.generation
                   AND EXISTS(SELECT 1 FROM main.file_rename z WHERE z.rename_op=h.op_id
                       AND z.file_id=l.file_id AND z.volume_id=l.volume_id AND z.target_parent=l.parent_id
                       AND z.target_name=l.display AND z.backing_op=m.transfer_op)))")?;
    q.bind_bytes(1,&backing.0)?;q.bind_bytes(2,&place.op.0)?;q.bind_bytes(3,&place.file.0)?;
    if !work_record::start_one(&q)?{return Err(invalid("no genuine physical confirmation of current retirement target"));}
    let result=RetireProof::decode(q.row()?.blob(0,MAX_RETIRE_PROOF)?)?;q.done()?;Ok(result)
}
enum Volume<'a>{Ordinary(OrdinaryRoot<'a>),Retained(RecycleRoot<'a>)}
impl<'a> Volume<'a>{
    fn open(src:&'a InitialReader,c:&Catalog,p:&Place,route:Option<&Route>)->io::Result<Self>{
        match p.role{
            Role::File=>{
                let dir=route.filter(|r|r.volume()==p.volume&&r.parent()==p.parent).ok_or_else(||invalid("wrong actual retirement parent"))?;
                Ok(Self::Ordinary(OrdinaryRoot::at_parent(src,c,dir,None)?))
            },
            Role::Recycle=>{
                let r=RecycleRoot::at(src,c)?;
                if r.volume()!=p.volume{return Err(invalid("different original recycle role"));}
                Ok(Self::Retained(r))
            },
            _=>Err(invalid("invalid source retirement role")),
        }
    }
    fn root(&self)->&HeldDir{match self{Self::Ordinary(f)=>f.folder(),Self::Retained(r)=>r.root_for_read()}}
    fn require(&self)->io::Result<()>{match self{Self::Ordinary(f)=>f.require_streaming(),Self::Retained(r)=>r.require()}}
    fn distinct_roles(&self,other:&Self)->io::Result<()>{
        match (self,other){
            (Self::Ordinary(f),Self::Retained(r))|(Self::Retained(r),Self::Ordinary(f))=>r.separate_from(f),
            _=>Ok(()), // multiple prior placements may ultimately return to the same volume
        }
    }
}
type Allowed<'a>=dyn Fn(&Connection,&Catalog,&Place,&Place)->io::Result<()>+'a;
impl Ticket{
    fn selected(&self,db:&Connection,c:&Catalog,allowed:&Allowed<'_>)->io::Result<Place>{
        let current=self.current(db,c)?;
        allowed(db,c,&self.source,&current)?;
        Ok(current)
    }
}
fn run(src:&InitialReader,t:&Ticket,allowed:&Allowed<'_>,live:&impl Fn()->io::Result<()>,
    point:&impl Fn(RetireCut)->io::Result<()>)->io::Result<()>{
    live()?;
    let proof=RetireProof::decode(&t.names)?;
    let (c,birth,selected,members,from_parent,to_parent)=src.at(|db,c|{
        let p=t.selected(db,&c,allowed)?;
        let b=file_create::committed_file(db,&c,p.file)?;
        let m=work_record::members(db,&c,&b)?;
        let parent=|place:&Place| if place.role==Role::File{Route::at(db,&c,place.volume,place.parent).map(Some)}else{Ok(None)};
        let old=parent(&t.source)?;let new=parent(&p)?;
        Ok((c,b,p,m,old,new))
    })?;
    let source=Volume::open(src,&c,&t.source,from_parent.as_ref())?;
    let has_parent=t.source.backing.is_some();
    let portable=t.source.leaf()?;
    let witness=birth.witness()?;let header=birth.header();
    let original=t.source.private_leaf()?.unwrap_or_else(||portable.clone());
    let isolated=Birth::retirement_name(t.op)?;
    let site=RetireSite{root:source.root(),original:&original,isolation:&isolated,portable:&portable,witness:&witness,header:&header,has_parent};
    let old=proof.lease(&site)?; // no target I/O / acquisition while a former reader remains
    let target=Volume::open(src,&c,&selected,to_parent.as_ref())?;
    source.distinct_roles(&target)?;
    let target_original=super::super::file_location::Placed{birth:birth.clone(),here:selected.clone()}
        .open(target.root(),&target.root().names()?.into_iter().collect(),false,|w|members.require(w))?;
    if source.root().same_mount(target.root())?&&proof.is_body(target_original.directory()){
        return Err(invalid("selected target is not distinct from the old physical FILE"));
    }
    let installed=src.at(|db,c|{
        if t.selected(db,&c,allowed)?!=selected{return Err(invalid("retirement target moved"));}
        applied_target(db,&selected)
    })?;
    target_original.still_retirement_target(target.root(),&installed)?;
    let observed=Cell::new(None);
    let gate=||->io::Result<()>{
        live()?;source.require()?;target.require()?;target_original.require(target.root())?;
        let version=super::smb_buffer::data_version(src)?;
        if observed.get()!=Some(version){
            src.at(|db,c|if t.selected(db,&c,allowed)?==selected{Ok(())}else{Err(invalid("retirement target moved"))})?;
            target_original.still_retirement_target(target.root(),&installed)?;
            observed.set(Some(version));
        }
        Ok(())
    };
    let last_cut=||->io::Result<()>{
        src.at(|db,c|if t.selected(db,&c,allowed)?==selected{Ok(())}else{Err(invalid("retirement target moved"))})?;
        gate()
    };
    let root=HeldDir::trusted_storage(&source.root().path)?;
    root.exclusive()?;
    if root.entity!=source.root().entity||!root.same_mount(source.root())?{return Err(invalid("wrong original source retirement parent"));}
    last_cut()?;
    proof.isolate(&RetireSite{root:&root,..site},old.as_ref(),&gate,live,point)?;
    drop(root); // siblings need no root lock during bounded per-file link removal
    last_cut()?;
    proof.remove(&site,old.as_ref(),&gate,live,point)?;
    gate()?;
    let w=Connection::open(&src.dir,true)?;
    src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
        if t.selected(db,&c,allowed)?!=selected{return Err(invalid("different selected target at source completion"));}
        live()?;
        let q=db.prepare(c"UPDATE main.file_transfer_source SET guard=NULL,status=1 WHERE seq=?1 AND transfer_op=?2 AND status=0")?;
        q.bind_integer(1,t.seq)?;q.bind_bytes(2,&t.op.0)?;q.done()
    })
}
pub(super) fn serve(src:&InitialReader,live:impl Fn()->io::Result<()>){
    serve_scoped(src,|_,_,_,_|Ok(()),live)
}
/// The exact existing work, with an optional family-specific restriction
/// at each SQL selection. No caller can supply a cleanup ticket.
pub(super) fn serve_scoped(src:&InitialReader,allowed:impl Fn(&Connection,&Catalog,&Place,&Place)->io::Result<()>,
    live:impl Fn()->io::Result<()>){
    let mut after=0;
    while live().is_ok(){
        let pause=match next(src,after){
            Ok(Some(t))=>{after=t.seq;let _=run(src,&t,&allowed,&live,&|_|Ok(()));250},
            _=>{after=0;1000},
        };
        // Avoid spinning on a genuinely open old FILE, an offline R, a
        // busy current writer or an independently damaged physical proof.
        thread::sleep(Duration::from_millis(pause));
    }
}
#[cfg(test)]
pub(super) fn one_for_test(src:&InitialReader,point:impl Fn(RetireCut)->io::Result<()>)->io::Result<()>{
    let t=next(src,0)?.ok_or_else(||invalid("no pending original retirement"))?;
    run(src,&t,&|_,_,_,_|Ok(()),&||Ok(()),&point)
}

//! A single SQLite reservation before any native side effect, then a short
//! conditional commit after the ordinary root/FILE header names have synced.
//! No secondary ledger. No source pathname, credential or permission boolean
//! is accepted from the client; a share never owns its own namespace.
use super::{Tree,Catalog,Connection,check,sqlite3_step};
use super::super::volume::OrdinaryRoot;
use crate::{BranchId,EntryId,FileId,HistoryId,OperationId,PoolId,Role,VolumeId,display,
    fs::{DIRECTORY_BYTES,DIRECTORY_NAMES,HeldDir,invalid},hash::Sha256,
    identity::{AccountId,ShareId},ordinary::portable_birth::Birth,system::ROOT_MARK};
use std::{collections::BTreeSet,ffi::{CStr,CString},io};

pub(crate) struct Reservation<'a>{birth:&'a Birth,parent:&'a HeldDir}
impl Reservation<'_>{pub(crate) fn original(&self)->(&Birth,&HeldDir){(self.birth,self.parent)}}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct NewFile{pub(crate) file:FileId,pub(crate) entry:EntryId,pub(crate) branch:BranchId,pub(crate) history:HistoryId,pub(crate) created:OperationId}
impl From<&Birth> for NewFile{fn from(b:&Birth)->Self{Self{file:b.file,entry:b.entry,branch:b.branch,history:b.history,created:b.operation}}}
#[derive(PartialEq,Eq)]
struct Origin{committed:bool,actor:AccountId,auth:OperationId,share:ShareId,mode:OperationId,birth:Birth}

fn request(pool:PoolId,op:OperationId,actor:AccountId,auth:OperationId,share:ShareId,mode:OperationId,volume:VolumeId,name:&str)->io::Result<[u8;32]>{
    display::component(name.as_bytes(),222)?;
    if [pool.0,op.0,actor.0,auth.0,share.0,mode.0,volume.0].contains(&[0;16]){return Err(invalid("missing FILE creation origin"));}
    let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/CREATE-ROOT-FILE\0")?;
    for v in [pool.0,op.0,actor.0,auth.0]{h.update(&v)?;}
    h.update(&1u64.to_be_bytes())?;
    for v in [share.0,mode.0,volume.0]{h.update(&v)?;}
    h.update(&[name.len() as u8])?;h.update(name.as_bytes())?;Ok(h.finish())
}

fn geometry(c:&Catalog,b:&Birth)->io::Result<()>{
    let f=c.rows().iter().find(|r|r.role==Role::File&&r.volume==b.volume).ok_or_else(||invalid("FILE is not in an enrolled ordinary volume"))?;
    let r=&c.rows()[1];b.validate_geometry((&f.path,f.limit),(&r.path,r.limit),ROOT_MARK)
}
fn by_op(db:&Connection,c:&Catalog,op:OperationId)->io::Result<Option<Origin>>{
    let s=db.prepare(c"SELECT o.kind,o.state,o.actor_id,o.request_sha,b.share_id,b.setting_op,b.auth_op,b.volume_id,b.display,b.terminal,b.entry_id,b.file_id,b.history_id,b.branch_id,b.ticket,r.volume_id,m.mode,m.share_id,a.smb_id,k.credential_op FROM main.operation o LEFT JOIN main.root_entry b ON b.create_op=o.op_id LEFT JOIN main.root_share r ON r.share_id=b.share_id LEFT JOIN main.share_mode m ON m.change_op=b.setting_op LEFT JOIN main.smb_password k ON k.credential_op=b.auth_op LEFT JOIN main.account_pair a ON a.singleton=k.slot WHERE o.op_id=?1")?;
    s.bind_bytes(1,&op.0)?;
    match unsafe{sqlite3_step(s.0)}{101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(invalid("failed to inspect original FILE operation"));}}
    if s.integer(0)?!=5{return Err(invalid("operation belongs to a different business object"));}
    let committed=match s.integer(1)?{1=>true,0=>false,_=>return Err(invalid("unknown FILE creation state"))};
    let actor=AccountId(s.array(2)?);let sha=s.array::<32>(3)?;let share=ShareId(s.array(4)?);
    let mode=OperationId(s.array(5)?);let auth=OperationId(s.array(6)?);let volume=VolumeId(s.array(7)?);
    let name=display::component(s.blob(8,222)?,222)?.to_owned();
    let b=Birth{pool:c.birth.pool,volume,operation:op,name,entry:EntryId(s.array(10)?),file:FileId(s.array(11)?),
        history:HistoryId(s.array(12)?),branch:BranchId(s.array(13)?),ticket:OperationId(s.array(14)?),slot:0};
    if b.container()?.to_bytes()!=s.blob(9,255)?||s.array::<16>(15)?!=volume.0||s.byte(16)?!=2||
       s.array::<16>(17)?!=share.0||s.array::<16>(18)?!=actor.0||s.array::<16>(19)?!=auth.0||
       [b.entry.0,b.file.0,b.history.0,b.branch.0,b.ticket.0].contains(&[0;16])||
       sha!=request(b.pool,op,actor,auth,share,mode,volume,&b.name)?{return Err(invalid("FILE membership disagrees with its original creation"));}
    geometry(c,&b)?;s.done()?;Ok(Some(Origin{committed,actor,auth,share,mode,birth:b}))
}

/// Bound the union of live names and SQL-promised final names (including
/// other incomplete operations). A stage and its future name occupy ONE slot
/// at different times. A user's display spelling that looks like a control
/// is not a directory claim. An unknown real ordinary directory is not free.
fn budget(db:&Connection,b:&Birth,live:&[Vec<u8>])->io::Result<()>{
    let mut remaining:BTreeSet<Vec<u8>>=live.iter().cloned().collect();let mut count=0usize;let mut bytes=0usize;
    if remaining.len()!=live.len()||!remaining.contains(ROOT_MARK.to_bytes()){return Err(invalid("ambiguous ordinary root census"));}

    let mut add=|pin:&CStr,stage:&CStr,terminal:&[u8],pending:bool|->io::Result<()>{
        if terminal.is_empty()||terminal.len()>255{return Err(invalid("invalid reserved container name"));}
        remaining.remove(pin.to_bytes());let has_end=remaining.remove(terminal);
        if pending&&remaining.remove(stage.to_bytes())&&has_end{return Err(invalid("both names of one pending container are occupied"));}
        count+=2;bytes+=2+pin.to_bytes().len()+if pending{terminal.len().max(stage.to_bytes().len())}else{terminal.len()};
        if count>DIRECTORY_NAMES||bytes>DIRECTORY_BYTES{return Err(invalid("registered ordinary namespace does not fit its census"));}
        Ok(())
    };
    let s=db.prepare(c"SELECT e.create_op,e.ticket,e.terminal,o.state FROM main.root_entry e JOIN main.operation o ON o.op_id=e.create_op WHERE e.volume_id=?1")?;
    s.bind_bytes(1,&b.volume.0)?;
    loop{match unsafe{sqlite3_step(s.0)}{
        100=>{let(pin,stage)=Birth::reservation_leaves(OperationId(s.array(0)?),OperationId(s.array(1)?))?;
            let terminal=s.blob(2,255)?;
            add(&pin,&stage,terminal,s.byte(3)?==0)?;}
        101=>break,rc=>{check(rc)?;return Err(invalid("cannot inspect outstanding ordinary reservations"));}
    }}
    add(&b.witness()?,&b.lower(0)?,b.container()?.to_bytes(),true)?;
    if count+remaining.len()>DIRECTORY_NAMES||bytes+remaining.iter().map(|n|1+n.len()).sum::<usize>()>DIRECTORY_BYTES{
        return Err(invalid("root already occupied or cannot accommodate all promised entries"));}
    Ok(())
}

/// Both initial and final admission use this same rule. Usually one O_PATH
/// vacancy check suffices. A native name equal to someone's FILE *terminal*
/// is not their displayed name. Only in that rare case verify its exact
/// spelling and real issued container before allowing the business name.
pub(super) fn display_free(db:&Connection,c:&Catalog,f:&HeldDir,b:&Birth)->io::Result<()>{
    display_free_in_sample(db,c,f,b,None)
}
pub(super) fn display_free_in_sample(db:&Connection,c:&Catalog,f:&HeldDir,b:&Birth,
    held_sample:Option<&BTreeSet<Vec<u8>>>)->io::Result<()>{
    if Birth::reserved_spelling(b.name.as_bytes()){return Ok(());} // never an ordinary-directory spelling
    let leaf=CString::new(b.name.as_str()).map_err(|_|invalid("bad FILE name"))?;
    if !f.entry_present(&leaf)?{return Ok(());}
    let s=db.prepare(c"SELECT create_op FROM main.root_entry WHERE volume_id=?1 AND terminal=?2")?;
    s.bind_bytes(1,&b.volume.0)?;s.bind_bytes(2,leaf.to_bytes())?;
    match unsafe{sqlite3_step(s.0)}{100=>{},101=>return Err(invalid("FILE display spelling occupied by an unrecognized native member")),rc=>{check(rc)?;return Err(invalid("cannot inspect a colliding native name"));}}
    let op=OperationId(s.array(0)?);s.done()?;
    let other=by_op(db,c,op)?.filter(|o|o.committed&&o.birth.volume==b.volume&&o.birth.name!=b.name)
        .ok_or_else(||invalid("colliding display spelling is not another confirmed FILE terminal"))?;
    if other.birth.container()?.as_c_str()!=leaf.as_c_str(){return Err(invalid("wrong derived terminal for displayed spelling"));}
    let(members,kept)=super::first_work::membership(db,c,&other.birth)?;
    let fresh;
    let sample=match held_sample{Some(s)=>s,None=>{fresh=f.names()?.into_iter().collect();&fresh}};
    other.birth.working_at(f,sample,false,members,&kept)?.require(f)
}

#[derive(Clone,Copy,PartialEq,Eq)]
pub(in crate::system::reader) enum Point{Reserved,Installed}
impl Tree<'_,'_>{
    fn origin(&self,db:&Connection,c:&Catalog,op:OperationId,name:&str)->io::Result<Option<Origin>>{
        let a=&self.session.original;
        let old=by_op(db,c,op)?;
        if old.as_ref().is_some_and(|x|x.birth.name!=name||x.birth.volume!=self.setting.volume||x.actor!=a.account||
           x.auth!=a.op||x.share!=self.setting.share||x.mode!=self.setting.op){return Err(invalid("same operation refers to a different original request"));}
        Ok(old)
    }
    pub(crate) fn create_file(&self,op:OperationId,name:&str)->io::Result<NewFile>{
        self.create_after(op,name,|_|Ok(()))
    }
    pub(in crate::system::reader) fn create_after(&self,op:OperationId,name:&str,mut point:impl FnMut(Point)->io::Result<()>)->io::Result<NewFile>{
        let src=self.session.source;let a=&self.session.original;let setting=self.setting;
        let sha=request(src.pool,op,a.account,a.op,setting.share,setting.op,setting.volume,name)?;
        let (catalog,seen)=src.at(|db,c|{self.require(db,&c,true)?;let old=self.origin(db,&c,op,name)?;Ok((c,old))})?;
        if let Some(ref x)=seen{if x.committed{return Ok(NewFile::from(&x.birth));}} // receipt, not a live file check
        let root=OrdinaryRoot::at(src,&catalog,setting.volume,true)?;
        let initial=if let Some(x)=seen{x.birth}else{
            let fresh=Birth{pool:src.pool,volume:setting.volume,operation:op,entry:EntryId::new()?,file:FileId::new()?,
                history:HistoryId::new()?,branch:BranchId::new()?,ticket:OperationId::new()?,slot:0,name:name.to_owned()};
            geometry(&catalog,&fresh)?;
            let w=Connection::open(&src.dir,true)?;
            let attempt=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
                self.require(db,&c,true)?;if let Some(x)=self.origin(db,&c,op,name)?{return Ok(x.birth);}
                display_free(db,&c,&root.root,&fresh)?;let live=root.root.names()?;fresh.new_root_vacancy(&root.root,&live)?;budget(db,&fresh,&live)?;root.require()?;
                let s=db.prepare(c"INSERT INTO main.root_entry(create_op,share_id,setting_op,auth_op,volume_id,display,terminal,entry_id,file_id,history_id,branch_id,ticket) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)")?;
                let end=fresh.container()?;
                for(i,b)in [op.0.as_slice(),setting.share.0.as_slice(),setting.op.0.as_slice(),a.op.0.as_slice(),
                    setting.volume.0.as_slice(),name.as_bytes(),end.to_bytes(),fresh.entry.0.as_slice(),fresh.file.0.as_slice(),
                    fresh.history.0.as_slice(),fresh.branch.0.as_slice(),fresh.ticket.0.as_slice()].into_iter().enumerate(){s.bind_bytes(i as i32+1,b)?;}
                s.done()?;drop(s);
                let o=db.prepare(c"INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,5,?2,?3,0)")?;
                o.bind_bytes(1,&op.0)?;o.bind_bytes(2,&a.account.0)?;o.bind_bytes(3,&sha)?;o.done()?;root.require()?;Ok(fresh)
            });
            drop(w);
            match attempt{Ok(b)=>b,Err(e)=>match src.at(|db,c|{self.require(db,&c,true)?;self.origin(db,&c,op,name)}){
                Ok(Some(x))=>x.birth,Ok(None)=>return Err(e),Err(_)=>return Err(invalid("reservation outcome unknown; query this same original operation"))}}
        };
        point(Point::Reserved)?;
        let selected=src.at(|db,c|{self.require(db,&c,true)?;
            let x=self.origin(db,&c,op,name)?.ok_or_else(||invalid("lost original FILE reservation"))?;
            if !x.committed{display_free(db,&c,&root.root,&x.birth)?;}Ok(x)
        })?;
        if selected.birth!=initial{return Err(invalid("reservation changed before native installation"));}
        if selected.committed{return Ok(NewFile::from(&initial));}
        root.require()?;let held=Reservation{birth:&initial,parent:&root.root}.make_ready()?;
        point(Point::Installed)?;
        let w=Connection::open(&src.dir,true)?;
        let outcome=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
            self.require(db,&c,true)?;let same=self.origin(db,&c,op,name)?.ok_or_else(||invalid("no reserved FILE"))?;
            if same.birth!=initial{return Err(invalid("different original FILE before confirmation"));}
            if !same.committed{
                let u=db.prepare(c"UPDATE main.operation SET state=1 WHERE op_id=?1 AND kind=5 AND state=0 RETURNING op_id")?;
                u.bind_bytes(1,&op.0)?;match unsafe{sqlite3_step(u.0)}{100 if u.array::<16>(0)?==op.0=>u.done()?,
                    rc=>{check(rc)?;return Err(invalid("FILE was not confirmed exactly once"));}}
            }
            display_free(db,&c,&root.root,&initial)?;root.require()?;held.require_newborn(&root.root)?;
            if root.root.entry_present(&initial.lower(0)?)?{return Err(invalid("original stage name was reoccupied before confirmation"));}
            Ok(NewFile::from(&initial))
        });
        drop(w);
        match outcome{Ok(done)=>Ok(done),Err(e)=>match src.at(|db,c|{self.require(db,&c,true)?;self.origin(db,&c,op,name)}){
            Ok(Some(x))if x.committed&&x.birth==initial=>Ok(NewFile::from(&initial)),
            Ok(_)=>Err(e),Err(_)=>Err(invalid("confirmation unknown; query the exact original FILE operation"))
        }}
    }

    pub(super) fn birth(&self,db:&Connection,c:&Catalog,name:&str,id:FileId)->io::Result<Birth>{
            let s=db.prepare(c"SELECT create_op FROM main.root_entry WHERE volume_id=?1 AND display=?2 AND file_id=?3")?;
            s.bind_bytes(1,&self.setting.volume.0)?;s.bind_bytes(2,name.as_bytes())?;s.bind_bytes(3,&id.0)?;
            match unsafe{sqlite3_step(s.0)}{100=>{},101=>return Err(io::ErrorKind::NotFound.into()),rc=>{check(rc)?;return Err(invalid("FILE lookup failed"));}}
            let op=OperationId(s.array(0)?);s.done()?;self.confirmed_birth(db,c,op,name,id)
    }
    pub(super) fn confirmed_birth(&self,db:&Connection,c:&Catalog,op:OperationId,name:&str,id:FileId)->io::Result<Birth>{
            let x=by_op(db,c,op)?.filter(|x|x.committed&&x.birth.file==id&&x.birth.volume==self.setting.volume&&x.birth.name==name)
                .ok_or_else(||invalid("original FILE not confirmed"))?;Ok(x.birth)
    }


    /// Independent cold open, not a receipt. Check the current share and its
    /// real F, not the original creator's share. Nothing here accepts a path
    /// from the SMB client or follows a recycle/other-volume fallback.
    pub(crate) fn newborn(&self,name:&str,id:FileId)->io::Result<NewFile>{
        display::component(name.as_bytes(),222)?;
        let src=self.session.source;
        let(c,b)=src.at(|db,c|{self.require(db,&c,false)?;let b=self.birth(db,&c,name,id)?;Ok((c,b))})?;
        let f=OrdinaryRoot::at(src,&c,b.volume,false)?;
        let names=f.root.names()?.into_iter().collect();let real=b.installed(&f.root,&names)?;
        src.at(|db,c|{self.require(db,&c,false)?;if self.birth(db,&c,name,id)?!=b{return Err(invalid("FILE membership changed during open"));}
            display_free(db,&c,&f.root,&b)?;f.require()?;real.require_newborn(&f.root)?;Ok(NewFile::from(&b))})
    }
}

//! A single SQLite reservation before any native side effect, then a short
//! conditional commit after the ordinary root/FILE header names have synced.
//! No secondary ledger. No source pathname, credential or permission boolean
//! is accepted from the client; a share never owns its own namespace.
use super::{Tree,Catalog,Connection,check,sqlite3_step};
use super::super::{file_location::{Place,Placed},volume::OrdinaryRoot,work_record::{self,Members}};
use crate::{BranchId,EntryId,FileId,HistoryId,OperationId,PoolId,Role,VolumeId,display,
    fs::{DIRECTORY_BYTES,DIRECTORY_NAMES,HeldDir,invalid},hash::Sha256,
    identity::{AccountId,ShareId},ordinary::portable_birth::Birth,system::ROOT_MARK};
use std::{collections::BTreeSet,ffi::{CStr,CString},io};

pub(super) struct NativeBorn<'r>{
    root:OrdinaryRoot<'r>,birth:Birth,installed:crate::ordinary::portable_birth::Installed,write:bool,
}
impl<'r> NativeBorn<'r>{
    pub(super) fn parts(self,writer:bool)->io::Result<(OrdinaryRoot<'r>,Birth,crate::ordinary::portable_birth::Installed)>{
        if self.write!=writer{return Err(invalid("native birth must keep the original holder kind"));}
        Ok((self.root,self.birth,self.installed))
    }
}

pub(crate) struct Reservation<'a>{birth:&'a Birth,parent:&'a HeldDir}
impl Reservation<'_>{pub(crate) fn original(&self)->(&Birth,&HeldDir){(self.birth,self.parent)}}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct NewFile{pub(crate) file:FileId,pub(crate) entry:EntryId,pub(crate) branch:BranchId,pub(crate) history:HistoryId,pub(crate) created:OperationId}
impl From<&Birth> for NewFile{fn from(b:&Birth)->Self{Self{file:b.file,entry:b.entry,branch:b.branch,history:b.history,created:b.operation}}}
#[derive(PartialEq,Eq)]
struct Origin{committed:bool,actor:AccountId,auth:OperationId,share:ShareId,mode:OperationId,birth:Birth}

fn request(pool:PoolId,op:OperationId,actor:AccountId,auth:OperationId,generation:i64,share:ShareId,mode:OperationId,volume:VolumeId,name:&str)->io::Result<[u8;32]>{
    display::component(name.as_bytes(),222)?;
    if generation<=0||[pool.0,op.0,actor.0,auth.0,share.0,mode.0,volume.0].contains(&[0;16]){return Err(invalid("missing FILE creation origin"));}
    let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/CREATE-ROOT-FILE\0")?;
    for v in [pool.0,op.0,actor.0,auth.0]{h.update(&v)?;}
    h.update(&generation.to_be_bytes())?;
    for v in [share.0,mode.0,volume.0]{h.update(&v)?;}
    h.update(&[name.len() as u8])?;h.update(name.as_bytes())?;Ok(h.finish())
}

fn geometry(c:&Catalog,b:&Birth)->io::Result<()>{
    let f=c.rows().iter().find(|r|r.role==Role::File&&r.volume==b.volume).ok_or_else(||invalid("FILE is not in an enrolled ordinary volume"))?;
    let r=&c.rows()[1];b.validate_geometry((&f.path,f.limit),(&r.path,r.limit),ROOT_MARK)
}
fn new_geometry(c:&Catalog,b:&Birth)->io::Result<()>{
    geometry(c,b)?;
    let role=&c.rows()[1];
    b.validate_transfer_geometry(&b.name,(&role.path,role.limit),b.ticket)
}
fn by_op(db:&Connection,c:&Catalog,op:OperationId)->io::Result<Option<Origin>>{
    let s=db.prepare(c"SELECT o.kind,o.state,o.actor_id,o.request_sha,b.share_id,b.setting_op,b.auth_op,b.volume_id,b.display,b.terminal,b.entry_id,b.file_id,b.history_id,b.branch_id,b.ticket,r.volume_id,m.mode,m.share_id,a.smb_id,k.credential_op,k.generation FROM main.operation o LEFT JOIN main.root_entry b ON b.create_op=o.op_id LEFT JOIN main.root_share r ON r.share_id=b.share_id LEFT JOIN main.share_mode m ON m.change_op=b.setting_op LEFT JOIN main.credential_epoch k ON k.role=1 AND k.credential_op=b.auth_op LEFT JOIN main.account_pair a ON a.singleton=k.slot WHERE o.op_id=?1")?;
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
       sha!=request(b.pool,op,actor,auth,s.integer(20)?,share,mode,volume,&b.name)?{return Err(invalid("FILE membership disagrees with its original creation"));}
    geometry(c,&b)?;s.done()?;Ok(Some(Origin{committed,actor,auth,share,mode,birth:b}))
}

pub(in crate::system::reader) fn committed_file(db:&Connection,c:&Catalog,id:FileId)->io::Result<Birth>{
    let s=db.prepare(c"SELECT create_op FROM main.root_entry WHERE file_id=?1")?;
    s.bind_bytes(1,&id.0)?;
    match unsafe{sqlite3_step(s.0)}{100=>{},101=>return Err(io::ErrorKind::NotFound.into()),rc=>{check(rc)?;return Err(invalid("FILE lookup failed"));}}
    let op=OperationId(s.array(0)?);s.done()?;
    by_op(db,c,op)?.filter(|x|x.committed&&x.birth.file==id).map(|x|x.birth).ok_or_else(||io::ErrorKind::NotFound.into())
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
    let s=db.prepare(c"SELECT e.create_op,e.ticket,e.terminal,o.state FROM main.root_entry e JOIN main.operation o ON o.op_id=e.create_op JOIN main.file_location l ON l.file_id=e.file_id AND l.generation=1 AND l.role=2 WHERE l.volume_id=?1")?;
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
    let owner=display_owner(db,c,b)?.ok_or_else(||invalid("FILE display spelling occupied by an unrecognized native member"))?;
    let fresh;
    let sample=match held_sample{Some(s)=>s,None=>{fresh=f.names()?.into_iter().collect();&fresh}};
    owner.check(f,sample)?.require(f)
}

/// SQLite half of an unusual display name equal to another FILE's terminal.
/// Does not look up any path or open F; the physical half is checked by the caller.
#[derive(PartialEq,Eq)]
pub(in crate::system::reader) struct Alias{birth:Birth,members:Option<Members>}
impl Alias{
    pub(in crate::system::reader) fn check(&self,root:&HeldDir,shape:&BTreeSet<Vec<u8>>)
        ->io::Result<crate::ordinary::portable_birth::Installed>{
        self.birth.working_records_at(root,shape,false,|scope|match &self.members{
            Some(live)=>live.require(scope),
            // A proved former container can explain a physical spelling;
            // it cannot grant FILE access or be inspected for its contents.
            None=>Ok(()),
        })
    }
}
pub(in crate::system::reader) fn display_owner(db:&Connection,c:&Catalog,b:&Birth)->io::Result<Option<Alias>>{
    display_owner_at(db,c,b.volume,&b.name)
}
pub(in crate::system::reader) fn display_owner_at(db:&Connection,c:&Catalog,volume:VolumeId,name:&str)->io::Result<Option<Alias>>{
    if Birth::reserved_spelling(name.as_bytes()){return Ok(None);}
    let s=db.prepare(c"SELECT e.create_op,EXISTS(SELECT 1 FROM main.file_location l WHERE l.file_id=e.file_id
       AND l.generation=1 AND l.position_op=e.create_op AND l.role=2 AND l.volume_id=e.volume_id)
      FROM main.root_entry e WHERE e.volume_id=?1 AND e.terminal=?2
        AND (EXISTS(SELECT 1 FROM main.file_location l WHERE l.file_id=e.file_id AND l.generation=1 AND l.role=2
                        AND l.position_op=e.create_op AND l.volume_id=e.volume_id)
          OR EXISTS(SELECT 1 FROM main.file_transfer m JOIN main.operation o ON o.op_id=m.transfer_op AND o.kind=12 AND o.state=1
                     WHERE m.file_id=e.file_id AND m.direction=1 AND m.source_op=e.create_op
                       AND m.source_gen=1 AND m.source_id=e.volume_id AND m.source_name=e.display))")?;
    s.bind_bytes(1,&volume.0)?;s.bind_bytes(2,name.as_bytes())?;
    match unsafe{sqlite3_step(s.0)}{100=>{},101=>return Ok(None),rc=>{check(rc)?;return Err(invalid("cannot inspect a colliding native name"));}}
    let op=OperationId(s.array(0)?);let current=s.byte(1)?==1;s.done()?;
    let Some(other)=by_op(db,c,op)?.filter(|o|o.committed)else{return Ok(None);};
    if other.birth.volume!=volume||other.birth.name==name||other.birth.container()?.to_bytes()!=name.as_bytes(){
        return Err(invalid("wrong confirmed terminal for displayed spelling"));}
    let members=if current{Some(work_record::members(db,c,&other.birth)?)}else{None};
    Ok(Some(Alias{birth:other.birth,members}))
}

#[derive(Clone,Copy,PartialEq,Eq)]
pub(in crate::system::reader) enum Point{Reserved,Installed}
impl<'s,'r> Tree<'s,'r>{
    /// No physical observation takes place in the caller's name cut. A
    /// pending birth is not absence and cannot be resolved by using its name.
    pub(super) fn occupied_name(&self,name:&str,write:bool)->io::Result<Option<FileId>>{
        display::component(name.as_bytes(),222)?;
        self.session.source.at(|db,c|{
            self.require(db,&c,write)?;
            self.name_on(db,&c,name)
        })
    }
    fn name_on(&self,db:&Connection,c:&Catalog,name:&str)->io::Result<Option<FileId>>{
        Ok(Place::named(db,c,self.setting.volume,name)?.map(|p|p.file))
    }
    /// An exact native share-mode admission precedes reservation. Hold the
    /// existing F namespace lock during this process, never a SQL transaction
    /// while waiting for native acknowledgement or doing volume I/O.
    pub(super) fn create_native<T>(&self,original:&OrdinaryRoot<'_>,name:&str,write_handle:bool,
        offer:impl FnOnce(&Birth,u64)->io::Result<()>,prepare:impl FnOnce(&HeldDir)->io::Result<T>,
        live:&impl Fn()->io::Result<()>)->io::Result<(NativeBorn<'r>,T)>{
        display::component(name.as_bytes(),222)?;
        let src=self.session.source;let a=&self.session.original;let setting=self.setting;
        let op=OperationId::new()?;let ticket=OperationId::new()?;let entry=EntryId::new()?;
        let history=HistoryId::new()?;let branch=BranchId::new()?;
        let candidates=FileId::birth_candidates()?;
        let sha=request(src.pool,op,a.account,a.op,a.generation,setting.share,setting.op,setting.volume,name)?;
        let (c,b,key,alias)=src.at(|db,c|{
            self.require(db,&c,true)?;live()?;
            if self.name_on(db,&c,name)?.is_some(){return Err(io::ErrorKind::AlreadyExists.into());}
            let (file,key)=super::file_number::available(db,src.pool,&candidates)?;
            let b=Birth{pool:src.pool,volume:setting.volume,operation:op,entry,file,history,branch,ticket,slot:0,name:name.to_owned()};
            new_geometry(&c,&b)?;let alias=display_owner(db,&c,&b)?;Ok((c,b,key,alias))
        })?;
        let locked=OrdinaryRoot::at(src,&c,setting.volume,true)?;
        let body=OrdinaryRoot::for_body(src,&c,setting.volume)?;
        for candidate in [&locked,&body]{
            if candidate.root.entity!=original.root.entity||!candidate.root.same_mount(&original.root)?{
                return Err(invalid("native CREATE cannot borrow a replacement folder"));
            }
        }
        original.require_streaming()?;
        let prove=||->io::Result<_>{
            let native=locked.root.names()?;
            b.new_root_vacancy(&locked.root,&native)?;
            let set=native.iter().cloned().collect();
            let owner=super::directory::check_alias(&locked,&b,alias.as_ref(),Some(&set))?;
            locked.require_streaming()?;Ok((native,owner))
        };
        prove()?;
        // With no recognized native ACK there is no SQLite reservation,
        // directory stage, publication, or successful CREATE reply.
        live()?;offer(&b,key.words().1)?;live()?;
        let (native,holder)=prove()?;
        let w=Connection::open(&src.dir,true)?;
        let reserved=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
            self.require(db,&c,true)?;
            if self.name_on(db,&c,name)?.is_some(){return Err(io::ErrorKind::AlreadyExists.into());}
            if self.origin(db,&c,op,name)?.is_some() || display_owner(db,&c,&b)?!=alias{
                return Err(invalid("native birth or confirmed display alias changed before reservation"));
            }
            // Other F roots have independent locks. Never substitute a new
            // FILE when the exact high-64 BLOB range now has another owner.
            super::file_number::available_exact(db,b.pool,b.file)?;
            budget(db,&b,&native)?;live()?;
            let s=db.prepare(c"INSERT INTO main.root_entry(create_op,share_id,setting_op,auth_op,volume_id,display,terminal,entry_id,file_id,history_id,branch_id,ticket) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)")?;
            let end=b.container()?;
            for(i,v)in [op.0.as_slice(),setting.share.0.as_slice(),setting.op.0.as_slice(),a.op.0.as_slice(),
                setting.volume.0.as_slice(),name.as_bytes(),end.to_bytes(),b.entry.0.as_slice(),b.file.0.as_slice(),
                b.history.0.as_slice(),b.branch.0.as_slice(),b.ticket.0.as_slice()].iter().enumerate(){s.bind_bytes(i as i32+1,v)?;}
            s.done()?;let cmd=db.prepare(c"INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,5,?2,?3,0)")?;
            cmd.bind_bytes(1,&op.0)?;cmd.bind_bytes(2,&a.account.0)?;cmd.bind_bytes(3,&sha)?;cmd.done()
        });
        drop(w);drop(holder);
        if let Err(error)=reserved{
            let prior=src.at(|db,c|{self.require(db,&c,true)?;self.origin(db,&c,op,name)})?;
            if !prior.is_some_and(|o|!o.committed&&o.birth==b){return Err(error);}
        }
        live()?;
        let installed=Reservation{birth:&b,parent:&locked.root}.make_ready()?;
        if write_handle{installed.directory().exclusive()?;}else{installed.directory().file.unlock()?;}
        // Obtain the live holder's new scratch/nonce before advertising the
        // original SQL birth. Failing here cannot make a visible FILE.
        let holder=prepare(installed.directory())?;
        let alias_pin=super::directory::check_alias(&locked,&b,alias.as_ref(),None)?;
        installed.require_newborn(&locked.root)?;
        if locked.root.entry_present(&b.lower(0)?)?{return Err(invalid("old stage name reappeared before native confirmation"));}
        locked.require_streaming()?;body.require_streaming()?;
        let w=Connection::open(&src.dir,true)?;
        let confirmed=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
            self.require(db,&c,true)?;
            if !self.origin(db,&c,op,name)?.is_some_and(|o|!o.committed&&o.birth==b)||
                display_owner(db,&c,&b)?!=alias{return Err(invalid("different pending native birth"));}
            super::file_number::original(db,b.pool,b.file)?;
            live()?;
            let u=db.prepare(c"UPDATE main.operation SET state=1 WHERE op_id=?1 AND kind=5 AND state=0 RETURNING op_id")?;
            u.bind_bytes(1,&op.0)?;
            if !work_record::start_one(&u)?||u.array::<16>(0)?!=op.0{return Err(invalid("native birth did not confirm its original"));}u.done()
        });
        drop(w);drop(alias_pin);
        if let Err(error)=confirmed{
            let prior=src.at(|db,c|{self.require(db,&c,true)?;self.origin(db,&c,op,name)})?;
            if !prior.is_some_and(|o|o.committed&&o.birth==b){return Err(error);}
        }
        Ok((NativeBorn{root:body,birth:b,installed,write:write_handle},holder))
    }

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
        let sha=request(src.pool,op,a.account,a.op,a.generation,setting.share,setting.op,setting.volume,name)?;
        let (catalog,seen)=src.at(|db,c|{self.require(db,&c,true)?;let old=self.origin(db,&c,op,name)?;Ok((c,old))})?;
        if let Some(ref x)=seen{if x.committed{return Ok(NewFile::from(&x.birth));}} // receipt, not a live file check
        let root=OrdinaryRoot::at(src,&catalog,setting.volume,true)?;
        let initial=if let Some(x)=seen{x.birth}else{
            let candidates=FileId::birth_candidates()?;let entry=EntryId::new()?;
            let history=HistoryId::new()?;let branch=BranchId::new()?;let ticket=OperationId::new()?;
            let w=Connection::open(&src.dir,true)?;
            let attempt=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
                self.require(db,&c,true)?;if let Some(x)=self.origin(db,&c,op,name)?{return Ok(x.birth);}
                let file=super::file_number::choose(db,&candidates)?;
                let fresh=Birth{pool:src.pool,volume:setting.volume,operation:op,entry,file,history,branch,ticket,slot:0,name:name.to_owned()};
                new_geometry(&c,&fresh)?;
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

    pub(super) fn placed(&self,db:&Connection,c:&Catalog,name:&str,id:Option<FileId>)->io::Result<Placed>{
        let p=Place::named(db,c,self.setting.volume,name)?.ok_or(io::ErrorKind::NotFound)?;
        if id.is_some_and(|id|p.file!=id){return Err(io::ErrorKind::NotFound.into());}
        p.bind(db,c)
    }
    // The legacy first-SAVE fixture and the newborn diagnostic address
    // only the immutable ordinary birth, not an arbitrary current placement.
    pub(super) fn birth(&self,db:&Connection,c:&Catalog,name:&str,id:FileId)->io::Result<Birth>{
        let p=self.placed(db,c,name,Some(id))?;
        if p.here!=Place::origin(&p.birth){return Err(invalid("expected the original ordinary FILE birth"));}
        Ok(p.birth)
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

#[cfg(test)]
mod native_tests{
    use super::*;
    use crate::{system::reader::{InitialReader,ordinary_pool,sqlite3_get_autocommit,smb::SmbSession},
        test_support::PrivateTemp};
    use std::cell::Cell;

    #[test]
    #[ignore="source fixture on two real SQLite observers and an ordinary F; acknowledges the native cut manually"]
    fn native_offer_needs_exact_ack_before_sql_and_does_not_take_a_read_lock(){
        let tmp=PrivateTemp::under(&std::env::temp_dir());let(path,pool,_,rw,ro)=ordinary_pool(&tmp);
        let src=InitialReader::open(&path,pool).unwrap();
        let s=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
        let w=s.connect(rw).unwrap();let r=s.connect(ro).unwrap();
        let obs=InitialReader::alongside(&src.dir,&src.marker,pool,src.expected_schema).unwrap();
        let c=src.snapshot().unwrap();let root=OrdinaryRoot::for_body(&src,&c,w.setting.volume).unwrap();
        let name="native.txt";let reached=Cell::new(false);
        let gate=|b:&Birth,num:u64|{
            assert_eq!(unsafe{sqlite3_get_autocommit(src.conn.0)},1);
            assert_eq!(u64::from_be_bytes(b.file.0[..8].try_into().unwrap()),num);
            obs.at(|db,c|{assert!(by_op(db,&c,b.operation)?.is_none());Ok(())})?;
            assert!(!root.root.entry_present(&b.container()?)?);
            reached.set(true);Ok(())
        };
        assert!(w.create_native(&root,name,false,|b,num|{
            gate(b,num)?;Err(io::ErrorKind::NotConnected.into())
        },|_|Ok(()),&||Ok(())).is_err());
        assert!(reached.get());assert_eq!(w.occupied_name(name,true).unwrap(),None);
        let (born,())=w.create_native(&root,name,false,gate,|_|Ok(()),&||Ok(())).unwrap();
        let old=w.reader_from_newborn(born).unwrap();assert_eq!(old.size(),0);
        assert_eq!(r.occupied_name(name,false).unwrap(),Some(old.file()));
        let writer=w.open_writer(name,old.file()).unwrap();
        assert!(w.open_writer(name,old.file()).is_err());
        assert_eq!(old.smb_read_window(0,10,|b|Ok(b.len())).unwrap(),0);
        drop(writer);
        assert!(w.create_native(&root,name,true,|_,_|panic!("already present"),|_|Ok(()),&||Ok(())).is_err());
        let offered=Cell::new(None);
        let fd_limit=w.create_native(&root,"cannot-prepare",true,|b,n|{
            gate(b,n)?;offered.set(Some(b.operation));Ok(())
        },|_|Err::<(),_>(io::Error::from(io::ErrorKind::StorageFull)),&||Ok(()));
        assert_eq!(fd_limit.err().unwrap().kind(),io::ErrorKind::StorageFull);
        obs.at(|db,c|{
            let row=by_op(db,&c,offered.get().unwrap())?.unwrap();
            assert!(!row.committed);Ok(())
        }).unwrap();
        assert!(w.open_current("cannot-prepare").is_err());
    }
}

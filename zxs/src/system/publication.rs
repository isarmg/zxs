//! First online metadata writer. An actual management identity may publish
//! one logical ordinary root. This action cannot grant or use any FILE ACL.
use super::{Management,Manager};
use super::super::{Catalog,Connection,check,sqlite3_step};
use crate::{OperationId,PoolId,Role,VolumeId,display,fs::invalid,hash::Sha256,identity::{AccountId,ShareId}};
use std::io;

#[derive(Clone,Debug,Eq,PartialEq)]
pub(crate) struct PublishedRoot{
    pub(crate) op:OperationId,pub(crate) share:ShareId,pub(crate) volume:VolumeId,pub(crate) label:String,
}
struct Request<'a>{
    pool:PoolId,op:OperationId,actor:AccountId,credential:OperationId,volume:VolumeId,label:&'a str,sha:[u8;32],
}
impl<'a> Request<'a>{
    fn new(m:&Manager<'_>,op:OperationId,volume:VolumeId,label:&'a str)->io::Result<Self>{
        let pool=m.source.pool;let actor=m.account;let credential=m.origin;
        display::component(label.as_bytes(),display::MAX)?;
        if op.0==[0;16]||volume.0==[0;16]{return Err(invalid("missing publication operation or volume"));}
        let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/PUBLISH-ROOT\0")?;
        for raw in [pool.0,op.0,actor.0,credential.0]{h.update(&raw)?;}
        h.update(&1u64.to_be_bytes())?; // only this first credential generation is implemented
        h.update(&volume.0)?;h.update(&(label.len() as u16).to_be_bytes())?;h.update(label.as_bytes())?;
        Ok(Self{pool,op,actor,credential,volume,label,sha:h.finish()})
    }
    /// Refuse a conflicting globally used Op even if it has no share row.
    /// No row is the only possible new-operation case.
    fn prior(&self,db:&Connection)->io::Result<Option<PublishedRoot>>{
        let s=db.prepare(c"SELECT o.kind,o.state,o.actor_id,o.request_sha,s.share_id,s.author_auth_op,s.volume_id,s.display_label FROM main.operation o LEFT JOIN main.root_share s ON s.publish_op=o.op_id WHERE o.op_id=?1")?;
        s.bind_bytes(1,&self.op.0)?;
        match unsafe{sqlite3_step(s.0)}{101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(invalid("unexpected receipt lookup"));}}
        if s.integer(0)?!=3||s.integer(1)?!=1||s.array::<16>(2)?!=self.actor.0||s.array::<32>(3)?!=self.sha||
            s.array::<16>(5)?!=self.credential.0||s.array::<16>(6)?!=self.volume.0||s.blob(7,display::MAX)?!=self.label.as_bytes(){
            return Err(invalid("operation belongs to a different action, principal, scope or request"));
        }
        let share=ShareId(s.array(4)?);if share.0==[0;16]{return Err(invalid("missing original ShareId"));}
        s.done()?;
        Ok(Some(self.result(share)))
    }
    fn result(&self,share:ShareId)->PublishedRoot{PublishedRoot{op:self.op,share,volume:self.volume,label:self.label.to_owned()}}
    fn at(&self,m:&Manager<'_>,db:&Connection,c:&Catalog)->io::Result<Option<PublishedRoot>>{
        // Both *current* first-account profiles manage share definitions.
        // A future profile must make that judgment explicitly.
        match m.current(db,c)?{Management::ReadAcl|Management::ReadAndWriteAcl=>{}}
        if c.birth.pool!=self.pool || !c.rows().iter().any(|v|v.role==Role::File&&v.volume==self.volume){
            return Err(invalid("share requires an ordinary registered root in this pool"));
        }
        self.prior(db)
    }
    fn insert(&self,db:&Connection,share:ShareId)->io::Result<PublishedRoot>{
        let s=db.prepare(c"INSERT INTO main.root_share(share_id,publish_op,author_auth_op,volume_id,display_label) VALUES(?1,?2,?3,?4,?5)")?;
        for (i,b) in [share.0.as_slice(),self.op.0.as_slice(),self.credential.0.as_slice(),self.volume.0.as_slice(),self.label.as_bytes()].into_iter().enumerate(){
            s.bind_bytes(i as i32+1,b)?;
        }
        s.done()?;drop(s);
        let o=db.prepare(c"INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,3,?2,?3,1)")?;
        o.bind_bytes(1,&self.op.0)?;o.bind_bytes(2,&self.actor.0)?;o.bind_bytes(3,&self.sha)?;o.done()?;
        Ok(self.result(share)) // not an externally successful result until COMMIT
    }
}
impl Manager<'_>{
    pub(crate) fn publish_root(&self,op:OperationId,volume:VolumeId,label:&str)->io::Result<PublishedRoot>{
        self.publish_root_after_selection(op,volume,label,||Ok(()))
    }
    pub(in crate::system::reader) fn publish_root_after_selection(
        &self,op:OperationId,volume:VolumeId,label:&str,after:impl FnOnce()->io::Result<()>
    )->io::Result<PublishedRoot>{
        let r=Request::new(self,op,volume,label)?;
        // An unauthorized/invalid request does not even ask for a RW connection.
        let read_original=||self.source.at(|db,c|r.at(self,db,&c));
        if let Some(old)=read_original()?{return Ok(old);}
        after()?;
        let candidate=ShareId::new()?; // non-reserving; no SQL write transaction during entropy acquisition
        self.source.dir.require()?;
        let result={
            let w=Connection::open(&self.source.dir,true)?;
            self.source.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
                if let Some(old)=r.at(self,db,&c)?{return Ok(old);}
                r.insert(db,candidate)
            })
        }; // close this SQLite writer before resolving any uncertain outcome
        match result{
            Ok(published)=>Ok(published),
            Err(cause)=>match read_original(){
                Ok(Some(old))=>Ok(old),
                Ok(None)=>Err(cause),
                Err(_)=>Err(io::Error::other("publication not confirmed; verify this exact operation on its original pool before retrying")),
            }
        }
    }
}

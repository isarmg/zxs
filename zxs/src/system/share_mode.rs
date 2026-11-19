//! One setting per ordinary-root share. This module cannot authenticate SMB,
//! issue an IO handle or convert a WebAdmin into the file user.
use super::{Catalog,Choice,Connection,WebAdmin,check,sqlite3_step};
use super::super::sqlite3_column_type;
use crate::{OperationId,PoolId,Role,VolumeId,fs::invalid,hash::Sha256,identity::{AccountId,ShareId}};
use std::io;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
#[repr(u8)]
pub(crate) enum Mode{Disabled=0,ReadOnly=1,ReadWrite=2}
impl Mode{fn parse(value:u8)->io::Result<Self>{match value{0=>Ok(Self::Disabled),1=>Ok(Self::ReadOnly),
    2=>Ok(Self::ReadWrite),_=>Err(invalid("unknown share mode"))}}}

/// No authorization is encoded in this display/receipt value. A later IO
/// action must bind the real SMB session and this same current mode revision.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct Setting{
    pub(crate) share:ShareId,pub(crate) volume:VolumeId,pub(crate) op:OperationId,
    pub(crate) revision:i64,pub(crate) mode:Mode,
}

// An exact SMB/management selection and a management-only keyset page use
// the same current-mode row. Both bounds use the BLOB primary key; a page
// doesn't enumerate the previous pages or make one extra SELECT per share.
fn selected(db:&Connection,c:&Catalog,lo:ShareId,hi:ShareId,skip_lo:bool,limit:u8,
            mut visit:impl FnMut(Setting,&str))->io::Result<()>{
    let s=db.prepare(c"SELECT r.volume_id,r.publish_op,p.kind,p.state,m.change_op,m.revision,m.mode,o.kind,o.state,r.share_id,r.display_label FROM main.root_share r JOIN main.operation p ON p.op_id=r.publish_op LEFT JOIN main.share_mode m ON m.share_id=r.share_id AND m.revision=(SELECT max(h.revision) FROM main.share_mode h WHERE h.share_id=r.share_id) LEFT JOIN main.operation o ON o.op_id=m.change_op WHERE r.share_id>=?1 AND r.share_id<=?2 AND (?3=0 OR r.share_id>?1) ORDER BY r.share_id LIMIT ?4")?;
    s.bind_bytes(1,&lo.0)?;s.bind_bytes(2,&hi.0)?;s.bind_integer(3,i64::from(u8::from(skip_lo)))?;s.bind_integer(4,i64::from(limit))?;
    loop{
        match unsafe{sqlite3_step(s.0)}{100=>{},101=>return Ok(()),rc=>{check(rc)?;return Err(invalid("cannot select share"));}}
        let share=ShareId(s.array(9)?);if share.0==[0;16]{return Err(invalid("invalid share identity"));}
        let volume=VolumeId(s.array(0)?);let publish=OperationId(s.array(1)?);
        if publish.0==[0;16]||s.integer(2)?!=3||s.integer(3)?!=1||
            !c.rows().iter().any(|v|v.volume==volume&&v.role==Role::File){return Err(invalid("share has no active ordinary-root origin"));}
        let (op,revision,mode)=if unsafe{sqlite3_column_type(s.0,4)}==5{
            (publish,0,Mode::Disabled)
        }else{
            let op=OperationId(s.array(4)?);let rev=s.integer(5)?;
            if op.0==[0;16]||rev<1||s.integer(7)?!=4||s.integer(8)?!=1{return Err(invalid("share setting has no committed origin"));}
            (op,rev,Mode::parse(s.byte(6)?)?)
        };
        visit(Setting{share,volume,op,revision,mode},crate::display::component(s.blob(10,crate::display::MAX)?,crate::display::MAX)?);
    }
}
pub(in crate::system::reader) fn current(db:&Connection,c:&Catalog,share:ShareId)->io::Result<Setting>{
    if share.0==[0;16]{return Err(invalid("missing share identity"));}
    find(db,c,share)?.ok_or_else(||invalid("share has not been published"))
}
pub(in crate::system::reader) fn find(db:&Connection,c:&Catalog,share:ShareId)->io::Result<Option<Setting>>{
    let mut found=None;selected(db,c,share,share,false,1,|s,_|found=Some(s))?;Ok(found)
}
pub(super) fn page(db:&Connection,c:&Catalog,after:Option<ShareId>)->io::Result<(Vec<(Setting,String)>,Option<ShareId>)>{
    const LIMIT:usize=32;let mut rows=Vec::new();
    selected(db,c,after.unwrap_or(ShareId([0;16])),ShareId([255;16]),true,(LIMIT+1) as u8,|s,name|rows.push((s,name.to_owned())))?;
    let next=if rows.len()>LIMIT{rows.truncate(LIMIT);rows.last().map(|r|r.0.share)}else{None};
    Ok((rows,next))
}

struct Change{pool:PoolId,actor:AccountId,auth:OperationId,op:OperationId,share:ShareId,previous:OperationId,mode:Mode,sha:[u8;32]}
impl Change{
    fn new(m:&WebAdmin<'_>,op:OperationId,share:ShareId,previous:OperationId,mode:Mode)->io::Result<Self>{
        if [op.0,share.0,previous.0].contains(&[0;16])||op==previous{return Err(invalid("missing or reused share mode identity"));}
        let pool=m.source.pool;let actor=m.account;let auth=m.origin;
        let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/SHARE-MODE\0")?;
        for x in [pool.0,op.0,actor.0,auth.0]{h.update(&x)?;}
        h.update(&1u64.to_be_bytes())?;h.update(&share.0)?;h.update(&previous.0)?;h.update(&[mode as u8])?;
        Ok(Self{pool,actor,auth,op,share,previous,mode,sha:h.finish()})
    }
    fn select(&self,db:&Connection,c:&Catalog)->io::Result<Choice<Setting,Setting>>{
        if c.birth.pool!=self.pool{return Err(invalid("wrong pool for share setting"));}
        let now=current(db,c,self.share)?;
        let s=db.prepare(c"SELECT o.kind,o.state,o.actor_id,o.request_sha,m.share_id,m.previous_op,m.author_auth_op,m.revision,m.mode FROM main.operation o LEFT JOIN main.share_mode m ON m.change_op=o.op_id WHERE o.op_id=?1")?;
        s.bind_bytes(1,&self.op.0)?;
        match unsafe{sqlite3_step(s.0)}{
            100=>{
                if s.integer(0)?!=4||s.integer(1)?!=1||s.array::<16>(2)?!=self.actor.0||s.array::<32>(3)?!=self.sha||
                    s.array::<16>(4)?!=self.share.0||s.array::<16>(5)?!=self.previous.0||s.array::<16>(6)?!=self.auth.0||
                    Mode::parse(s.byte(8)?)?!=self.mode{return Err(invalid("operation identifies a different setting, identity or request"));}
                let rev=s.integer(7)?;if rev<1||rev>now.revision{return Err(invalid("setting is outside current history"));}
                s.done()?;Ok(Choice::Original(Setting{op:self.op,revision:rev,mode:self.mode,..now}))
            }
            101=>{
                if now.op!=self.previous||now.mode==self.mode{return Err(invalid("stale setting or no actual change"));}
                let rev=now.revision.checked_add(1).ok_or_else(||invalid("share mode revision exhausted"))?;
                Ok(Choice::New(Setting{op:self.op,revision:rev,mode:self.mode,..now}))
            }
            rc=>{check(rc)?;Err(invalid("could not inspect original share mode operation"))}
        }
    }
    fn insert(&self,db:&Connection,next:Setting)->io::Result<Setting>{
        let s=db.prepare(c"INSERT INTO main.share_mode(change_op,share_id,previous_op,author_auth_op,revision,mode) VALUES(?1,?2,?3,?4,?5,?6)")?;
        for(i,x)in [self.op.0,self.share.0,self.previous.0,self.auth.0].iter().enumerate(){s.bind_bytes(i as i32+1,x)?;}
        s.bind_integer(5,next.revision)?;s.bind_integer(6,i64::from(self.mode as u8))?;s.done()?;drop(s);
        let o=db.prepare(c"INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,4,?2,?3,1)")?;
        o.bind_bytes(1,&self.op.0)?;o.bind_bytes(2,&self.actor.0)?;o.bind_bytes(3,&self.sha)?;o.done()?;
        Ok(next)
    }
}
impl WebAdmin<'_>{
    pub(crate) fn share_setting(&self,share:ShareId)->io::Result<Setting>{
        self.source.at(|db,c|{self.current(db,&c)?;current(db,&c,share)})
    }
    pub(crate) fn set_share_mode(&self,op:OperationId,share:ShareId,previous:OperationId,mode:Mode)->io::Result<Setting>{
        let request=Change::new(self,op,share,previous,mode)?;
        self.metadata_change(|db,c|request.select(db,c),||Ok(()),|db,next,()|request.insert(db,next))
    }
}

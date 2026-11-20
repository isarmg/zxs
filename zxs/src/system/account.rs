//! Two and only two protocol identities in one SQLite pool. Web never selects
//! the Samba secret. Neither an account row nor a share definition opens FILE.
use super::{Catalog,Connection,InitialReader,check,sqlite3_step};
use crate::{OperationId,PoolId,fs::invalid,identity::AccountId,password::Verifier};
use std::io;

use super::credential::{Pair,Role};
#[cfg(test)] use super::credential::setup_request;

#[derive(Eq,PartialEq)]
struct WebCurrent{account:AccountId,op:OperationId,generation:i64,login:String,smb_login:String,verifier:Verifier}
impl WebCurrent{
    fn select(db:&Connection,c:&Catalog)->io::Result<Option<Self>>{
        let Some(pair)=Pair::confirmed(db,c)? else{return Ok(None)};
        let Some(epoch)=pair.epoch(db,c,Role::Web)? else{return Ok(None)};
        let s=db.prepare(c"SELECT salt,verifier FROM main.web_password WHERE slot=1 AND credential_op=?1 AND generation=?2")?;
        s.bind_bytes(1,&epoch.op.0)?;s.bind_integer(2,epoch.generation)?;
        match unsafe{sqlite3_step(s.0)}{100=>{},101=>return Err(invalid("missing selected Web credential")),rc=>{check(rc)?;return Err(invalid("cannot select Web credential"));}}
        let verifier=Verifier::new(s.array(0)?,s.array(1)?);s.done()?;
        Ok(Some(Self{account:pair.web,op:epoch.op,generation:epoch.generation,login:pair.web_login,smb_login:pair.smb_login,verifier}))
    }
}
fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"Web credentials not accepted")}

/// Only this type can issue a Web management operation. It has no data-open
/// methods, never accepts a Samba SID or a claimed remote account id.
pub(crate) struct WebAdmin<'a>{source:&'a InitialReader,account:AccountId,origin:OperationId,generation:i64}
impl InitialReader{
    pub(crate) fn authenticate_web(&self,login:&str,password:&str)->io::Result<WebAdmin<'_>>{
        self.authenticate_web_after(login,password,||Ok(()))
    }
    pub(super) fn authenticate_web_after(&self,login:&str,password:&str,after:impl FnOnce()->io::Result<()>)->io::Result<WebAdmin<'_>>{
        if login.is_empty()||login.len()>64||password.is_empty()||password.len()>1024{return Err(denied());}
        let m=self.at(|db,c|WebCurrent::select(db,&c))?.ok_or_else(denied)?;
        if !m.verifier.matches(password)?||m.login.as_bytes()!=login.as_bytes(){return Err(denied());}
        after()?;
        self.at(|db,c|{
            if WebCurrent::select(db,&c)?.as_ref()!=Some(&m){return Err(denied());}
            Ok(WebAdmin{source:self,account:m.account,origin:m.op,generation:m.generation})
        })
    }
}
// Metadata-only transaction: don't hold a SQL lock during KDF, OS I/O, or
// entropy acquisition. Retry an ambiguous write only by its exact original.
enum Choice<T,P>{Original(T),New(P)}
impl WebAdmin<'_>{
    fn metadata_change<T,P,N>(&self,
        select:impl Fn(&Connection,&Catalog)->io::Result<Choice<T,P>>,
        prepare:impl FnOnce()->io::Result<N>,
        insert:impl FnOnce(&Connection,P,N)->io::Result<T>
    )->io::Result<T>{
        let choose=|db:&Connection,c:&Catalog|{self.current(db,c)?;select(db,c)};
        let read=||self.source.at(|db,c|choose(db,&c));
        if let Choice::Original(old)=read()?{return Ok(old);}
        let prepared=prepare()?;
        self.source.dir.require()?;
        let result={
            let w=Connection::open(&self.source.dir,true)?;
            self.source.transaction(&w,c"BEGIN IMMEDIATE",|db,c|match choose(db,&c)?{
                Choice::Original(old)=>Ok(old),Choice::New(base)=>insert(db,base,prepared),
            })
        };
        match result{
            Ok(value)=>Ok(value),
            Err(cause)=>match read(){
                Ok(Choice::Original(old))=>Ok(old),Ok(Choice::New(_))=>Err(cause),
                Err(_)=>Err(io::Error::other("outcome not confirmed; check this exact operation at the original pool")),
            }
        }
    }
    fn current(&self,db:&Connection,c:&Catalog)->io::Result<WebCurrent>{
        let m=WebCurrent::select(db,c)?.ok_or_else(denied)?;
        if (m.account,m.op,m.generation)!=(self.account,self.origin,self.generation){return Err(denied());}Ok(m)
    }
    #[cfg(test)]
    pub(super) fn id(&self)->io::Result<AccountId>{self.source.at(|db,c|{self.current(db,&c)?;Ok(self.account)})}
}

/// Same nonsecret request and receipt helper must implement independently.
/// Do not ask for either password merely to resolve a lost setup response.
#[cfg(test)]
pub(super) fn setup_state(src:&InitialReader,op:OperationId,actor:[u8;16],approval:[u8;16],
    instance:[u8;16],bound:[u8;32],web:&[u8],smb:&[u8],sid:&[u8;28])->io::Result<Option<(bool,AccountId,AccountId)>>{
    let q=std::ffi::CString::new(include_str!("original-first.sql")).expect("fixed SQL");
    src.at(|db,c|{
        let sha=setup_request(c.birth.pool,c.birth.op,op,actor,approval,instance,bound,web,smb,sid)?;let s=db.prepare(&q)?;
        for(i,b)in [c.birth.pool.0.as_slice(),op.0.as_slice(),actor.as_slice(),sha.as_slice(),c.birth.op.0.as_slice(),
                    approval.as_slice(),web,smb,sid.as_slice(),instance.as_slice(),bound.as_slice()].into_iter().enumerate(){s.bind_bytes(i as i32+1,b)?;}
        match unsafe{sqlite3_step(s.0)}{101=>Ok(None),100=>{
            let committed=match s.byte(0)?{0=>false,1=>true,_=>return Err(invalid("invalid account original state"))};
            let result=(committed,AccountId(s.array(1)?),AccountId(s.array(2)?));s.done()?;Ok(Some(result))
        },rc=>{check(rc)?;Err(invalid("unexpected original pair result"))}}
    })
}

#[cfg(test)]
#[path="publication.rs"]
mod publication;

#[path="share_mode.rs"]
pub(crate) mod share_mode;

#[path="overview.rs"]
pub(crate) mod overview;

#[path="volume_status.rs"]
pub(crate) mod volume_status;

#[path="manual_work.rs"]
pub(crate) mod manual_work;
#[path="history.rs"]
pub(crate) mod history;

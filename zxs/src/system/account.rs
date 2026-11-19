//! Two and only two protocol identities in one SQLite pool. Web never selects
//! the Samba secret. Neither an account row nor a share definition opens FILE.
use super::{Catalog,Connection,InitialReader,check,sqlite3_step};
use crate::{OperationId,PoolId,fs::invalid,hash::Sha256,identity::AccountId,password::{Verifier,login_name}};
use std::io;

const DOMAIN:&[u8]=b"ZXS-SQLITE-1/SETUP-PAIR\0";
const SID_PREFIX:[u8;12]=[1,5,0,0,0,0,0,5,21,0,0,0];
/// Standalone Samba domain SID S-1-5-21-x-y-z plus the dedicated normal RID
/// 1000. This nonsecret mapping is not a token or proof of authentication.
fn check_names(web:&[u8],smb:&[u8],sid:&[u8;28])->io::Result<()>{
    login_name(web)?;let s=login_name(smb)?;
    if web==smb||matches!(s,"guest"|"anonymous"|"root"|"nobody")||
        sid[..12]!=SID_PREFIX||sid[12..24]==[0;12]||sid[24..]!=1000u32.to_le_bytes(){
        return Err(invalid("invalid or colliding fixed-protocol identities"));
    }Ok(())
}
pub(crate) fn setup_request(pool:PoolId,init:OperationId,op:OperationId,actor:[u8;16],approval:[u8;16],
    web:&[u8],smb:&[u8],sid:&[u8;28])->io::Result<[u8;32]>{
    check_names(web,smb,sid)?;
    if [pool.0,init.0,op.0,actor,approval].contains(&[0;16])||op==init{return Err(invalid("invalid account setup origin"));}
    let mut h=Sha256::new();h.update(DOMAIN)?;
    for id in [pool.0,init.0,op.0,actor,approval]{h.update(&id)?;}
    for name in [web,smb]{h.update(&[name.len() as u8])?;h.update(name)?;}
    h.update(sid)?;Ok(h.finish())
}

#[derive(Eq,PartialEq)]
struct WebCurrent{account:AccountId,op:OperationId,login:String,smb_login:String,verifier:Verifier}
impl WebCurrent{
    fn select(db:&Connection,c:&Catalog)->io::Result<Option<Self>>{
        let s=db.prepare(c"SELECT o.op_id,o.actor_id,o.request_sha,a.approval_ref,a.web_id,a.web_login,a.smb_id,a.smb_login,a.smb_sid,w.credential_op,w.generation,w.salt,w.verifier FROM main.account_pair a JOIN main.operation o ON o.op_id=a.birth_op JOIN main.web_password w ON w.slot=a.singleton WHERE a.singleton=1 AND o.kind=2 AND o.state=1")?;
        match unsafe{sqlite3_step(s.0)}{101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(invalid("unexpected Web credential query"));}}
        let op=OperationId(s.array(0)?);let actor=s.array(1)?;let request=s.array::<32>(2)?;let approval=s.array(3)?;
        let account=AccountId(s.array(4)?);let login=s.blob(5,64)?;let smb_id=s.array::<16>(6)?;
        if account.0==[0;16]||smb_id==[0;16]||smb_id==account.0||s.array::<16>(9)?!=op.0||s.integer(10)?!=1||
            setup_request(c.birth.pool,c.birth.op,op,actor,approval,login,s.blob(7,64)?,&s.array(8)?)?!=request{
            return Err(invalid("Web credential not bound to the original fixed pair"));
        }
        let row=Self{account,op,login:login_name(login)?.to_owned(),smb_login:login_name(s.blob(7,64)?)?.to_owned(),verifier:Verifier::new(s.array(11)?,s.array(12)?)};
        s.done()?;Ok(Some(row))
    }
}
fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"Web credentials not accepted")}

/// Only this type can issue a Web management operation. It has no data-open
/// methods, never accepts a Samba SID or a claimed remote account id.
pub(crate) struct WebAdmin<'a>{source:&'a InitialReader,account:AccountId,origin:OperationId}
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
            Ok(WebAdmin{source:self,account:m.account,origin:m.op})
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
        if (m.account,m.op)!=(self.account,self.origin){return Err(denied());}Ok(m)
    }
    #[cfg(test)]
    pub(super) fn id(&self)->io::Result<AccountId>{self.source.at(|db,c|{self.current(db,&c)?;Ok(self.account)})}
}

/// Same nonsecret request and receipt helper must implement independently.
/// Do not ask for either password merely to resolve a lost setup response.
#[cfg(test)]
pub(super) fn setup_receipt(src:&InitialReader,op:OperationId,actor:[u8;16],approval:[u8;16],
    web:&[u8],smb:&[u8],sid:&[u8;28])->io::Result<Option<(AccountId,AccountId)>>{
    let q=std::ffi::CString::new(include_str!("original-first.sql")).expect("fixed SQL");
    src.at(|db,c|{
        let sha=setup_request(c.birth.pool,c.birth.op,op,actor,approval,web,smb,sid)?;let s=db.prepare(&q)?;
        for(i,b)in [c.birth.pool.0.as_slice(),op.0.as_slice(),actor.as_slice(),sha.as_slice(),c.birth.op.0.as_slice(),
                    approval.as_slice(),web,smb,sid.as_slice()].into_iter().enumerate(){s.bind_bytes(i as i32+1,b)?;}
        match unsafe{sqlite3_step(s.0)}{101=>Ok(None),100=>{
            let result=(AccountId(s.array(0)?),AccountId(s.array(1)?));s.done()?;Ok(Some(result))
        },rc=>{check(rc)?;Err(invalid("unexpected original pair result"))}}
    })
}

#[path="publication.rs"]
pub(crate) mod publication;

#[path="share_mode.rs"]
pub(crate) mod share_mode;

#[path="overview.rs"]
pub(crate) mod overview;

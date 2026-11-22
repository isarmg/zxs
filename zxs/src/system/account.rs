//! One shared identity in the original SQLite pool. A Web role remains a
//! separate authority with no SMB secret or FILE body access.
use super::{Catalog,Connection,InitialReader,check};
use crate::{OperationId,identity::AccountId,password::Verifier};
use std::io;

use super::{credential::Pair,super::shared_account::PublicEpoch};

#[derive(Eq,PartialEq)]
struct WebCurrent{account:AccountId,canonical:AccountId,op:OperationId,generation:i64,login:String}
impl WebCurrent{
    fn from_head(pair:Pair,epoch:PublicEpoch)->Self{
        Self{account:pair.web,canonical:pair.canonical,op:OperationId(epoch.op),
            generation:epoch.generation,login:pair.login}
    }
    fn select(db:&Connection,c:&Catalog)->io::Result<Option<Self>>{
        Ok(Pair::qualified_web_metadata(db,c)?.map(|(pair,epoch)|Self::from_head(pair,epoch)))
    }
    fn for_login(db:&Connection,c:&Catalog)->io::Result<Option<(Self,Verifier)>>{
        Ok(Pair::qualified_web(db,c)?.map(|(pair,epoch,verifier)|(Self::from_head(pair,epoch),verifier)))
    }
}
fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"Web credentials not accepted")}

/// Only this type can issue a Web management operation. It has no data-open
/// methods, never accepts a remote SID or a claimed account id.
pub(crate) struct WebAdmin<'a>{source:&'a InitialReader,account:AccountId,canonical:AccountId,origin:OperationId,generation:i64}
impl InitialReader{
    pub(crate) fn authenticate_web(&self,login:&str,password:&str)->io::Result<WebAdmin<'_>>{
        self.authenticate_web_after(login,password,||Ok(()))
    }
    pub(super) fn authenticate_web_after(&self,login:&str,password:&str,after:impl FnOnce()->io::Result<()>)->io::Result<WebAdmin<'_>>{
        if login.is_empty()||login.len()>64||password.is_empty()||password.len()>1024{return Err(denied());}
        let before=self.at(|db,c|WebCurrent::for_login(db,&c))?.ok_or_else(denied)?;
        let (m,verifier)=&before;
        if !verifier.matches(password)?||m.login.as_bytes()!=login.as_bytes(){return Err(denied());}
        after()?;
        self.at(|db,c|{
            if WebCurrent::for_login(db,&c)?.as_ref()!=Some(&before){return Err(denied());}
            Ok(WebAdmin{source:self,account:m.account,canonical:m.canonical,origin:m.op,generation:m.generation})
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
        if (m.account,m.canonical,m.op,m.generation)!=(self.account,self.canonical,self.origin,self.generation){return Err(denied());}Ok(m)
    }
    #[cfg(test)]
    pub(super) fn id(&self)->io::Result<AccountId>{self.source.at(|db,c|{self.current(db,&c)?;Ok(self.account)})}
}

#[cfg(test)]
#[path="publication.rs"]
mod publication;

#[path="share_mode.rs"]
pub(crate) mod share_mode;

#[path="overview.rs"]
pub(crate) mod overview;


#[path="manual_work.rs"]
pub(crate) mod manual_work;
#[path="history.rs"]
pub(crate) mod history;

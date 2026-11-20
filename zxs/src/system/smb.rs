//! Business authority belongs to the original authenticated session. Production
//! construction is private to its storage-side endpoint after the managed SMB
//! worker's successful final native update; a SID, name or WebAdmin is not proof.
use super::{Catalog,Connection,InitialReader,account::share_mode::{self,Mode,Setting},credential::{Pair,Role},check,sqlite3_step};
use crate::{OperationId,identity::{AccountId,ShareId}};
use std::io;

fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"no current authenticated SMB access")}
#[derive(Clone,Debug,Eq,PartialEq)]
struct Credential{account:AccountId,instance:[u8;16],op:OperationId,generation:i64,sid:[u8;28],login:Vec<u8>}
impl Credential{
    // Nonsecret mapping only. Native authentication never reads or opens SQL.
    fn current(db:&Connection,c:&Catalog)->io::Result<Self>{
        let pair=Pair::confirmed(db,c)?.ok_or_else(denied)?;
        let epoch=pair.epoch(db,c,Role::Smb)?.ok_or_else(denied)?;
        Ok(Self{account:pair.smb,instance:pair.instance,op:epoch.op,generation:epoch.generation,sid:pair.sid,login:pair.smb_login.into_bytes()})
    }
}
pub(crate) struct SmbSession<'r>{source:&'r InitialReader,original:Credential}
pub(crate) struct Tree<'s,'r>{session:&'s SmbSession<'r>,setting:Setting}
impl<'r> SmbSession<'r>{
    #[cfg(test)]
    pub(in crate::system::reader) fn fixture_authenticated(source:&'r InitialReader,login:&str)->io::Result<Self>{
        let original=source.at(|db,c|Credential::current(db,&c))?;
        if original.login!=login.as_bytes(){return Err(denied());}Ok(Self{source,original})
    }
    #[cfg(test)]
    pub(crate) fn connect(&self,share:ShareId)->io::Result<Tree<'_,'r>>{
        self.connect_service(share.smb_service().as_bytes())
    }
    /// Only the service portion already separated by SMB, never an entire
    /// UNC, display label or host path. Resolving an ID isn't authentication.
    pub(crate) fn connect_service(&self,name:&[u8])->io::Result<Tree<'_,'r>>{
        self.connect_id(ShareId::from_smb_service(name).ok_or_else(denied)?)
    }
    fn connect_id(&self,share:ShareId)->io::Result<Tree<'_,'r>>{
        self.select_with_catalog(share).map(|(tree,_)|tree)
    }
    fn select_with_catalog(&self,share:ShareId)->io::Result<(Tree<'_,'r>,Catalog)>{
        self.source.at(|db,c|{
            if Credential::current(db,&c)?!=self.original{return Err(denied());}
            let setting=share_mode::find(db,&c,share)?.ok_or_else(denied)?;
            if setting.mode==Mode::Disabled{return Err(denied());}Ok((Tree{session:self,setting},c))
        })
    }
}
impl Tree<'_,'_>{
    fn require(&self,db:&Connection,c:&Catalog,write:bool)->io::Result<()>{
        if Credential::current(db,c)?!=self.session.original||share_mode::current(db,c,self.setting.share)?!=self.setting||
           self.setting.mode==Mode::Disabled||(write&&self.setting.mode!=Mode::ReadWrite){return Err(denied());}Ok(())
    }
}

#[path="file_number.rs"]
pub(crate) mod file_number;

#[path="file_create.rs"]
pub(crate) mod file_create;

#[path="first_work.rs"]
pub(crate) mod first_work;

#[path="directory.rs"]
pub(crate) mod directory;

#[path="smb_runtime.rs"]
pub(crate) mod runtime;

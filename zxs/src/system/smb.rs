//! Business authority belongs to the original authenticated session, not
//! a raw name/SID or permission to use the other protocol. The in-process
//! test session below never issues a live SMB grant.
use super::{Catalog,Connection,InitialReader,account::share_mode::{self,Mode,Setting},credential::Pair,check,sqlite3_step};
use crate::{OperationId,fs::directory_birth::DirectoryParent,identity::{AccountId,ShareId}};
use std::io;

fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"no current authenticated SMB access")}
#[derive(Clone,Debug,Eq,PartialEq)]
struct Credential{account:AccountId,canonical:AccountId,instance:[u8;16],op:OperationId,generation:i64,sid:[u8;28],login:Vec<u8>}
impl Credential{
    // Nonsecret mapping only. The live Rust challenge still requires its vault.
    fn current(db:&Connection,c:&Catalog)->io::Result<Self>{
        let pair=Pair::confirmed(db,c)?.ok_or_else(denied)?;
        let epoch=pair.current(db,c)?.ok_or_else(denied)?;
        Ok(Self{account:pair.smb,canonical:pair.canonical,instance:pair.instance,op:OperationId(epoch.op),generation:epoch.generation,sid:pair.sid,login:pair.login.into_bytes()})
    }
}
pub(crate) struct SmbSession<'r>{source:&'r InitialReader,original:Credential}
pub(crate) struct Tree<'s,'r>{session:&'s SmbSession<'r>,setting:Setting,parent:DirectoryParent}
impl<'r> SmbSession<'r>{
    #[cfg(test)]
    pub(in crate::system::reader) fn fixture_authenticated(source:&'r InitialReader,login:&str)->io::Result<Self>{
        let original=source.at(|db,c|Credential::current(db,&c))?;
        if original.login!=login.as_bytes(){return Err(denied());}Ok(Self{source,original})
    }
    fn require_current(&self,db:&Connection,c:&Catalog)->io::Result<()>{
        if Credential::current(db,c)?!=self.original{return Err(denied());}Ok(())
    }
    /// A SQL fixture may choose a typed ShareId; it is not a wire service name.
    #[cfg(test)]
    pub(crate) fn connect(&self,share:ShareId)->io::Result<Tree<'_,'r>>{self.connect_id(share)}
    fn connect_id(&self,share:ShareId)->io::Result<Tree<'_,'r>>{
        self.select_with_catalog(share).map(|(tree,_)|tree)
    }
    fn select_with_catalog(&self,share:ShareId)->io::Result<(Tree<'_,'r>,Catalog)>{
        self.source.at(|db,c|{
            if Credential::current(db,&c)?!=self.original{return Err(denied());}
            let setting=share_mode::find(db,&c,share)?.ok_or_else(denied)?;
            if setting.mode==Mode::Disabled{return Err(denied());}Ok((Tree{session:self,setting,parent:DirectoryParent::Root},c))
        })
    }
}
impl<'s,'r> Tree<'s,'r>{
    fn scoped(&self,parent:DirectoryParent)->Tree<'s,'r>{Tree{session:self.session,setting:self.setting,parent}}
    fn require(&self,db:&Connection,c:&Catalog,write:bool)->io::Result<()>{
        if Credential::current(db,c)?!=self.session.original||share_mode::current(db,c,self.setting.share)?!=self.setting||
           self.setting.mode==Mode::Disabled||(write&&self.setting.mode!=Mode::ReadWrite){return Err(denied());}Ok(())
    }
}

#[path="file_number.rs"]
pub(crate) mod file_number;

#[path="file_create.rs"]
pub(crate) mod file_create;
pub(crate) use file_create::{RustFileBirth,RustNamedWrite};

#[path="first_work.rs"]
pub(crate) mod first_work;

#[path="smb_buffer.rs"]
mod smb_buffer;
#[path="retained_read.rs"]
mod retained_read;
pub(crate) use retained_read::{RustRetainedDirectory,RustRetainedFile,RustRetainedKind,RustRetainedPage,RustRetainedReadProof};

#[path="file_transfer.rs"]
mod file_transfer;
pub(crate) use file_transfer::{ConfirmedFileDelete,RustFileDelete,RustRetainedRestore};
#[path="file_rename.rs"]
mod file_rename;
#[path="file_retirement.rs"]
mod retirement;

#[path="work_change.rs"]
pub(crate) mod work_change;

#[path="smb_stage.rs"]
mod smb_stage;
pub(crate) use smb_stage::{RustStagedName,RustStagedReadProof,RustStagedStandard,RustStagedState,RustStagedWriter};

#[path="directory.rs"]
pub(crate) mod directory;

#[path="ordinary_directory.rs"]
pub(in crate::system::reader) mod ordinary_directory;
pub(crate) use ordinary_directory::RustNamedDirectory;

#[path="smb_runtime.rs"]
pub(crate) mod runtime;

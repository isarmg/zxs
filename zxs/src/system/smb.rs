//! The business side of a positively authenticated SMB session. No network
//! adapter is active yet. A SID, a table row, a name, a WebAdmin or a true-ish
//! argument cannot create a session. Only the source qualification fixture can
//! supply that boundary until the actual Samba authentication bridge exists.
use super::{Catalog,Connection,InitialReader,account::{setup_request,share_mode::{self,Mode,Setting}},check,sqlite3_step};
use crate::{OperationId,fs::invalid,identity::{AccountId,ShareId}};
use std::io;

fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"no current authenticated SMB access")}
#[derive(Clone,Debug,Eq,PartialEq)]
struct Credential{account:AccountId,op:OperationId,generation:i64,sid:[u8;28],login:Vec<u8>}
impl Credential{
    // Only the public identity/generation of the original credential, never
    // the NT password-equivalent and never the Web verifier. Not a login.
    fn current(db:&Connection,c:&Catalog)->io::Result<Self>{
        let s=db.prepare(c"SELECT b.birth_op,x.actor_id,x.request_sha,b.approval_ref,b.web_login,b.smb_id,b.smb_login,b.smb_sid,b.web_id,k.credential_op,k.generation FROM main.account_pair b JOIN main.operation x ON x.op_id=b.birth_op JOIN main.smb_password k ON k.slot=b.singleton WHERE b.singleton=1 AND x.kind=2 AND x.state=1")?;
        match unsafe{sqlite3_step(s.0)}{100=>{},101=>return Err(denied()),rc=>{check(rc)?;return Err(denied());}}
        let op=OperationId(s.array(0)?);let account=AccountId(s.array(5)?);let login=s.blob(6,64)?.to_vec();let sid=s.array(7)?;
        let generation=s.integer(10)?;
        if account.0==[0;16]||s.array::<16>(8)?==account.0||s.array::<16>(9)?!=op.0||generation!=1||
           setup_request(c.birth.pool,c.birth.op,op,s.array(1)?,s.array(3)?,s.blob(4,64)?,&login,&sid)?!=s.array::<32>(2)?{
            return Err(invalid("SMB origin is not the exact committed protocol credential"));}
        s.done()?;Ok(Self{account,op,generation,sid,login})
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
    pub(crate) fn connect(&self,share:ShareId)->io::Result<Tree<'_,'r>>{
        self.source.at(|db,c|{
            if Credential::current(db,&c)?!=self.original{return Err(denied());}
            let setting=share_mode::current(db,&c,share)?;
            if setting.mode==Mode::Disabled{return Err(denied());}Ok(Tree{session:self,setting})
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

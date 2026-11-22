//! Qualified reads for one account. Must be used only inside the same original
//! S transaction as its validated catalog. Nothing here issues a credential,
//! opens the vault or a writer, or makes an authenticated role.
use super::{Catalog,Connection,Statement,check,super::shared_account::{PublicEpoch,SharedScope}};
use crate::{OperationId,PoolId,fs::invalid,identity::AccountId,password::{self,Verifier}};
use std::{ffi::CStr,io};

#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) struct Epoch{pub(super) op:OperationId,pub(super) generation:i64}

/// Shared public coordinates plus the SMB actor domain, not a secret or grant.
#[derive(Clone,Eq,PartialEq)]
pub(super) struct ActiveRustMeta{
    pub(super) pool:PoolId,pub(super) init:OperationId,pub(super) setup:OperationId,pub(super) install_sha:[u8;32],
    pub(super) instance:[u8;16],pub(super) bind:[u8;16],pub(super) canonical:AccountId,pub(super) smb:AccountId,
    pub(super) login:String,pub(super) sid:[u8;28],pub(super) epoch:Epoch,pub(super) record:[u8;16],
}
pub(super) type Pair=OneAccount;

pub(super) struct OneAccount{
    pub(super) setup:OperationId,
    pub(super) setup_bound:[u8;32],
    pub(super) instance:[u8;16],
    pub(super) bind:[u8;16],
    pub(super) canonical:AccountId,
    // These are internal role actor keys, not distinct configured accounts.
    pub(super) web:AccountId,
    pub(super) smb:AccountId,
    pub(super) login:String,
    pub(super) sid:[u8;28],
    first:[u8;16],
}
fn bad()->io::Error{invalid("cannot qualify the exact shared account and epoch")}
impl OneAccount{
    fn scope<'a>(&'a self,c:&Catalog)->SharedScope<'a>{
        SharedScope{pool:c.birth.pool.0,init:c.birth.op.0,setup:self.setup.0,instance:self.instance,
            bind:self.bind,account:self.canonical.0,init_sha:c.birth.request,plan_sha:self.setup_bound,
            original_actor:c.birth.actor,login:self.login.as_bytes(),sid:self.sid}
    }
    pub(super) fn confirmed(db:&Connection,c:&Catalog)->io::Result<Option<Self>>{
        let mut s=db.prepare(c"SELECT a.birth_op,a.setup_bound_sha,a.instance_id,a.smb_rust_bind_id,
                 a.account_id,a.web_id,a.smb_id,a.login,a.smb_sid,v.record_id,
                 o.actor_id,a.approval_ref,o.request_sha
            FROM main.account_pair a
            CROSS JOIN main.operation o ON o.op_id=a.birth_op AND o.kind=2 AND o.state=1
            LEFT JOIN main.smb_rust_revision v ON v.credential_op=a.birth_op
            WHERE a.singleton=1")?;
        match s.step(){101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(bad());}}
        let identity=Self{setup:OperationId(s.array(0)?),setup_bound:s.array(1)?,instance:s.array(2)?,
            bind:s.array(3)?,canonical:AccountId(s.array(4)?),web:AccountId(s.array(5)?),smb:AccountId(s.array(6)?),
            login:password::login_name(s.row()?.blob(7,64)?)?.to_owned(),sid:s.array(8)?,first:s.array(9)?};
        let scope=identity.scope(c);let keys=scope.role_keys()?;
        if s.array::<16>(10)?!=c.birth.actor||keys.web!=identity.web.0||keys.smb!=identity.smb.0||
            scope.setup_request(s.array(11)?,identity.first)?!=s.array::<32>(12)?{return Err(bad());}
        s.done()?;
        Ok(Some(identity))
    }

    fn web_head(db:&Connection,c:&Catalog)->io::Result<Option<(Self,PublicEpoch)>>{
        let Some(pair)=Self::confirmed(db,c)? else{return Ok(None);};
        let Some(epoch)=pair.current(db,c)? else{return Ok(None);};
        Ok(Some((pair,epoch)))
    }

    /// A business cut needs the current Web role and a validly shaped
    /// verifier row, not the salt or tag. Keep them out of its SQL result.
    pub(super) fn qualified_web_metadata(db:&Connection,c:&Catalog)->io::Result<Option<(Self,PublicEpoch)>>{
        let Some((pair,epoch))=Self::web_head(db,c)? else{return Ok(None);};
        require_web_shape(db,epoch)?;
        Ok(Some((pair,epoch)))
    }

    /// The only lookup exposing a Web verifier. Raw public coordinates alone
    /// cannot load the tag: setup and current reset qualify in this same cut.
    pub(super) fn qualified_web(db:&Connection,c:&Catalog)->io::Result<Option<(Self,PublicEpoch,Verifier)>>{
        let Some((pair,epoch))=Self::web_head(db,c)? else{return Ok(None);};
        Ok(Some((pair,epoch,web_material(db,epoch)?)))
    }

    pub(super) fn active_rust_at(&self,db:&Connection,c:&Catalog)->io::Result<Option<ActiveRustMeta>>{
        let Some(e)=self.current(db,c)? else{return Ok(None);};
        Ok(Some(ActiveRustMeta{pool:c.birth.pool,init:c.birth.op,setup:self.setup,install_sha:self.setup_bound,
            instance:self.instance,bind:self.bind,canonical:self.canonical,smb:self.smb,login:self.login.clone(),
            sid:self.sid,epoch:Epoch{op:OperationId(e.op),generation:e.generation},record:e.record}))
    }

    /// One original head, or neither role is usable. No password material
    /// is loaded or returned. The DDL's highest attempted generation is
    /// bounded by index; select the exact predecessor by a primary-key join
    /// rather than materializing the historical epoch/role view.
    pub(super) fn current(&self,db:&Connection,c:&Catalog)->io::Result<Option<PublicEpoch>>{
        let s=db.prepare(c"SELECT e.credential_op,e.generation,e.record_id,o.actor_id,o.request_sha,
                r.approval_ref,r.previous_op,r.bound_sha,p.generation,po.state,po.kind,po.actor_id,vp.record_id
            FROM main.active_shared_credential e
            CROSS JOIN main.operation o ON o.op_id=e.credential_op
            LEFT JOIN main.credential_reset r ON r.op_id=e.credential_op
            LEFT JOIN main.credential_reset p ON p.op_id=r.previous_op
            LEFT JOIN main.operation po ON po.op_id=r.previous_op
            LEFT JOIN main.smb_rust_revision vp ON vp.credential_op=r.previous_op
            WHERE e.slot=1")?;
        match s.step(){101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(bad());}}
        let result=PublicEpoch{op:s.array(0)?,generation:s.integer(1)?,record:s.array(2)?};
        let is_null=|i|s.is_null(i);
        if s.array::<16>(3)?!=c.birth.actor{return Err(bad());}
        if result.generation==1{
            if result.op!=self.setup.0||result.record!=self.first||(5..13).any(|i|!is_null(i)){return Err(bad());}
        }else{
            let old=PublicEpoch{op:s.array(6)?,generation:result.generation.checked_sub(1).ok_or_else(bad)?,record:s.array(12)?};
            let first=old.generation==1;
            let previous_matches=if first{is_null(8)&&old.record==self.first}else{s.integer(8)?==old.generation};
            if result.generation<=1||s.array::<32>(7)?!=self.setup_bound||s.integer(9)?!=1||
                s.integer(10)?!=(if first{2}else{9})||s.array::<16>(11)?!=c.birth.actor||
                !previous_matches
            {return Err(bad());}
            let expected=self.scope(c).reset_request(s.array(5)?,old,result)?;
            if expected!=s.array::<32>(4)?{return Err(bad());}
        }
        s.done()?;
        Ok(Some(result))
    }
}

// Both projections must identify the same original row. Only their result
// columns differ; the joins and all three epoch binds are fixed here.
macro_rules! select_web_password{
    ($columns:literal)=>{
        match CStr::from_bytes_with_nul(concat!("SELECT ",$columns," FROM main.active_shared_credential a
        CROSS JOIN main.web_password w ON w.slot=a.slot AND w.credential_op=a.credential_op AND w.generation=a.generation
        WHERE a.credential_op=?1 AND a.generation=?2 AND a.record_id=?3\0").as_bytes()){
            Ok(sql)=>sql,Err(_)=>panic!("invalid fixed Web password query"),
        }
    }
}
const WEB_MATERIAL:&CStr=select_web_password!("w.salt,w.verifier");
const WEB_SHAPE:&CStr=select_web_password!(
    "CASE WHEN typeof(w.salt)='blob' THEN length(w.salt) ELSE -1 END,
     CASE WHEN typeof(w.verifier)='blob' THEN length(w.verifier) ELSE -1 END");

fn web_password_row<'a>(db:&'a Connection,e:PublicEpoch,sql:&'static CStr)->io::Result<Statement<'a>>{
    let s=db.prepare(sql)?;
    s.bind_bytes(1,&e.op)?;s.bind_integer(2,e.generation)?;s.bind_bytes(3,&e.record)?;
    match s.step(){100=>Ok(s),rc=>{check(rc)?;Err(bad())}}
}

fn require_web_shape(db:&Connection,e:PublicEpoch)->io::Result<()>{
    let s=web_password_row(db,e,WEB_SHAPE)?;
    for (column,width) in [(0,16),(1,32)]{
        // Non-BLOB is not an empty BLOB. Preserve the fixed-blob reader's
        // storage-class and width decisions without asking for the data.
        let n=s.integer(column)?;
        if n<0{return Err(invalid("unexpected SQLite blob storage class"));}
        if n==0||n>width{return Err(invalid("missing or oversized SQLite blob"));}
        if n!=width{return Err(invalid("wrong SQLite identity/digest width"));}
    }
    s.done()
}

/// Neither SMB nor launch/custody receives this verifier.
fn web_material(db:&Connection,e:PublicEpoch)->io::Result<Verifier>{
    let s=web_password_row(db,e,WEB_MATERIAL)?;
    let result=Verifier::new(s.array(0)?,s.array(1)?);
    s.done()?;
    Ok(result)
}

#[cfg(test)]
#[path = "../../tests/unit/attached/system/credential/tests.rs"]
mod tests;

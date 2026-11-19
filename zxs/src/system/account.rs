//! Online first-account authentication from the very same SQLite cut as
//! the original pool. No issuing, remote bearer, volume or FILE grant here.
use super::{Catalog,Connection,InitialReader,check,sqlite3_step};
use crate::{OperationId,PoolId,fs::invalid,hash::Sha256,identity::AccountId,password::{Verifier,login_name}};
use std::io;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) enum Management{ReadAcl,ReadAndWriteAcl}
impl Management{
    fn decode(v:u8)->io::Result<Self>{match v{1=>Ok(Self::ReadAcl),2=>Ok(Self::ReadAndWriteAcl),_=>Err(invalid("unknown first-account management scope"))}}
    fn tag(self)->u8{match self{Self::ReadAcl=>1,Self::ReadAndWriteAcl=>2}}
}
pub(crate) fn request(pool:PoolId,init:OperationId,op:OperationId,actor:[u8;16],approval:[u8;16],scope:Management,login:&[u8])->io::Result<[u8;32]>{
    login_name(login)?;
    if [pool.0,init.0,op.0,actor,approval].contains(&[0;16])||op==init{
        return Err(invalid("invalid first-account origin"));
    }
    let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/FIRST-ADMIN\0")?;
    for id in [pool.0,init.0,op.0,actor,approval]{h.update(&id)?;}
    h.update(&[scope.tag(),login.len() as u8])?;h.update(login)?;Ok(h.finish())
}

#[derive(Eq,PartialEq)]
struct First{
    account:AccountId,op:OperationId,scope:Management,login:String,verifier:Verifier,
}
impl First{
    fn select(db:&Connection,cut:&Catalog)->io::Result<Option<Self>>{
        // There is precisely one accepted bootstrap action. Unknown versions,
        // incomplete credentials and an altered non-secret request never log in.
        let s=db.prepare(c"SELECT o.op_id,o.actor_id,o.request_sha,a.account_id,a.login,a.approval_ref,a.boot_profile,p.credential_op,p.generation,p.salt,p.verifier FROM main.operation o JOIN main.account a ON a.birth_op=o.op_id JOIN main.account_password p ON p.account_id=a.account_id WHERE o.kind=2 AND o.state=1")?;
        match unsafe{sqlite3_step(s.0)}{101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(invalid("unexpected SQLite account step"));}}
        let op=OperationId(s.array(0)?);let actor=s.array(1)?;let recorded=s.array::<32>(2)?;
        let account=AccountId(s.array(3)?);let login=login_name(s.blob(4,64)?)?.to_owned();
        let approval=s.array(5)?;let scope=Management::decode(s.byte(6)?)?;
        if account.0==[0;16]||s.array::<16>(7)?!=op.0||s.integer(8)?!=1||
            request(cut.birth.pool,cut.birth.op,op,actor,approval,scope,login.as_bytes())?!=recorded{
            return Err(invalid("first-account relation disagrees with its origin"));
        }
        let verifier=Verifier::new(s.array(9)?,s.array(10)?);
        match unsafe{sqlite3_step(s.0)}{101=>Ok(Some(Self{account,op,scope,login,verifier})),
           100=>Err(invalid("more than one original first account")),rc=>{check(rc)?;Err(invalid("unexpected end of first account"))}}
    }
}
fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"account credentials unavailable or not accepted")}

/// Not a FILE capability. Holding this keeps the same true S lifecycle SH,
/// not an open SQL transaction. Re-observe the current account for each use.
pub(crate) struct Manager<'a>{source:&'a InitialReader,account:AccountId,origin:OperationId}
impl InitialReader{
    pub(crate) fn authenticate_first(&self,login:&str,password:&str)->io::Result<Manager<'_>>{
        self.authenticate_first_after(login,password,||Ok(()))
    }
    pub(super) fn authenticate_first_after(&self,login:&str,password:&str,after_verify:impl FnOnce()->io::Result<()>)->io::Result<Manager<'_>>{
        if login.len()>64||password.is_empty()||password.len()>1024{return Err(denied());}
        let first=self.at(|db,c|First::select(db,&c))?.ok_or_else(denied)?;
        // Even a different well-sized login goes through the same first
        // account KDF. Neither the SQL transaction nor its rows survive here.
        if !first.verifier.matches(password)? || first.login.as_bytes()!=login.as_bytes(){return Err(denied());}
        after_verify()?;
        self.at(|db,c|{
            if First::select(db,&c)?.as_ref()!=Some(&first){return Err(denied());}
            Ok(Manager{source:self,account:first.account,origin:first.op})
        })
    }
}
impl Manager<'_>{
    /// Fresh metadata only; future actual management operations also need
    /// their own same-transaction checks. This result cannot grant a FILE.
    pub(crate) fn profile(&self)->io::Result<(AccountId,Management)>{
        self.source.at(|db,c|{
            let m=First::select(db,&c)?.ok_or_else(denied)?;
            if m.account!=self.account||m.op!=self.origin{return Err(denied());}
            Ok((m.account,m.scope))
        })
    }
}

/// Sole public-query shape for a separately authorized original-operation
/// reader (helper implements its own trusted actor checks). Parameters are
/// borrowed for the lifetime of the statement, never interpolated.
#[cfg(test)]
pub(super) fn first_receipt(src:&InitialReader,op:OperationId,actor:[u8;16],approval:[u8;16],scope:Management,login:&[u8])->io::Result<Option<(AccountId,Management)>>{
    use super::sqlite3_bind_blob;
    use std::{ffi::CString,os::raw::c_int};
    unsafe extern "C"{fn sqlite3_bind_int(s:*mut super::Stmt,k:c_int,v:c_int)->c_int;}
    const SQL:&str=include_str!("original-first.sql");
    let q=CString::new(SQL).expect("fixed statement");
    src.at(|db,c|{
        let sha=request(c.birth.pool,c.birth.op,op,actor,approval,scope,login)?;let s=db.prepare(&q)?;
        // All SQLITE_STATIC arrays live beyond the Statement, including sha.
        for(k,b)in [(1,c.birth.pool.0.as_slice()),(2,op.0.as_slice()),(3,actor.as_slice()),(4,sha.as_slice()),
                    (5,c.birth.op.0.as_slice()),(6,approval.as_slice()),(8,login)]{
            check(unsafe{sqlite3_bind_blob(s.0,k,b.as_ptr().cast(),b.len() as i32,None)})?;
        }
        check(unsafe{sqlite3_bind_int(s.0,7,c_int::from(scope.tag()))})?;
        match unsafe{sqlite3_step(s.0)}{101=>Ok(None),100=>{
            let value=(AccountId(s.array(0)?),Management::decode(s.byte(1)?)?);
            match unsafe{sqlite3_step(s.0)}{101=>Ok(Some(value)),rc=>{check(rc)?;Err(invalid("non-unique first receipt"))}}
        },rc=>{check(rc)?;Err(invalid("unexpected first receipt result"))}}
    })
}

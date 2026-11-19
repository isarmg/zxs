//! Bounded account birth in the one SYSTEM envelope. Parsing here does not
//! admit a medium, issue a credential, authenticate a login or authorize I/O.
use super::wire::{self,In};
use crate::{OperationId,PoolId,identity::AccountId,fs::{self,invalid}};
use std::{ffi::{c_char,c_int,c_void,CString},io,sync::Mutex};

pub(crate) const MAX_FIRST:usize=319; // 255 + a 64-byte login
const DOMAIN:&[u8]=b"ZXS/ACCOUNT/1\0";
const REQUEST:&[u8]=b"ZXS/FIRST-ADMIN/REQUEST/1\0";
const KIND:u8=9;
// Binary salt/tag have fixed widths. Nothing in a request can choose PHC
// parameters or ask for more work. Never log the PHC or derive a cheap tag.
pub(crate) struct First{
    pub(crate) account:AccountId,pub(crate) operation:OperationId,pub(crate) decision:[u8;32],login:String,
    salt:[u8;16],tag:[u8;32],
}
impl First{
    pub(crate) fn confirmed(pool:PoolId,init:OperationId,genesis:[u8;32],tail:&[u8],floor:fs::Frontier)->io::Result<Self>{
        if !(256..=MAX_FIRST).contains(&tail.len()){return Err(invalid("not the one original first-account decision"));}
        let e=wire::envelope(pool,tail)?;
        if e.seq!=1||e.previous!=genesis||e.kind!=KIND||floor!=fs::frontier(1,tail.len(),e.digest){
            return Err(invalid("not the confirmed first-account cut of this original pool"));}
        let mut r=In(e.body.strip_prefix(DOMAIN).ok_or_else(||invalid("unknown account decision domain"))?);
        if r.byte()?!=1{return Err(invalid("unsupported original account action"));}
        let operation=OperationId(r.id()?);let account=AccountId(r.id()?);let operator_ref=r.id()?;
        let name=r.short_buf()?;
        if operation==init||!(1..=64).contains(&name.len())||!name[0].is_ascii_alphanumeric()||
            !name.iter().all(|c|c.is_ascii_lowercase()||c.is_ascii_digit()||b"._-".contains(c)){
            return Err(invalid("invalid first-account operation or exact login name"));}
        let salt=r.fixed()?;let tag=r.fixed()?;r.end()?;
        if original_request(pool,operation,account,operator_ref,name)?!=e.request{
            return Err(invalid("altered original first-account request"));}
        Ok(Self{account,operation,decision:e.digest,login:std::str::from_utf8(name).expect("checked ASCII login").into(),salt,tag})
    }
    /// Actual upstream KDF verification, *not* a bearer grant. A named native
    /// prefix must still be checked after this slower, short-lock-free step.
    pub(crate) fn password_matches(&self,login:&str,password:&str)->io::Result<bool>{
        if login.len()>64||password.is_empty()||password.len()>1024{return Ok(false);}
        // A single process can allocate at most one 64 MiB verification at once.
        // Per-client/remote login throttling remains the network endpoint's job.
        static KDF:Mutex<()>=Mutex::new(());
        let _slot=KDF.try_lock().map_err(|_|io::Error::new(io::ErrorKind::WouldBlock,"password verifier unavailable"))?;
        let encoded=CString::new(format!("$argon2id$v=19$m=65536,t=3,p=4${}${}",b64(&self.salt),b64(&self.tag)))
            .expect("canonical PHC contains no NUL");
        // SAFETY: libargon2 reads the NUL-terminated bounded PHC and exactly
        // the borrowed password bytes synchronously. It compares tags itself.
        let rc=unsafe{argon2id_verify(encoded.as_ptr(),password.as_ptr().cast(),password.len())};
        match rc{0=>Ok(self.login.as_bytes()==login.as_bytes()),-35=>Ok(false),
            _=>Err(io::Error::other("password verifier unavailable"))}
    }
}
fn original_request(pool:PoolId,op:OperationId,account:AccountId,operator:[u8;16],name:&[u8])->io::Result<[u8;32]>{
    let mut b=REQUEST.to_vec();b.extend_from_slice(&pool.0);b.extend_from_slice(&op.0);b.extend_from_slice(&account.0);
    b.extend_from_slice(&operator);b.push(name.len() as u8);b.extend_from_slice(name);crate::hash::digest(&b)
}
fn b64(raw:&[u8])->String{
    const ABC:&[u8;64]=b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out=String::new();
    for part in raw.chunks(3){
        let b=|n:usize|u32::from(part.get(n).copied().unwrap_or(0));
        let word=b(0)<<16|b(1)<<8|b(2);
        for shift in [18,12,6,0].into_iter().take(part.len()+1){out.push(char::from(ABC[((word>>shift)&63) as usize]));}
    }
    out
}
#[link(name="argon2",kind="dylib")]
unsafe extern "C"{
    fn argon2id_verify(encoded:*const c_char,pwd:*const c_void,pwdlen:usize)->c_int;
}

#[cfg(test)]
pub(crate) fn fixture_frame(pool:PoolId,init:OperationId,genesis:[u8;32],op:OperationId,account:AccountId,
    reference:[u8;16],login:&[u8],salt:[u8;16],tag:[u8;32])->(Vec<u8>,fs::Frontier){
    let mut b=DOMAIN.to_vec();b.push(1);b.extend_from_slice(&op.0);b.extend_from_slice(&account.0);b.extend_from_slice(&reference);
    b.push(login.len() as u8);b.extend_from_slice(login);b.extend_from_slice(&salt);b.extend_from_slice(&tag);
    let(f,sum)=wire::encode(pool,1,genesis,KIND,original_request(pool,op,account,reference,login).unwrap(),&b).unwrap();
    let floor=fs::frontier(1,f.len(),sum);First::confirmed(pool,init,genesis,&f,floor).unwrap();(f,floor)
}

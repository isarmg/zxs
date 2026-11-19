//! A bounded private verifier selected by the confirmed portable account
//! decision. Does not establish an account, management scope, or FILE right.
use super::{fixed,take};
use crate::{OperationId,identity::AccountId,fs::invalid};
use std::{ffi::{c_char,c_int,c_void,CString},io,sync::Mutex};

pub(super) struct Account{
    pub(super) account:AccountId,pub(super) operation:OperationId,pub(super) decision:[u8;32],
    login:String,salt:[u8;16],tag:[u8;32],
}
impl Account{
    pub(super) fn credential(account:AccountId,operation:OperationId,decision:[u8;32],mut bytes:&[u8])->io::Result<Self>{
        let len=usize::from(fixed::<1>(&mut bytes)?[0]);
        if !(1..=64).contains(&len){return Err(invalid("invalid account login length"));}
        let name=take(&mut bytes,len)?;
        if !name[0].is_ascii_alphanumeric()||!name.iter().all(|c|c.is_ascii_lowercase()||c.is_ascii_digit()||b"._-".contains(c)){
            return Err(invalid("invalid exact account login"));}
        let salt=fixed(&mut bytes)?;let tag=fixed(&mut bytes)?;
        if !bytes.is_empty(){return Err(invalid("trailing account credential data"));}
        Ok(Self{account,operation,decision,login:std::str::from_utf8(name).expect("checked ASCII login").into(),salt,tag})
    }
    pub(super) fn login(&self)->&str{&self.login}
    /// Actual upstream KDF verification, *not* a bearer grant. A named native
    /// prefix must still be checked after this slower, short-lock-free step.
    pub(super) fn password_matches(&self,login:&str,password:&str)->io::Result<bool>{
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

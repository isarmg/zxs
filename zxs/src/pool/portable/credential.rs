//! Test-only adapter for the retired portable decision-stream format.
use super::{fixed,take};
use crate::{OperationId,identity::AccountId,fs::invalid,password::{Verifier,login_name}};
use std::io;
pub(super) struct Account{
    pub(super) account:AccountId,pub(super) operation:OperationId,pub(super) decision:[u8;32],
    login:String,verifier:Verifier,
}
impl Account{
    pub(super) fn credential(account:AccountId,operation:OperationId,decision:[u8;32],mut bytes:&[u8])->io::Result<Self>{
        let len=usize::from(fixed::<1>(&mut bytes)?[0]);
        let login=login_name(take(&mut bytes,len)?)?.to_owned();
        let verifier=Verifier::new(fixed(&mut bytes)?,fixed(&mut bytes)?);
        if !bytes.is_empty(){return Err(invalid("trailing account credential data"));}
        Ok(Self{account,operation,decision,login,verifier})
    }
    pub(super) fn login(&self)->&str{&self.login}
    pub(super) fn password_matches(&self,login:&str,password:&str)->io::Result<bool>{
        if login.len()>64{return Ok(false);}
        Ok(self.verifier.matches(password)? && self.login.as_bytes()==login.as_bytes())
    }
}

//! Public comparison bytes for the fresh single-account successor. No
//! database/media operation, password, verifier, role grant, or launch path.
#![allow(dead_code)] // The new reader/writer and launch must agree before admission.
use crate::{fs::invalid,hash::Sha256,password::login_name};
use std::io;

type Id=[u8;16];
const ACTOR:&[u8]=b"ZXS-SQLITE-1/ACCOUNT-ACTOR-1\0";
const SETUP:&[u8]=b"ZXS-SQLITE-1/SHARED-SETUP-1\0";
const RESET:&[u8]=b"ZXS-SQLITE-1/SHARED-RESET-1\0";
pub(crate) const VAULT_MAGIC:&[u8;16]=b"ZXS-RUST-SMB-V2\0";
pub(crate) const VAULT_NAME:&std::ffi::CStr=c"credential.v2";
pub(crate) const VAULT_PREFIX:usize=224;
pub(crate) const VAULT_LENGTH:usize=VAULT_PREFIX+16;

const SID_PREFIX:[u8;12]=[1,5,0,0,0,0,0,5,21,0,0,0];

#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) struct PublicEpoch{
    pub(super) op:Id,
    pub(super) generation:i64,
    pub(super) record:Id,
}
#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) struct RoleKeys{
    pub(super) web:Id,
    pub(super) smb:Id,
}

/// All IDs, digests and the actor must be separately proved in the same
/// original-S cut. These canonical bytes cannot establish their provenance.
#[derive(Clone,Copy)]
pub(super) struct SharedScope<'a>{
    pub(super) pool:Id,
    pub(super) init:Id,
    pub(super) setup:Id,
    pub(super) instance:Id,
    pub(super) bind:Id,
    pub(super) account:Id,
    pub(super) init_sha:[u8;32],
    pub(super) plan_sha:[u8;32],
    pub(super) original_actor:Id,
    pub(super) login:&'a [u8],
    pub(super) sid:[u8;28],
}
fn bad()->io::Error{invalid("invalid original shared-account comparison facts")}
impl SharedScope<'_>{
    fn installation(&self)->[Id;5]{[self.pool,self.init,self.setup,self.instance,self.bind]}
    fn ids(&self)->[Id;6]{[self.pool,self.init,self.setup,self.instance,self.bind,self.account]}
    pub(super) fn role_keys(&self)->io::Result<RoleKeys>{
        let fixed=self.installation();
        if self.ids().contains(&[0;16])||
            fixed.iter().enumerate().any(|(i,id)|fixed[..i].contains(id))||
            self.original_actor==[0;16]||
            self.account==self.original_actor||self.init_sha==[0;32]||self.plan_sha==[0;32]||
            matches!(login_name(self.login)?,"guest"|"anonymous"|"nobody"|"root")||
            self.sid[..12]!=SID_PREFIX||self.sid[12..24]==[0;12]||
            self.sid[24..]!=1000u32.to_le_bytes(){return Err(bad());}
        let actor=|r:u8|->io::Result<Id>{
            let mut hash=Sha256::new();
            hash.update(ACTOR)?;hash.update(&self.account)?;hash.update(&[r])?;
            Ok(hash.finish()[..16].try_into().expect("fixed actor prefix"))
        };
        let keys=RoleKeys{web:actor(0)?,smb:actor(1)?};
        let invalid=[[0;16],self.original_actor,self.account];
        if keys.web==keys.smb||invalid.contains(&keys.web)||invalid.contains(&keys.smb){return Err(bad());}
        Ok(keys)
    }
    fn request(&self,domain:&[u8],approval:Id)->io::Result<Sha256>{
        self.role_keys()?;
        if approval==[0;16]{return Err(bad());}
        let mut h=Sha256::new();
        h.update(domain)?;
        for id in self.ids(){h.update(&id)?;}
        h.update(&self.init_sha)?;h.update(&self.plan_sha)?;
        h.update(&[self.login.len() as u8])?;h.update(self.login)?;h.update(&self.sid)?;
        h.update(&self.original_actor)?;h.update(&approval)?;
        Ok(h)
    }
    pub(super) fn setup_request(&self,approval:Id,first:Id)->io::Result<[u8;32]>{
        if first==[0;16]||approval==self.setup{return Err(bad());}
        let mut h=self.request(SETUP,approval)?;
        h.update(&first)?;Ok(h.finish())
    }
    pub(super) fn reset_request(&self,approval:Id,old:PublicEpoch,new:PublicEpoch)->io::Result<[u8;32]>{
        if approval==new.op||!self.epoch_shape(old)||!self.epoch_shape(new)||
            old.op==new.op||old.record==new.record||old.generation.checked_add(1)!=Some(new.generation){
            return Err(bad());
        }
        let mut h=self.request(RESET,approval)?;
        for id in [new.op,old.op,old.record]{h.update(&id)?;}
        h.update(&old.generation.to_be_bytes())?;h.update(&new.record)?;
        Ok(h.finish())
    }
    fn epoch_shape(&self,e:PublicEpoch)->bool{
        e.record!=[0;16]&&
            match e.generation{
                1=>e.op==self.setup,
                2.. =>e.op!=[0;16]&&!self.installation().contains(&e.op),
                _=>false,
            }
    }
    /// Exact public prefix only; no parameter can supply the password-equivalent key.
    pub(super) fn vault_prefix(&self,epoch:PublicEpoch)->io::Result<[u8;VAULT_PREFIX]>{
        self.role_keys()?;
        if !self.epoch_shape(epoch){return Err(bad());}
        let mut b=[0;VAULT_PREFIX];
        b[..16].copy_from_slice(VAULT_MAGIC);
        b[16]=1; // only the SMB verifier record, never a Web verifier
        b[17]=self.login.len() as u8;
        let ids=[self.pool,self.init,self.setup,self.instance,self.bind,self.account,epoch.op,epoch.record];
        for (out,id) in b[24..152].as_chunks_mut::<16>().0.iter_mut().zip(ids){*out=id;}
        b[152..160].copy_from_slice(&epoch.generation.to_be_bytes());
        b[160..160+self.login.len()].copy_from_slice(self.login);
        Ok(b)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/attached/system/shared_account/tests.rs"]
mod tests;

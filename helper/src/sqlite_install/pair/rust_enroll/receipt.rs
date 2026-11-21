//! Nonsecret observations of the exact authorized transaction cut; none
//! grants permission or represents a received NTLM credential.
use super::{bad,Id,manifest::seal_bytes};
use crate::{smb_profile::rust_first::RustPlan,sqlite_install::pair::native_sql::rust_vault::ActiveRustMeta};
use std::{fs::File,io};

const MAGIC:&[u8;16]=b"ZXS-RUST-OBS-01\0";
const PART:usize=252;
const LIMIT:usize=16+49+2*PART;
fn no()->io::Error{bad("wrong exact public Rust credential observation")}

pub(in crate::sqlite_install::pair) struct Observed{
    pub(in crate::sqlite_install::pair) state:u8,
    pub(in crate::sqlite_install::pair) action:[u8;32],
    pub(in crate::sqlite_install::pair) target:ActiveRustMeta,
    pub(in crate::sqlite_install::pair) previous:Option<ActiveRustMeta>,
    pub(in crate::sqlite_install::pair) web_account:Option<Id>,
}
impl Observed {
    pub(in crate::sqlite_install::pair) fn encode(&self)->io::Result<Vec<u8>>{
        if !(1..=3).contains(&self.state)||self.action==[0;32]||
            self.previous.is_some()!=(self.target.generation>1)||self.web_account.is_some()==self.previous.is_some()||
            self.web_account.is_some_and(|id|id==[0;16]||id==self.target.account){return Err(no());}
        let mut b=Vec::with_capacity(LIMIT);
        b.extend(MAGIC);b.push(self.state);b.extend(self.action);b.extend(self.web_account.unwrap_or([0;16]));
        for v in std::iter::once(&self.target).chain(self.previous.iter()){
            b.extend(v.record_prefix()?);b.extend(v.sid);
        }
        Ok(b)
    }
    pub(in crate::sqlite_install::pair) fn read(file:&File,plan:&RustPlan)->io::Result<Self>{
        Self::decode(&seal_bytes(file,LIMIT)?,plan)
    }
    pub(super) fn decode(bytes:&[u8],plan:&RustPlan)->io::Result<Self>{
        if bytes.len()>LIMIT{return Err(no());}
        let b=bytes.strip_prefix(MAGIC).ok_or_else(no)?;
        let (h,r)=b.split_at_checked(49).ok_or_else(no)?;
        let state=h[0];let action=h[1..33].try_into().map_err(|_|no())?;
        let web_id:Id=h[33..49].try_into().map_err(|_|no())?;
        if !(1..=3).contains(&state)||action==[0;32]{return Err(no());}
        let (after,before)=r.split_at_checked(PART).ok_or_else(no)?;
        let target=public(after,plan)?;
        let previous=match before.len(){
            0 if target.generation==1=>None,
            PART=>{
                let v=public(before,plan)?;
                if v.account!=target.account||v.login!=target.login||v.sid!=target.sid||
                   v.generation.checked_add(1)!=Some(target.generation)||v.op==target.op||v.record==target.record{return Err(no());}
                Some(v)
            }
            _=>return Err(no()),
        };
        let web_account=(web_id!=[0;16]).then_some(web_id);
        if web_account.is_some()==previous.is_some()||web_account==Some(target.account){return Err(no());}
        Ok(Self{state,action,target,previous,web_account})
    }
}
fn public(raw:&[u8],plan:&RustPlan)->io::Result<ActiveRustMeta>{
    let (b,sid)=raw.split_at_checked(224).ok_or_else(no)?;
    let id=|at:usize|->io::Result<Id>{b[at..at+16].try_into().map_err(|_|no())};
    let o=plan.origin;
    if [id(24)?,id(40)?,id(56)?,id(72)?,id(88)?]!=[o.pool,o.init,o.setup,o.instance,o.bind]{return Err(no());}
    let len=usize::from(b[17]);let login=b.get(160..160+len).ok_or_else(no)?.to_vec();
    let revision=ActiveRustMeta{pool:o.pool,init:o.init,setup:o.setup,descriptor_sha:plan.sha(),instance:o.instance,
        bind:o.bind,login,sid:sid.try_into().map_err(|_|no())?,account:id(104)?,op:id(120)?,record:id(136)?,
        generation:i64::from_be_bytes(b[152..160].try_into().map_err(|_|no())?)};
    if revision.record_prefix()?.as_slice()!=b{return Err(no());}
    Ok(revision)
}

//! Exact nonsecret local request. This memfd cannot authorize the worker; its
//! independent parent challenge must succeed before it looks up SYSTEM.
use super::{Intent,Id,bad};
use crate::{path_rules::Kind,sqlite_install::pair::{login,distinct_roles,native_sql::{Binding,Birth,Reset,transaction::Order}}};
use rustix::fs::{SealFlags,fcntl_get_seals};
use sha2::{Digest,Sha256};
use std::{fs::File,io,os::unix::fs::{FileExt,MetadataExt},path::PathBuf};

const PREFIX:&[u8;8]=b"NORDV7\0\0";
const MAX:usize=PREFIX.len()+3+32+7*16+32+128+24+4*16+8+2+4096+2*65;
#[derive(Clone,Debug)]
pub(in crate::sqlite_install::pair) enum Role{Birth{web:Vec<u8>,smb:Vec<u8>},Reset{setup:Id,account:Id,previous:Id,generation:i64,smb:Vec<u8>}}
pub(super) struct Manifest{
    pub(super) intent:Intent,pub(super) invocation:[u8;32],
    pub(super) pool:Id,pub(super) init:Id,pub(super) instance:Id,pub(super) provision:Id,
    pub(super) op:Id,pub(super) actor:Id,pub(super) approval:Id,pub(super) live:[u8;32],pub(super) bound:[u8;128],
    pub(super) system:PathBuf,pub(super) origin:[u64;3],pub(super) role:Role,
}
impl Manifest{
    pub(super) fn encode(&self)->io::Result<Vec<u8>>{
        let path=self.system.to_str().ok_or_else(error)?.as_bytes();
        Kind::StorageRoot.check_path(path).map_err(bad)?;
        let mut v=Vec::with_capacity(MAX);
        v.extend_from_slice(PREFIX);
        v.push(match self.intent{Intent::Lookup=>0,Intent::New=>1,Intent::Resume=>2});
        v.extend_from_slice(&self.invocation);
        for id in [self.pool,self.init,self.instance,self.provision,self.op,self.actor,self.approval]{v.extend_from_slice(&id);}
        v.extend_from_slice(&self.live);v.extend_from_slice(&self.bound);
        for n in self.origin{v.extend_from_slice(&n.to_be_bytes());}
        v.extend_from_slice(&(path.len()as u16).to_be_bytes());v.extend_from_slice(path);
        fn name(v:&mut Vec<u8>,n:&[u8])->io::Result<()>{
            if !login(n){return Err(error());}v.push(n.len()as u8);v.extend_from_slice(n);Ok(())
        }
        match &self.role{
            Role::Birth{web,smb}=>{v.push(1);name(&mut v,web)?;name(&mut v,smb)?;}
            Role::Reset{setup,account,previous,generation,smb}=>{
                v.push(2);
                for id in [setup,account,previous]{v.extend_from_slice(id);}
                v.extend_from_slice(&generation.to_be_bytes());name(&mut v,smb)?;
            }
        }
        self.validate()?;
        Ok(v)
    }
    pub(super) fn scope(&self)->io::Result<[u8;32]>{
        let mut h=Sha256::new();h.update(b"helper original offline v7 native scope\0");h.update(self.encode()?);Ok(h.finalize().into())
    }
    fn parse(bytes:&[u8])->io::Result<Self>{
        if bytes.len()>MAX{return Err(error());}
        let mut remaining=bytes.strip_prefix(PREFIX).ok_or_else(error)?;
        fn take<'a>(rest:&mut &'a [u8],n:usize)->io::Result<&'a [u8]>{
            let (a,b)=rest.split_at_checked(n).ok_or_else(error)?;*rest=b;Ok(a)
        }
        fn fixed<const N:usize>(rest:&mut &[u8])->io::Result<[u8;N]>{
            Ok(take(rest,N)?.try_into().expect("fixed width"))
        }
        fn name(rest:&mut &[u8])->io::Result<Vec<u8>>{
            let n=usize::from(fixed::<1>(rest)?[0]);let b=take(rest,n)?;
            if !login(b){return Err(error());}Ok(b.to_vec())
        }
        let b=&mut remaining;
        let intent=match fixed::<1>(b)?[0]{0=>Intent::Lookup,1=>Intent::New,2=>Intent::Resume,_=>return Err(error())};
        let invocation=fixed(b)?;let pool=fixed(b)?;let init=fixed(b)?;let instance=fixed(b)?;
        let provision=fixed(b)?;let op=fixed(b)?;let actor=fixed(b)?;let approval=fixed(b)?;
        let live=fixed(b)?;let bound=fixed(b)?;
        let origin=[u64::from_be_bytes(fixed(b)?),u64::from_be_bytes(fixed(b)?),u64::from_be_bytes(fixed(b)?)];
        let len=usize::from(u16::from_be_bytes(fixed(b)?));let path=take(b,len)?;
        Kind::StorageRoot.check_path(path).map_err(bad)?;
        let system=PathBuf::from(std::str::from_utf8(path).map_err(|_|error())?);
        let role=match fixed::<1>(b)?[0]{
            1=>Role::Birth{web:name(b)?,smb:name(b)?},
            2=>Role::Reset{setup:fixed(b)?,account:fixed(b)?,previous:fixed(b)?,generation:i64::from_be_bytes(fixed(b)?),smb:name(b)?},
            _=>return Err(error()),
        };
        if !b.is_empty(){return Err(error());}
        let manifest=Self{intent,invocation,pool,init,instance,provision,op,actor,approval,live,bound,system,origin,role};
        manifest.validate()?;Ok(manifest)
    }
    fn validate(&self)->io::Result<()>{
        if self.invocation==[0;32]||self.live==[0;32]||self.origin[1]==0||self.origin[2]==0
            ||self.op==self.provision||self.provision==self.init{return Err(error());}
        self.order().map(|_|())
    }
    pub(super) fn order(&self)->io::Result<Order>{
        let b=Binding::expected(&self.bound,&self.pool,&self.instance,&self.provision,&self.live)?;
        match &self.role{
            Role::Birth{web,smb}=>Ok(Order::Setup(Birth::new(self.pool,self.init,self.op,self.actor,self.approval,b,web,smb)?)),
            Role::Reset{setup,account,previous,generation,smb} if distinct_roles(&[],smb)=>
                Ok(Order::Reset(Reset::smb(self.pool,self.init,*setup,self.op,self.actor,self.approval,*account,*previous,*generation,b,smb)?)),
            _=>Err(error()),
        }
    }
    pub(super) fn read(f:&File)->io::Result<Self>{
        let m=f.metadata()?;
        if !m.is_file()||m.uid()!=0||m.gid()!=0||m.nlink()!=0||m.mode()&0o111!=0||m.len()==0||m.len()>MAX as u64
            ||!fcntl_get_seals(f)?.contains(SealFlags::SEAL|SealFlags::SHRINK|SealFlags::GROW|SealFlags::WRITE|SealFlags::EXEC){return Err(error());}
        let mut v=vec![0;m.len()as usize];f.read_exact_at(&mut v,0)?;
        loop{match f.read_at(&mut [0],m.len()){
            Ok(0)=>break,Err(e)if e.kind()==io::ErrorKind::Interrupted=>continue,_=>return Err(error()),
        }}
        Self::parse(&v)
    }
}
fn error()->io::Error{bad("not the exact sealed nonsecret offline operation")}

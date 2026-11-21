//! Exact nonsecret order from the original root account owner. Decode is
//! not approval. The child still proves ancestry, parent and the lent S.
use super::{Id,bad};
use crate::sqlite_install::pair::fresh_issuer::CredentialScope;
use crate::{path_rules::Kind,smb_profile::rust_first::{MAX_WIRE,RustPlan},
    sqlite_install::pair::{distinct_roles,login,native_sql::rust_vault::{Desired,RustOrder}}};
use rustix::fs::{SealFlags,fcntl_get_seals};
use sha2::{Digest,Sha256};
use std::{fs::File,io,os::unix::fs::{FileExt,MetadataExt},path::PathBuf};

const TAG:&[u8;18]=b"ZXS-RUST-ENROLL-1\0";
const LIMIT:usize=TAG.len()+2+32+3*16+3*8+2+4095+2+MAX_WIRE+2*65+CredentialScope::WIDTH;
fn no()->io::Error{bad("wrong sealed original Rust account request")}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::sqlite_install::pair) enum Intent{Lookup,Fresh,Resume}
pub(in crate::sqlite_install::pair) struct Manifest{
    pub(in crate::sqlite_install::pair) intent:Intent,
    pub(in crate::sqlite_install::pair) nonce:[u8;32],
    pub(in crate::sqlite_install::pair) system:PathBuf,
    pub(in crate::sqlite_install::pair) entity:[u64;3],
    pub(in crate::sqlite_install::pair) plan:RustPlan,
    pub(in crate::sqlite_install::pair) order:RustOrder,
    pub(in crate::sqlite_install::pair) fresh:Option<CredentialScope>,
}
impl Manifest{
    fn require(&self)->io::Result<()>{
        let o=self.plan.origin;let w=&self.order;
        let path=self.system.to_str().ok_or_else(no)?.as_bytes();
        Kind::StorageRoot.check_path(path).map_err(bad)?;
        if path.len()>4095||self.entity[1]==0||self.entity[2]==0||self.nonce==[0;32]||
            [w.op,w.actor,w.approval].contains(&[0;16])||w.op==o.init{return Err(no());}
        if let Some(new)=self.fresh{
            if self.intent!=Intent::Fresh||!matches!(&w.role,Desired::Setup{..})||new.subject.tuple().0!=w.actor{
                return Err(no());
            }
        }
        match &w.role{
            Desired::Setup{web,smb} if w.op==o.setup&&login(web)&&login(smb)&&distinct_roles(web,smb)=>Ok(()),
            Desired::Reset{previous,generation} if ![o.init,o.bind,w.op,[0;16]].contains(previous)&&
                ![o.setup,o.bind].contains(&w.op)&&(1..i64::MAX).contains(generation)&&
                (*previous==o.setup)==(*generation==1)=>Ok(()),
            _=>Err(no()),
        }
    }
    pub(in crate::sqlite_install::pair) fn encode(&self)->io::Result<Vec<u8>>{
        self.require()?;
        let path=self.system.to_str().ok_or_else(no)?.as_bytes();
        let mut e=Vec::with_capacity(LIMIT);e.extend_from_slice(TAG);
        e.push(match self.intent{Intent::Lookup=>0,Intent::Fresh=>1,Intent::Resume=>2});
        e.extend(self.nonce);
        for id in [self.order.actor,self.order.approval,self.order.op]{e.extend(id);}
        for n in self.entity{e.extend(n.to_be_bytes());}
        e.extend((path.len()as u16).to_be_bytes());e.extend(path);
        let raw=self.plan.wire();
        e.extend((raw.len()as u16).to_be_bytes());e.extend(raw);
        match &self.order.role{
            Desired::Setup{web,smb}=>{
                e.push(1);
                for n in [web,smb]{e.push(n.len()as u8);e.extend(n);}
            }
            Desired::Reset{previous,generation}=>{
                e.push(2);e.extend(previous);e.extend(generation.to_be_bytes());
            }
        }
        if let Some(fresh)=self.fresh{fresh.encode(&mut e)?;}
        if e.len()>LIMIT{return Err(no());}
        Ok(e)
    }
    pub(in crate::sqlite_install::pair) fn scope(&self)->io::Result<[u8;32]>{
        let mut h=Sha256::new();h.update(b"ZXS-HELPER/RUST-ACCOUNT-SCOPE-1\0");h.update(self.encode()?);Ok(h.finalize().into())
    }
    pub(in crate::sqlite_install::pair) fn read(file:&File)->io::Result<Self>{decode(&seal_bytes(file,LIMIT)?)}
}

pub(in crate::sqlite_install::pair) fn seal_bytes(file:&File,limit:usize)->io::Result<Vec<u8>>{
    let m=file.metadata()?;
    let seals=fcntl_get_seals(file)?;
    let needed=SealFlags::WRITE|SealFlags::GROW|SealFlags::SHRINK|SealFlags::SEAL;
    let n=usize::try_from(m.len()).ok().filter(|n|(1..=limit).contains(n)).ok_or_else(no)?;
    if !m.is_file()||m.nlink()!=0||!seals.contains(needed){return Err(no());}
    let mut out=vec![0;n];file.read_exact_at(&mut out,0)?;
    if file.read_at(&mut [0;1],n as u64)?!=0{return Err(no());}
    Ok(out)
}
pub(in crate::sqlite_install::pair) fn decode(raw:&[u8])->io::Result<Manifest>{
    if raw.len()>LIMIT{return Err(no());}
    let mut b=Cursor(raw.strip_prefix(TAG).ok_or_else(no)?);
    let intent=match b.byte()?{0=>Intent::Lookup,1=>Intent::Fresh,2=>Intent::Resume,_=>return Err(no())};
    let nonce=b.fixed()?;
    let actor=b.fixed()?;let approval=b.fixed()?;let op=b.fixed()?;
    let entity=[u64::from_be_bytes(b.fixed()?),u64::from_be_bytes(b.fixed()?),u64::from_be_bytes(b.fixed()?)];
    let system=PathBuf::from(std::str::from_utf8(b.counted(4095)?).map_err(|_|no())?);
    let plan=RustPlan::decode(b.counted(MAX_WIRE)?).map_err(bad)?;
    let role=match b.byte()?{
        1=>Desired::Setup{web:b.name()?,smb:b.name()?},
        2=>Desired::Reset{previous:b.fixed::<16>()?,generation:i64::from_be_bytes(b.fixed()?)},
        _=>return Err(no()),
    };
    let fresh=if b.0.is_empty(){None}else{Some(CredentialScope::decode(b.0)?)};
    let m=Manifest{intent,nonce,system,entity,plan,order:RustOrder{actor,approval,op,role},fresh};
    m.require()?;Ok(m)
}
struct Cursor<'a>(&'a[u8]);
impl<'a> Cursor<'a>{
    fn take(&mut self,n:usize)->io::Result<&'a[u8]>{
        let (a,b)=self.0.split_at_checked(n).ok_or_else(no)?;self.0=b;Ok(a)
    }
    fn fixed<const N:usize>(&mut self)->io::Result<[u8;N]>{
        self.take(N)?.try_into().map_err(|_|no())
    }
    fn byte(&mut self)->io::Result<u8>{let [n]=self.fixed()?;Ok(n)}
    fn counted(&mut self,limit:usize)->io::Result<&'a[u8]>{
        let n=usize::from(u16::from_be_bytes(self.fixed()?));
        if !(1..=limit).contains(&n){return Err(no());}self.take(n)
    }
    fn name(&mut self)->io::Result<Vec<u8>>{
        let n=usize::from(self.byte()?);let b=self.take(n)?;
        if !login(b){return Err(no());}Ok(b.to_vec())
    }
}

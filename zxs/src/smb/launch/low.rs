//! One-shot S-only startup query and its strictly bound public reply. The
//! distinct low child must carry actual inherited parent proof before read;
//! a parsed descriptor, digest or decoded receipt is not launch authority.
#![allow(dead_code)] // The matching same-ELF original parent is a separate slice.
use super::{Fault,Result,entry::InstalledTerm,transport::{InheritedControl,seal_public}};
use crate::{Entity,VolumeId,fs::{self,ComponentLimit,NameBudget},hash::digest,system::{ROOT_MARK,
    reader::{InitialReader,smb::runtime::{PreflightClaim,PreflightEpoch,PreflightPlan,PreflightRead,PreflightSelection,preflight_read,web_reader}}}};
use std::{collections::BTreeSet,ffi::{OsStr,c_int,c_long},io,net::IpAddr,os::unix::ffi::OsStrExt,path::{Path,PathBuf},
    time::{Duration,Instant}};

const START:&[u8;16]=b"ZXS-RUST/PREFL1\0";
const DONE:&[u8;16]=b"ZXS-RUST/PFDONE\0";
const INPUT:&[u8]=b"ZXS-RUST/PF-ENV-1\0";
const OUTPUT:&[u8]=b"ZXS-RUST/PF-OUT-1\0";
const PATH:usize=4096;
const PLAN:usize=17_885;
const FOLDERS:usize=62;
pub(super) const QUESTION_MAX:usize=INPUT.len()+32+32+32+16+8+16+2+PATH+2+PLAN;
pub(super) const REPLY_MAX:usize=OUTPUT.len()+32+80+32+16*3+28+8+1+64+1+(FOLDERS+1)*(2+2+PATH+101);
pub(super) const FRAME:usize=80;
const WAIT:Duration=Duration::from_secs(10);

/// Only the guarded process entry may take the original two FDs. This
/// distinct role never receives a vault or listening socket and exits
/// after its authenticated read-only receipt, not into the server or Web.
pub(super) unsafe fn enter(term:InstalledTerm)->Result<()>{
    if term.stopping(){return Err(Fault::Handoff);}
    let until=Instant::now().checked_add(WAIT).ok_or(Fault::Handoff)?;
    // SAFETY: the dedicated argv0 path has sole early fd3/fd4 ownership.
    let inherited=unsafe{InheritedControl::inherit()}?;
    let (verified,frame,rights)=inherited.recv_once::<FRAME>(until)?;
    require_reduced(verified.subject())?;
    let original=||->io::Result<()>{
        if term.stopping()||Instant::now()>=until{return Err(io::ErrorKind::PermissionDenied.into());}
        verified.require_live().map_err(|_|io::ErrorKind::PermissionDenied.into())
    };
    original().map_err(|_|Fault::Handoff)?;
    let question=PreflightQuestion::decode_request(&frame,rights.only_sealed::<QUESTION_MAX>()?,verified.subject())?;
    let selected=question.select(original)?;
    let (frame,body)=question.build_reply(&selected)?;
    let sealed=seal_public(&body,REPLY_MAX)?;
    original().map_err(|_|Fault::Handoff)?;
    verified.send_sealed_reply(&frame,&sealed,until)?;
    if term.stopping(){return Err(Fault::Handoff);}Ok(())
}

unsafe extern "C"{
    fn observe_own_user_triplet(out:*mut[u32;3])->c_int;
    fn observe_own_group_triplet(out:*mut[u32;3])->c_int;
    fn observe_own_group_count()->c_int;
    fn observe_own_capability_words(out:*mut[[u32;3];2])->c_long;
}

pub(super) fn require_reduced((uid,gid):(u32,u32))->Result<()>{
    // Preflight, Web and SMB inherited entries require the same
    // reduced-subject evidence alongside their transport checks.
    let mut uids=[0;3];
    let mut gids=[0;3];
    let mut caps=[[0;3];2];
    // SAFETY: this task's read-only queries use live fixed-width output arrays.
    unsafe{
        if observe_own_user_triplet(&raw mut uids)!=0||uids!=[uid;3]||
            observe_own_group_triplet(&raw mut gids)!=0||gids!=[gid;3]||
            observe_own_group_count()!=0||
            observe_own_capability_words(&raw mut caps)!=0||
            caps.iter().any(|[effective,permitted,inheritable]|*effective!=0||*permitted!=0||*inheritable!=0){
            return Err(Fault::Handoff);
        }
    }
    Ok(())
}

fn hash(raw:&[u8])->Result<[u8;32]>{
    let sum=digest(raw).map_err(|_|Fault::Shape)?;
    if sum==[0;32]{return Err(Fault::Shape);}Ok(sum)
}
fn frame(magic:&[u8;16],nonce:[u8;32],digest:[u8;32])->[u8;FRAME]{
    let mut out=[0;FRAME];out[..16].copy_from_slice(magic);out[16..48].copy_from_slice(&nonce);out[48..].copy_from_slice(&digest);out
}
fn path_bytes(path:&Path)->Result<&[u8]>{
    fs::check_path(path).map_err(|_|Fault::Shape)?;
    if path.iter().any(|p|p.as_bytes().len()>255){return Err(Fault::Shape);}
    Ok(path.as_os_str().as_bytes())
}
fn add(out:&mut Vec<u8>,raw:&[u8]){
    out.extend_from_slice(&u16::try_from(raw.len()).expect("already bounded field").to_be_bytes());out.extend_from_slice(raw);
}

/// Exact public fields from the already protected original Site and image.
/// Its parent retains both lifetime leases; none are transferred to low.
pub(super) struct QuestionSource<'a>{
    pub(super) nonce:[u8;32],pub(super) release:[u8;32],pub(super) installed:[u8;32],
    pub(super) init_actor:[u8;16],pub(super) uid:u32,pub(super) gid:u32,
    pub(super) entity:Entity,pub(super) system:&'a Path,pub(super) install:&'a [u8],
}
pub(super) struct PreflightQuestion{
    raw:Vec<u8>,nonce:[u8;32],request:[u8;32],release:[u8;32],installed:[u8;32],plan:PreflightPlan,
    init_actor:[u8;16],uid:u32,gid:u32,entity:Entity,system:PathBuf,install:Vec<u8>,
}
impl PreflightQuestion{
    pub(super) fn encode(source:QuestionSource<'_>)->Result<Self>{
        let path=path_bytes(source.system)?;
        if source.install.len()>PLAN||path.len()>PATH{return Err(Fault::Shape);}
        let mut out=INPUT.to_vec();
        out.extend_from_slice(&source.nonce);out.extend_from_slice(&source.release);
        out.extend_from_slice(&source.installed);out.extend_from_slice(&source.init_actor);
        out.extend_from_slice(&source.uid.to_be_bytes());out.extend_from_slice(&source.gid.to_be_bytes());
        out.extend_from_slice(&source.entity.device.to_be_bytes());out.extend_from_slice(&source.entity.inode.to_be_bytes());
        add(&mut out,path);add(&mut out,source.install);Self::decode(out)
    }
    fn decode(raw:Vec<u8>)->Result<Self>{
        if raw.len()>QUESTION_MAX{return Err(Fault::Shape);}
        let mut c=Slice(raw.strip_prefix(INPUT).ok_or(Fault::Shape)?);
        let nonce=c.array::<32>()?;let release=c.array::<32>()?;let installed=c.array::<32>()?;
        let init_actor=c.id()?;let uid=c.u32()?;let gid=c.u32()?;
        let entity=Entity{device:c.u64()?,inode:c.u64()?};
        if [nonce,release,installed].contains(&[0;32])||entity.inode==0||
            matches!(uid,0|65534|u32::MAX)||matches!(gid,0|65534|u32::MAX){return Err(Fault::Shape);}
        let system=PathBuf::from(std::str::from_utf8(c.counted(PATH)?).map_err(|_|Fault::Shape)?);
        path_bytes(&system)?;
        let install=c.counted(PLAN)?.to_vec();
        let plan=PreflightPlan::decode(&install,installed).map_err(|_|Fault::Shape)?;
        if !c.0.is_empty(){return Err(Fault::Shape);}
        let request=hash(&raw)?;
        Ok(Self{raw,nonce,request,release,installed,plan,init_actor,uid,gid,entity,system,install})
    }
    pub(super) fn decode_request(header:&[u8;FRAME],raw:Vec<u8>,subject:(u32,u32))->Result<Self>{
        let q=Self::decode(raw)?;
        if &q.start()!=header||q.subject()!=subject{return Err(Fault::Handoff);}Ok(q)
    }
    pub(super) fn subject(&self)->(u32,u32){(self.uid,self.gid)}
    pub(super) fn release(&self)->[u8;32]{self.release}
    /// These are only the immutable approved PLAN spellings. Compare
    /// independently against real local interfaces before binding sockets.
    pub(super) fn endpoints(&self)->&[(IpAddr,u8)]{self.plan.endpoints()}
    pub(super) fn hostname(&self)->&str{self.plan.hostname()}
    pub(super) fn planned_folders(&self)->impl ExactSizeIterator<Item=(VolumeId,&str,bool)>+'_{
        self.plan.planned_folders()
    }
    pub(super) fn recycle_label(&self)->&str{self.plan.recycle_label()}
    pub(super) fn start(&self)->[u8;FRAME]{frame(START,self.nonce,self.request)}
    pub(super) fn bytes(&self)->&[u8]{&self.raw}

    fn read_scope(&self)->PreflightRead<'_>{
        PreflightRead{system:&self.system,original_system:self.entity,install:&self.install,
            approved_install:self.installed,original_init_actor:self.init_actor,storage_uid:self.uid}
    }
    pub(super) fn select(&self,live:impl Fn()->io::Result<()>)->Result<PreflightSelection>{
        preflight_read(self.read_scope(),live).map_err(|_|Fault::Handoff)
    }
    pub(super) fn web_scope(raw:Vec<u8>,subject:(u32,u32))->Result<Self>{
        // The Web-specific outer envelope must already authenticate these
        // exact bytes. Never treat its payload as a preflight transport.
        let q=Self::decode(raw)?;
        if q.subject()!=subject{return Err(Fault::Handoff);}
        Ok(q)
    }
    pub(super) fn web_reader(&self,read_only:bool,live:impl Fn()->io::Result<()>)->Result<(InitialReader,PreflightEpoch)>{
        web_reader(self.read_scope(),read_only,live).map_err(|_|Fault::Handoff)
    }
    pub(super) fn cohort_for(&self,epoch:&PreflightEpoch)->Result<[u8;32]>{
        super::cohort::digest(self.release,self.installed,self.entity,epoch)
    }

    fn approve_shape(&self,reply:&PreflightSelection)->Result<()>{
        if self.plan.origin!=reply.origin||self.installed!=reply.install_sha||self.plan.ordinary.len()!=reply.ordinary.len(){
            return Err(Fault::Shape);
        }
        let e=&reply.epoch;
        if [e.account,e.op,e.record].contains(&[0;16])||e.generation<1||
            (e.generation==1)!=(e.op==reply.origin[4])||!valid_login(&e.login)||
            e.sid[..12]!=[1,5,0,0,0,0,0,5,21,0,0,0]||e.sid[12..24]==[0;12]||e.sid[24..]!=1000u32.to_le_bytes(){
            return Err(Fault::Shape);
        }
        let mut paths=BTreeSet::from([self.system.as_path()]);
        let mut slots=BTreeSet::new();let mut volumes=BTreeSet::new();let mut stamps=BTreeSet::new();
        for (i,row) in reply.ordinary.iter().chain(std::iter::once(&reply.recycle)).enumerate(){
            let role=if let Some(&volume)=self.plan.ordinary.get(i){
                if row.volume!=volume||!(2..=63).contains(&row.ordinal){return Err(Fault::Shape);}2
            }else{if row.ordinal!=1{return Err(Fault::Shape);}1};
            let p=row.path.as_path();path_bytes(p)?;
            let limit=ComponentLimit::from_original(row.component_limit).map_err(|_|Fault::Shape)?;
            NameBudget::from_original(fs::path_budget(p).map_err(|_|Fault::Shape)?,limit).child(ROOT_MARK).map_err(|_|Fault::Shape)?;
            let mark=&row.marker;
            if mark[..20]!=*b"ZXS-SQLITE-1/VOLUME\0"||mark[20]!=role||mark[21..37]!=reply.origin[0]||
                mark[37..53]!=row.volume.0||mark[53..69]!=reply.origin[1]||mark[69..]==[0;32]||
                row.volume.0==[0;16]||row.volume.0==reply.origin[0]||!slots.insert(row.ordinal)||!volumes.insert(row.volume)||
                !stamps.insert(&mark[69..])||paths.iter().any(|other|p.starts_with(other)||other.starts_with(p)){
                return Err(Fault::Shape);
            }
            paths.insert(p);
        }
        Ok(())
    }
    pub(super) fn build_reply(&self,reply:&PreflightSelection)->Result<([u8;FRAME],Vec<u8>)>{
        self.approve_shape(reply)?;
        let mut b=OUTPUT.to_vec();b.extend_from_slice(&self.request);
        for id in reply.origin{b.extend_from_slice(&id);}
        b.extend_from_slice(&reply.install_sha);
        let e=&reply.epoch;
        for id in [e.account,e.op,e.record]{b.extend_from_slice(&id);}
        b.extend_from_slice(&e.sid);b.extend_from_slice(&e.generation.to_be_bytes());
        b.push(e.login.len() as u8);b.extend_from_slice(e.login.as_bytes());b.push(reply.ordinary.len() as u8);
        for r in reply.ordinary.iter().chain(std::iter::once(&reply.recycle)){
            b.extend_from_slice(&[r.ordinal,r.component_limit]);add(&mut b,r.path.as_os_str().as_bytes());b.extend_from_slice(&r.marker);
        }
        if b.len()>REPLY_MAX{return Err(Fault::Shape);}
        Ok((frame(DONE,self.nonce,hash(&b)?),b))
    }
}

/// A shape-checked *description*, not a kernel-corroborated successful
/// preflight. The actual parent still owns the birth/pidfd/exit proof and
/// must verify all named media and current site before attaching anything.
pub(super) struct PublicReply{selection:PreflightSelection}
impl PublicReply{
    pub(super) fn decode_verified(q:&PreflightQuestion,header:&[u8;FRAME],raw:&[u8])->Result<Self>{
        if raw.len()>REPLY_MAX||&frame(DONE,q.nonce,hash(raw)?)!=header{return Err(Fault::Shape);}
        let mut c=Slice(raw.strip_prefix(OUTPUT).ok_or(Fault::Shape)?);
        if c.array::<32>()?!=q.request{return Err(Fault::Shape);}
        let origin=[c.id()?,c.id()?,c.id()?,c.id()?,c.id()?];
        let install_sha=c.array::<32>()?;
        let account=c.id()?;let op=c.id()?;let record=c.id()?;let sid=c.array::<28>()?;
        let generation=i64::from_be_bytes(c.array()?);let n=usize::from(c.byte()?);
        if !(1..=64).contains(&n){return Err(Fault::Shape);}
        let login=std::str::from_utf8(c.take(n)?).map_err(|_|Fault::Shape)?.to_owned();
        let count=usize::from(c.byte()?);
        if !(1..=FOLDERS).contains(&count)||count!=q.plan.ordinary.len(){return Err(Fault::Shape);}
        let ordinary=(0..count).map(|_|c.claim()).collect::<Result<Vec<_>>>()?;
        let recycle=c.claim()?;
        if !c.0.is_empty(){return Err(Fault::Shape);}
        let selection=PreflightSelection{origin,install_sha,epoch:PreflightEpoch{account,op,record,sid,generation,login},ordinary,recycle};
        q.approve_shape(&selection)?;
        Ok(Self{selection})
    }
    pub(super) fn facts(&self)->&PreflightSelection{&self.selection}
}
fn valid_login(name:&str)->bool{
    !matches!(name,"root"|"nobody"|"anonymous"|"guest")&&crate::password::login_name(name.as_bytes()).is_ok()
}

struct Slice<'a>(&'a [u8]);
impl<'a> Slice<'a>{
    fn take(&mut self,n:usize)->Result<&'a[u8]>{
        let (x,rest)=self.0.split_at_checked(n).ok_or(Fault::Shape)?;self.0=rest;Ok(x)
    }
    fn array<const N:usize>(&mut self)->Result<[u8;N]>{Ok(self.take(N)?.try_into().expect("exact length"))}
    fn id(&mut self)->Result<[u8;16]>{let out=self.array::<16>()?;if out==[0;16]{Err(Fault::Shape)}else{Ok(out)}}
    fn byte(&mut self)->Result<u8>{let [v]=self.array()?;Ok(v)}
    fn u32(&mut self)->Result<u32>{Ok(u32::from_be_bytes(self.array()?))}
    fn u64(&mut self)->Result<u64>{Ok(u64::from_be_bytes(self.array()?))}
    fn counted(&mut self,cap:usize)->Result<&'a[u8]>{
        let n=usize::from(u16::from_be_bytes(self.array()?));if n==0||n>cap{return Err(Fault::Shape);}self.take(n)
    }
    fn claim(&mut self)->Result<PreflightClaim>{
        let ordinal=self.byte()?;let component_limit=self.byte()?;
        let path=PathBuf::from(OsStr::from_bytes(self.counted(PATH)?));let marker=self.array::<101>()?;
        let volume=VolumeId(marker[37..53].try_into().expect("claim volume"));
        Ok(PreflightClaim{ordinal,volume,path,component_limit,marker})
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/attached/smb/launch/low/tests.rs"]
pub(super) mod tests;

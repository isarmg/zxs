//! Server-owned NTLMv2 Type 2 for the configured standalone service. No
//! general GSS acceptor, credential enrollment, live session or key export.
use super::{AGREED,ALWAYS_SIGN,FORBIDDEN,MIN,TARGET_INFO,Issued,Pending,ProtectedRevision,Rejected,Result,record};
use crate::{smb::{InheritedRun,local,session::envelope::{ChallengeLayer,HaveNegotiate,Saved}},
    system::reader::smb::runtime::QualifiedRustInstall};
use std::{io,num::NonZeroU64,sync::{Arc,RwLock,TryLockError},time::{Instant,SystemTime,UNIX_EPOCH}};

/// Only the actual checked installation and its original protected source
/// can provide the strictly scoped key and standalone server names.
pub(in crate::smb) struct ChallengeService {
    names:ServerNames,
    revision:RwLock<Option<Arc<ProtectedRevision>>>,
    #[cfg(test)]
    fixture_wall_time:Option<u64>,
}
struct ServerNames {
    machine:Box<[u8]>, nb_domain:Box<[u8]>, dns_machine:Option<Box<[u8]>>,
    major:u8, minor:u8, build:u16,
}
#[must_use]
pub(in crate::smb) struct ChallengeStep { gss:Vec<u8>, issued:Issued }

impl ChallengeService {
    pub(in crate::smb) fn from_qualified(start:&Arc<InheritedRun>,installation:&QualifiedRustInstall)->Result<Self>{
        if !installation.is_original_start(start){return Err(Rejected);}
        let names=ServerNames::from_install(installation.hostname(),linux_version()?)?;
        let original=record::load_original(start,installation)?;
        Ok(Self{names,revision:RwLock::new(Some(Arc::new(original))),#[cfg(test)] fixture_wall_time:None})
    }
    pub(super) fn snapshot(&self)->Result<Arc<ProtectedRevision>> {
        let r=self.revision.read().map_err(|_|Rejected)?.as_ref().cloned().ok_or(Rejected)?;
        if !r.source.live(){return Err(Rejected);}
        Ok(r)
    }
    pub(super) fn current(&self,rev:&Arc<ProtectedRevision>)->bool {
        self.while_current(rev,||()).is_some()
    }
    /// Same protected revision/startup without waiting in an active Core
    /// transaction. The caller's original registration uses registry ->
    /// verifier lock order. No callback or secret is exposed here.
    pub(super) fn require_held_revision(&self,rev:&Arc<ProtectedRevision>,start:&Arc<InheritedRun>)->io::Result<()> {
        let g=self.revision.try_read().map_err(|err|io::Error::from(match err {
            TryLockError::WouldBlock=>io::ErrorKind::WouldBlock,
            TryLockError::Poisoned(_)=>io::ErrorKind::PermissionDenied,
        }))?;
        if !g.as_ref().is_some_and(|v|v.epoch==rev.epoch&&Arc::ptr_eq(v,rev))||
            !rev.source.installed().is_some_and(|v|Arc::ptr_eq(&v,start)){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        start.require_live_source() // original parent pidfd, poll(0)
    }
    /// Only short in-memory publication uses the held read lock. A future
    /// credential writer must never take a transport/registry lock while
    /// holding the write lock; the shared order is registry -> revision.
    pub(super) fn while_current<R>(&self,rev:&Arc<ProtectedRevision>,f:impl FnOnce()->R)->Option<R> {
        if !rev.source.live(){return None;}
        let g=self.revision.read().ok()?;
        if !g.as_ref().is_some_and(|v|v.epoch==rev.epoch && Arc::ptr_eq(v,rev)) {return None;}
        let out=f();drop(g);Some(out)
    }
    fn wall_time(&self)->Result<u64> {
        #[cfg(test)]
        if let Some(t)=self.fixture_wall_time {return Ok(t);}
        local::filetime_delta(SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e|e.duration())).ok_or(Rejected)
    }
    /// No production branch accepts a nonce, timestamp, caller-supplied key
    /// or substitute provider. Failure of the actual kernel fill aborts.
    pub(in crate::smb) fn issue(&self,start:HaveNegotiate,sid:NonZeroU64,expires:Instant)->Result<ChallengeStep> {
        self.issue_from(start,sid,expires,||{
            let mut nonce=[0;8];
            if !local::random(&mut nonce) {return Err(Rejected);}
            Ok((nonce,self.wall_time()?))
        })
    }
    fn issue_from(&self,start:HaveNegotiate,sid:NonZeroU64,expires:Instant,
        source:impl FnOnce()->Result<([u8;8],u64)>)->Result<ChallengeStep> {
        if sid.get()==u64::MAX || Instant::now()>=expires {return Err(Rejected);}
        let (offered,layer)=start.challenge_offer().map_err(|_|Rejected)?;
        let flags=choose(offered)?;
        let revision=self.snapshot()?;
        let (nonce,time)=source()?;
        let type2=self.names.encode(flags,nonce,time)?;
        let gss=wrapped(layer,&type2)?;
        let expected_type2=Saved::from_wire(&type2).map_err(|_|Rejected)?;
        if Instant::now()>=expires || !self.current(&revision) {return Err(Rejected);}
        Ok(ChallengeStep{gss,issued:Issued{start,revision,assigned:sid,expected_type2,expires}})
    }
}
impl super::MechanismProof {
    pub(in crate::smb) fn current(&self,service:&ChallengeService)->bool {Instant::now()<self.expires && service.current(&self.revision)}
}
impl Pending {
    /// Sole non-test proof entry: consume the original challenge once, only
    /// while its own provider still names that exact revision. Never hold a
    /// provider lock across the mechanism verification.
    pub(in crate::smb) fn prove_current(self,service:&ChallengeService,p:&crate::smb::wire::message::Compound<'_>)
        ->Result<super::MechanismProof> {
        if !self.current_at(service,Instant::now()) {return Err(Rejected);}
        // One trusted wall-clock sample, not a client value, the saved Type 2
        // timestamp, or a replacement for the original Instant deadline.
        let server_time=service.wall_time()?;
        let until=self.expires;let proof=self.received(p,server_time)?;
        if Instant::now()>=until || !proof.current(service) {return Err(Rejected);}
        Ok(proof)
    }
    /// The original challenge becomes usable only inside this short, same-
    /// revision operation after the full network write. No I/O under the lock.
    pub(in crate::smb) fn install_while_current<R>(self,service:&ChallengeService,now:Instant,
        install:impl FnOnce(Self)->R)->Option<R> {
        let original=Arc::clone(&self.revision);
        service.while_current(&original,||{
            (now.max(Instant::now())<self.expires).then(||install(self))
        }).flatten()
    }
    pub(in crate::smb) fn current_at(&self,service:&ChallengeService,now:Instant)->bool {
        now<self.expires && service.current(&self.revision)
    }
}
impl ChallengeStep {
    pub(in crate::smb) fn token(&self)->&[u8] {&self.gss}
    /// The sender is only allowed to bind THIS exact standalone MORE,
    /// including our selected envelope (e.g. request-mic on the first reply).
    pub(in crate::smb) fn bind(self,actual:&[u8])->Result<Pending> {
        if actual.get(68..70)!=Some(&72u16.to_le_bytes()[..]) ||
            actual.get(70..72)!=Some(&(self.gss.len() as u16).to_le_bytes()[..]) ||
            actual.get(72..)!=Some(&self.gss[..]) {return Err(Rejected);}
        self.issued.sent(actual)
    }
}

// Strict local profile; never infer auth level from the client OS. MS-NLMP
// 2.2.2.5: ESS wins over an offered LM_KEY; unsupported flags never echo.
// ALWAYS_SIGN is required on Type 1 / 2. NTLM sign/seal are mechanism flags,
// not an SMB encryption capability.
fn choose(f:u32)->Result<u32> {
    let must=MIN|ALWAYS_SIGN;
    if f&must!=must || f&(FORBIDDEN & !0x80)!=0 {return Err(Rejected);}
    Ok((f&AGREED)|TARGET_INFO|0x0002_0000|4) // standalone TARGET_TYPE_SERVER | REQUEST_TARGET
}
fn name(v:&[u8],max:usize)->Result<()> {
    if v.is_empty() || v.len()>max || !v.len().is_multiple_of(2) {return Err(Rejected);}
    let units=v.as_chunks::<2>().0.iter().copied().map(u16::from_le_bytes);
    if !char::decode_utf16(units).all(|x|x.is_ok_and(|c|c!='\0')) {return Err(Rejected);}
    Ok(())
}
fn av(out:&mut Vec<u8>,id:u16,v:&[u8]) {
    out.extend_from_slice(&id.to_le_bytes());out.extend_from_slice(&(v.len() as u16).to_le_bytes());
    out.extend_from_slice(v);
}
fn descriptor(buf:&mut [u8],at:usize,n:usize,offset:usize) {
    buf[at..at+2].copy_from_slice(&(n as u16).to_le_bytes());
    buf[at+2..at+4].copy_from_slice(&(n as u16).to_le_bytes());
    buf[at+4..at+8].copy_from_slice(&(offset as u32).to_le_bytes());
}
pub(super) fn utf16(ascii:&[u8])->Result<Box<[u8]>>{
    if ascii.is_empty()||!ascii.is_ascii()||ascii.contains(&0){return Err(Rejected);}
    let mut out=Vec::new();out.try_reserve_exact(ascii.len().checked_mul(2).ok_or(Rejected)?).map_err(|_|Rejected)?;
    for &v in ascii{out.extend_from_slice(&[v,0]);}
    Ok(out.into_boxed_slice())
}
fn release_parts(release:&[u8])->Result<(u8,u8,u16)>{
    fn decimal(input:&[u8])->Result<u16>{
        if input.is_empty()||!input.iter().all(u8::is_ascii_digit){return Err(Rejected);}
        input.iter().try_fold(0u16,|n,v|
            n.checked_mul(10).and_then(|n|n.checked_add(u16::from(*v-b'0'))).ok_or(Rejected))
    }
    let mut parts=release.splitn(3,|&v|v==b'.');
    let major=u8::try_from(decimal(parts.next().ok_or(Rejected)?)?).map_err(|_|Rejected)?;
    let minor=u8::try_from(decimal(parts.next().ok_or(Rejected)?)?).map_err(|_|Rejected)?;
    let suffix=parts.next().ok_or(Rejected)?;
    let end=suffix.iter().take_while(|b|b.is_ascii_digit()).count();
    Ok((major,minor,decimal(&suffix[..end])?))
}
fn linux_version()->Result<(u8,u8,u16)>{
    // SAFETY: uname populates a complete C utsname; release is a bounded
    // byte field. No environment strings, external command or host lookup.
    let mut u:libc::utsname=unsafe{std::mem::zeroed()};
    if unsafe{libc::uname(&mut u)}!=0{return Err(Rejected);}
    let bytes=unsafe{std::slice::from_raw_parts(u.release.as_ptr().cast::<u8>(),u.release.len())};
    let release=std::ffi::CStr::from_bytes_until_nul(bytes).map_err(|_|Rejected)?;
    release_parts(release.to_bytes())
}
impl ServerNames {
    fn from_install(host:&str,version:(u8,u8,u16))->Result<Self>{
        let first=host.split('.').next().ok_or(Rejected)?;
        let machine=utf16(first.to_ascii_uppercase().as_bytes())?;
        let nb_domain=utf16(record::DOMAIN.as_bytes())?;
        let dns_machine=Some(utf16(host.as_bytes())?);
        let (major,minor,build)=version;
        Ok(Self{machine,nb_domain,dns_machine,major,minor,build})
    }
    fn encode(&self,flags:u32,nonce:[u8;8],time:u64)->Result<Vec<u8>> {
        // At most 15 UTF-16 code units for each local NetBIOS label and 255
        // for the configured DNS machine. Arithmetic below is bounded by this.
        name(&self.machine,30)?;name(&self.nb_domain,30)?;
        if let Some(d)=self.dns_machine.as_deref() {name(d,510)?;}
        let dns=self.dns_machine.as_deref().map_or(0,|d|4+d.len());
        let info=4+self.machine.len()+4+self.nb_domain.len()+dns+12+4;
        let offset=56+self.machine.len();let mut p=Vec::new();
        p.try_reserve_exact(offset+info).map_err(|_|Rejected)?;p.resize(56,0);
        p[..8].copy_from_slice(b"NTLMSSP\0");p[8..12].copy_from_slice(&2u32.to_le_bytes());
        descriptor(&mut p,12,self.machine.len(),56);
        p[20..24].copy_from_slice(&flags.to_le_bytes());p[24..32].copy_from_slice(&nonce);
        descriptor(&mut p,40,info,offset);p[48]=self.major;p[49]=self.minor;
        p[50..52].copy_from_slice(&self.build.to_le_bytes());p[55]=15;
        p.extend_from_slice(&self.machine);av(&mut p,1,&self.machine);av(&mut p,2,&self.nb_domain);
        if let Some(d)=self.dns_machine.as_deref() {av(&mut p,3,d);}
        av(&mut p,7,&time.to_le_bytes());av(&mut p,0,&[]);Ok(p)
    }
}

fn tlv_len(n:usize)->Result<usize> {
    if n>usize::from(u16::MAX) {return Err(Rejected);}
    Ok(n+if n<128 {2} else if n<=255 {3} else {4})
}
fn prefix(p:&mut Vec<u8>,tag:u8,n:usize) {
    p.push(tag);
    if n<128 {p.push(n as u8);} else if n<=255 {p.extend_from_slice(&[0x81,n as u8]);}
    else {p.push(0x82);p.extend_from_slice(&(n as u16).to_be_bytes());}
}
fn wrapped(layer:ChallengeLayer,type2:&[u8])->Result<Vec<u8>> {
    let fields:&[u8]=match layer {
        ChallengeLayer::Direct=>{let mut p=Vec::new();p.try_reserve_exact(type2.len()).map_err(|_|Rejected)?;
            p.extend_from_slice(type2);return Ok(p);},
        // First acceptor response: request-mic, supportedMech=NTLM.
        ChallengeLayer::FirstSpnego=>&[0xa0,3,10,1,3,0xa1,12,6,10,0x2b,6,1,4,1,0x82,0x37,2,2,10],
        // After the earlier mechanism-selection MORE, no repeated OID or MIC.
        ChallengeLayer::LaterSpnego=>&[0xa0,3,10,1,1],
    };
    let oct=tlv_len(type2.len())?;let fields_size=fields.len()+tlv_len(oct)?;
    let seq=tlv_len(fields_size)?;let total=tlv_len(seq)?;
    if total>usize::from(u16::MAX) {return Err(Rejected);}
    let mut out=Vec::new();out.try_reserve_exact(total).map_err(|_|Rejected)?;
    prefix(&mut out,0xa1,seq);prefix(&mut out,0x30,fields_size);out.extend_from_slice(fields);
    prefix(&mut out,0xa2,oct);prefix(&mut out,4,type2.len());out.extend_from_slice(type2);Ok(out)
}

#[cfg(test)]
#[path = "../../../test/smb/challenge.rs"]
mod tests;

//! Server-owned NTLMv2 Type 2 for the configured standalone service. No
//! general GSS acceptor, credential provisioning, live account or key export.
use super::{AGREED,ALWAYS_SIGN,FORBIDDEN,MIN,TARGET_INFO,Issued,Pending,ProtectedRevision,Rejected,Result};
use crate::smb::{local,session::envelope::{ChallengeLayer,HaveNegotiate,Saved}};
use std::{num::NonZeroU64,sync::{Arc,RwLock},time::{Instant,SystemTime,UNIX_EPOCH}};

/// Deliberately no production constructor. The future protected credential
/// owner must supply trusted standalone server names, actual OS version and
/// one coherent account revision; none is learned from the referring client.
pub(in crate::smb) struct ChallengeService {
    names:ServerNames,
    revision:RwLock<Option<Arc<ProtectedRevision>>>,
}
struct ServerNames {
    machine:Box<[u8]>, nb_domain:Box<[u8]>, dns_machine:Option<Box<[u8]>>,
    major:u8, minor:u8, build:u16,
}
#[must_use]
pub(in crate::smb) struct ChallengeStep { gss:Vec<u8>, issued:Issued }

impl ChallengeService {
    pub(super) fn snapshot(&self)->Result<Arc<ProtectedRevision>> {
        self.revision.read().map_err(|_|Rejected)?.as_ref().cloned().ok_or(Rejected)
    }
    pub(super) fn current(&self,rev:&Arc<ProtectedRevision>)->bool {
        self.while_current(rev,||()).is_some()
    }
    /// Only short in-memory publication uses the held read lock. A future
    /// credential writer must never take a transport/registry lock while
    /// holding the write lock; the shared order is registry -> revision.
    pub(super) fn while_current<R>(&self,rev:&Arc<ProtectedRevision>,f:impl FnOnce()->R)->Option<R> {
        let g=self.revision.read().ok()?;
        if !g.as_ref().is_some_and(|v|v.epoch==rev.epoch && Arc::ptr_eq(v,rev)) {return None;}
        let out=f();drop(g);Some(out)
    }
    /// No production branch accepts a nonce, timestamp, caller-supplied key
    /// or substitute provider. Failure of the actual kernel fill aborts.
    pub(in crate::smb) fn issue(&self,start:HaveNegotiate,sid:NonZeroU64,expires:Instant)->Result<ChallengeStep> {
        self.issue_from(start,sid,expires,||{
            let mut nonce=[0;8];
            if !local::random(&mut nonce) {return Err(Rejected);}
            let time=local::filetime_delta(SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e|e.duration()))
                .ok_or(Rejected)?;
            Ok((nonce,time))
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
        let until=self.expires;let proof=self.received(p)?;
        if Instant::now()>=until || !proof.current(service) {return Err(Rejected);}
        Ok(proof)
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
    if v.is_empty() || v.len()>max || v.len()%2!=0 {return Err(Rejected);}
    let units=v.chunks_exact(2).map(|u|u16::from_le_bytes([u[0],u[1]]));
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
impl ServerNames {
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

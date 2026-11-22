//! Complete a strictly authenticated exchange, never a caller-supplied
//! username, flag, raw key, success template or replacement final GSS token.
use super::{FinalGss,MechanismProof,ProtectedRevision,Rejected,Result,challenge::ChallengeService};
use crate::smb::session::{FinalKeys,SessionSigner,VerifiedExchange};
use std::{io,num::NonZeroU64,sync::{Arc,Weak}};

/// Still NOT a live entry or file authority. The original connection must own
/// this candidate, keep its real Claim and publish only on confirmed send.
#[must_use]
pub(in crate::smb) struct SealedFinal { bytes:Vec<u8>, bound:BoundSession }
pub(in crate::smb) struct BoundSession { revision:Weak<ProtectedRevision>, keys:FinalKeys }

impl MechanismProof {
    pub(in crate::smb) fn seal(self,service:&ChallengeService,credits:u16)->Result<SealedFinal> {
        if !self.current(service) {return Err(Rejected);}
        let Self{request,exported,revision,final_gss,expires}=self;
        let token:&[u8]=match &final_gss {FinalGss::Direct=>&[],FinalGss::Spnego(v)=>v};
        // The only header source is the same immutable Type 3 already
        // included in this verifier's five-/seven-message transcript.
        let mut bytes=request.final_template(credits,token).map_err(|_|Rejected)?;
        let keys=VerifiedExchange{request,exported}.complete(&mut bytes).map_err(|_|Rejected)?;
        if std::time::Instant::now()>=expires || !service.current(&revision) {return Err(Rejected);}
        Ok(SealedFinal{bytes,bound:BoundSession{revision:Arc::downgrade(&revision),keys}})
    }
}
impl BoundSession {
    pub(in crate::smb) fn current(&self,service:&ChallengeService)->bool {
        self.revision.upgrade().is_some_and(|r|service.current(&r))
    }
    pub(in crate::smb) fn on_revision(&self,stamp:&BatchRevision)->bool {Weak::ptr_eq(&self.revision,&stamp.0)}
    pub(in crate::smb::session) fn signer_for(&self,stamp:&BatchRevision)->Option<&SessionSigner> {
        self.on_revision(stamp).then_some(&self.keys.signer)
    }
}
/// Snapshot identity only. Never keeps the verifier secret alive across a
/// request or grants a session. Key use requires a matching published state.
pub(in crate::smb) struct BatchRevision(Weak<ProtectedRevision>);
impl ChallengeService {
    pub(in crate::smb) fn batch_revision(&self)->Option<BatchRevision> {
        Some(BatchRevision(Arc::downgrade(&self.snapshot().ok()?)))
    }
}
impl BatchRevision {
    pub(in crate::smb) fn current(&self,s:&ChallengeService)->bool {
        self.0.upgrade().is_some_and(|r|s.current(&r))
    }
    pub(in crate::smb) fn original_start(&self,s:&ChallengeService)->Option<Arc<crate::smb::InheritedRun>>{
        let r=self.0.upgrade()?;
        if !s.current(&r){return None;}
        r.source.installed()
    }
}
/// Weak currency marker in the run-wide index. It conveys no key, subject,
/// storage right or way to make a session. The connection owns the keys.
pub(in crate::smb) struct LiveMark(Weak<ProtectedRevision>);
impl LiveMark {
    pub(in crate::smb) fn current(&self,s:&ChallengeService)->bool {
        self.0.upgrade().is_some_and(|r|s.current(&r))
    }
    pub(in crate::smb) fn original_start(&self,s:&ChallengeService)->Option<Arc<crate::smb::InheritedRun>>{
        let r=self.0.upgrade()?;
        if !s.current(&r){return None;}
        r.source.installed()
    }
    pub(in crate::smb) fn require_held_revision(&self,s:&ChallengeService,start:&Arc<crate::smb::InheritedRun>)
        ->io::Result<()> {
        let r=self.0.upgrade().ok_or(io::ErrorKind::PermissionDenied)?;
        s.require_held_revision(&r,start)
    }
}
impl SealedFinal {
    /// The caller binds these owned, already-signed bytes to the original
    /// reply ledger. Failed binding drops the same sealed state and all keys.
    pub(in crate::smb) fn bind_to<T,E>(self,bind:impl FnOnce(Vec<u8>)->std::result::Result<T,E>)
        ->std::result::Result<(T,BoundSession),E> {Ok((bind(self.bytes)?,self.bound))}
}
impl BoundSession {
    /// A closed command-specific codec, not a signing oracle over arbitrary
    /// buffers. The connection separately verifies the current original Claim.
    pub(in crate::smb) fn sign_small_reply(&self,stamp:&BatchRevision,reply:crate::smb::wire::fixed_reply::PayableSmall)
        ->std::result::Result<crate::smb::wire::fixed_reply::SignedSmall,crate::smb::session::Fault> {
        let tag=self.reply_tag(stamp,reply.sid(),reply.for_mac())?;
        Ok(reply.signed(tag))
    }
    pub(in crate::smb) fn sign_directory(&self,stamp:&BatchRevision,reply:crate::smb::wire::query_reply::PayableDirectory)
        ->std::result::Result<crate::smb::wire::query_reply::SignedDirectory,crate::smb::session::Fault>{
        let tag=self.reply_tag(stamp,reply.sid(),reply.for_mac())?;
        Ok(reply.signed(tag))
    }
    pub(in crate::smb) fn sign_file_read(&self,stamp:&BatchRevision,reply:crate::smb::wire::read_reply::PayableRead)
        ->std::result::Result<crate::smb::wire::read_reply::SignedRead,crate::smb::session::Fault>{
        let tag=self.reply_tag(stamp,reply.sid(),reply.for_mac())?;
        Ok(reply.signed(tag))
    }
    pub(in crate::smb) fn sign_file_name(&self,stamp:&BatchRevision,reply:crate::smb::wire::file_name_reply::PayableFileName)
        ->std::result::Result<crate::smb::wire::file_name_reply::SignedFileName,crate::smb::session::Fault>{
        let tag=self.reply_tag(stamp,reply.sid(),reply.for_mac())?;
        Ok(reply.signed(tag))
    }
    fn reply_tag(&self,stamp:&BatchRevision,id:NonZeroU64,exact:&[u8])
        ->std::result::Result<[u8;16],crate::smb::session::Fault>{
        use crate::smb::fixed_output::Mac;
        let signer=self.signer_for(stamp).filter(|s|s.id==id).ok_or(crate::smb::session::Fault::UnknownSession)?;
        let mut mac=signer.mac()?;mac.update(exact);let result=mac.finalize();
        let mut public_tag=[0;16];public_tag.copy_from_slice(result.as_bytes());
        Ok(public_tag)
    }
    pub(in crate::smb) fn same_id(&self,id:NonZeroU64)->bool {self.keys.signer.id==id}
    pub(in crate::smb) fn install_while_current<R>(self,s:&ChallengeService,install:impl FnOnce(Self,LiveMark)->R)
        ->Option<R> {
        let rev=self.revision.upgrade()?;
        s.while_current(&rev,||install(self,LiveMark(Arc::downgrade(&rev))))
    }
}

#[cfg(test)]
#[path = "../../../../tests/smb/final_keys.rs"]
mod tests;

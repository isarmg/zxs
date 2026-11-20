//! Complete a strictly authenticated exchange, never a caller-supplied
//! username, flag, raw key, success template or replacement final GSS token.
use super::{FinalGss,MechanismProof,ProtectedRevision,Rejected,Result,challenge::ChallengeService};
use crate::smb::session::{FinalKeys,VerifiedExchange};
use std::sync::Arc;

/// Still NOT a live entry or file authority. The original connection must own
/// this candidate, keep its real Claim and publish only on confirmed send.
#[must_use]
pub(in crate::smb) struct SealedFinal { bytes:Vec<u8>, bound:BoundSession }
pub(in crate::smb) struct BoundSession { revision:Arc<ProtectedRevision>, keys:FinalKeys }

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
        Ok(SealedFinal{bytes,bound:BoundSession{revision,keys}})
    }
}
impl BoundSession {
    pub(in crate::smb) fn current(&self,service:&ChallengeService)->bool {service.current(&self.revision)}
}
// There is deliberately no production packet/key getter at this stage.

#[cfg(test)]
#[path = "../../../test/smb/final_keys.rs"]
mod tests;

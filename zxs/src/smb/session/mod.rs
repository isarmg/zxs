//! Plaintext SMB 3.1.1, fresh authenticated NTLM session, AES-128-CMAC only.
//! The original connection owns publication and request admission. There is no
//! enabled production entry or listener. A signature alone grants no file access.
mod block_cipher;
mod der;
pub(in crate::smb) mod envelope;
mod keyed_digest;
mod ntlm_shape;
mod packet_mac;
mod strict_verifier;
pub(in crate::smb) use strict_verifier::{MechanismProof,Pending,Rejected as ChallengeRejected,VaultDecoder,challenge::{ChallengeService,ChallengeStep},completion::{BatchRevision,BoundSession,LiveMark}};

#[cfg(test)]
pub(in crate::smb) use self::keyed_digest::Hmac as HmacForSessionFixture;

#[cfg(test)]
pub(in crate::smb) type SigningMacForSessionFixture = SigningMac;

use crate::smb::wire::{
    message::{CANCEL, Compound, MAX_COMMANDS, Message, RELATED, SIGNED},
    preauth::RequestKeyContext,
};
use self::block_cipher::Aes128;
use crate::smb::handshake_hash::Sha256;
use digest::{FixedOutput, KeyInit, Mac, Output};
use self::keyed_digest::Hmac;
use std::num::NonZeroU64;
use zeroize::{Zeroize, Zeroizing};

type KdfMac = Hmac<Sha256>;
type SigningMac = packet_mac::Cmac<Aes128>;
type Secret = Zeroizing<[u8; 16]>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum Fault {
    WrongPhase,
    UnknownSession,
    SignatureRequired,
    BadSignature,
    UnusableKey,
    InvalidFinalResponse,
}

/// Only the strictly authenticating mechanism inside this module may produce
/// this from its complete proof, original request and that same exchange's
/// exported NTLM session key after rechecking its protected revision.
/// Never construct this from a username, a bool, a wire key or two independent
/// lookup results. No Debug, Clone or key-byte getter.
#[must_use]
struct VerifiedExchange {
    request: RequestKeyContext,
    exported: Secret,
}

impl VerifiedExchange {
    fn signing_key(&self) -> Result<Secret, Fault> {self.derive(b"SMBSigningKey\0")}
    fn application_key(&self)->Result<Secret,Fault> {self.derive(b"SMBAppKey\0")}
    fn derive(&self,label:&'static [u8]) -> Result<Secret, Fault> {
        // MS-SMB2 §§3.1.4.2, 3.3.5.5.3: SP800-108 counter mode with
        // HMAC-SHA-256, one block, 128 output bits. The LABEL includes its
        // terminator; the KDF's own zero separator is a *different* byte.
        let mut prf = KdfMac::new_from_slice(&self.exported[..]).map_err(|_| Fault::UnusableKey)?;
        prf.update(&1u32.to_be_bytes());
        prf.update(label);
        prf.update(&[0]);
        prf.update(self.request.as_bytes());
        prf.update(&128u32.to_be_bytes());
        let mut full = Output::<KdfMac>::default();
        FixedOutput::finalize_into(prf, &mut full);
        let mut key = Zeroizing::new([0; 16]);
        key.copy_from_slice(&full[..16]);
        full.as_mut_slice().zeroize();
        Ok(key)
    }

    /// Complete only with our actual, fully serialized, standalone successful
    /// response to the exact final request. It is signed, NOT included in the
    /// KDF context. A failure changes no response bytes and yields no signer.
    fn complete(self, reply: &mut [u8]) -> Result<FinalKeys, Fault> {
        let sid = self.request.check_final_reply(reply).map_err(|_| Fault::InvalidFinalResponse)?;
        let id = NonZeroU64::new(sid).ok_or(Fault::InvalidFinalResponse)?;
        let signer = SessionSigner { id, key: self.signing_key()? };
        let application=self.application_key()?;
        let mut mac = signer.mac()?;
        // check_final_reply proved the original priority plus SERVER_TO_REDIR.
        // Substitute only its signed flag and Signature; all other bytes,
        // including the actual GSS output and padding, are exactly as sent.
        let flags = (u32::from(reply[16]) | SIGNED).to_le_bytes();
        mac.update(&reply[..16]);
        mac.update(&flags);
        mac.update(&reply[20..48]);
        mac.update(&[0; 16]);
        mac.update(&reply[64..]);
        let tag = mac.finalize();
        reply[16..20].copy_from_slice(&flags);
        reply[48..64].copy_from_slice(tag.as_bytes());
        Ok(FinalKeys {signer,application,session:self.exported,context:self.request})
    }
}

/// One original single-channel session: Session.SigningKey and the channel's
/// SigningKey refer to the same signer; the application/session keys are kept
/// private, never used as proofs of identity or exposed to file operations.
struct FinalKeys { signer:SessionSigner, application:Secret, session:Secret, context:RequestKeyContext }


/// Only for a live, valid, current-epoch session on its original connection.
/// The connection/epoch checks and insertion/removal belong to that owner's
/// table, not to this derived key. Signing is not proof of a new identity.
#[must_use]
struct SessionSigner {
    id: NonZeroU64,
    key: Secret,
}

impl SessionSigner {
    fn mac(&self) -> Result<SigningMac, Fault> {
        SigningMac::new_from_slice(&self.key[..]).map_err(|_| Fault::UnusableKey)
    }
}

/// Effective session identifiers are only lookup coordinates for the original
/// already-checked frame, not standalone authority. Never rewrite wire fields.
pub(in crate::smb) struct VerifiedIds([Option<NonZeroU64>; MAX_COMMANDS]);
impl VerifiedIds {
    pub(in crate::smb) fn at(&self,ordinal:usize)->Option<NonZeroU64> {
        self.0.get(ordinal).copied().flatten()
    }
}
/// The production entry accepts only sealed strict-session state from the
/// original connection owner, for the single protected revision this batch
/// started on. No caller outside this module sees a signer or a key.
pub(in crate::smb) fn verify_published<'s>(
    frame:&Compound<'_>,revision:&BatchRevision,mut lookup:impl FnMut(NonZeroU64)->Option<&'s BoundSession>,
)->Result<VerifiedIds,(usize,Fault)> {
    verify_ids(frame,|sid|lookup(sid)?.signer_for(revision))
}
/// CANCEL has no sequence debit or related SID. This is the same raw CMAC
/// check, not admission as an executable ordinary member or a cancel target.
pub(in crate::smb) fn verify_cancel<'s>(
    frame:&Compound<'_>,revision:&BatchRevision,mut lookup:impl FnMut(NonZeroU64)->Option<&'s BoundSession>,
)->Result<NonZeroU64,Fault> {
    let [m]=frame.messages() else {return Err(Fault::WrongPhase);};
    if m.header.command!=CANCEL {return Err(Fault::WrongPhase);}
    verify_signed(m,None,&mut |sid|lookup(sid)?.signer_for(revision))
}
fn verify_ids<'s>(frame:&Compound<'_>,mut lookup:impl FnMut(NonZeroU64)->Option<&'s SessionSigner>)
    ->Result<VerifiedIds,(usize,Fault)> {
    let mut out=VerifiedIds([None;MAX_COMMANDS]);let mut previous=None;
    for (i,m) in frame.messages().iter().enumerate() {
        let id=verify_member(m,previous,&mut lookup).map_err(|why|(i,why))?;
        out.0[i]=Some(id);previous=Some(id);
    }
    Ok(out) // a valid prefix never escapes on a later failure
}
fn verify_member<'s>(m:&Message<'_>,previous:Option<NonZeroU64>,
    lookup:&mut impl FnMut(NonZeroU64)->Option<&'s SessionSigner>)->Result<NonZeroU64,Fault> {
    if !matches!(m.header.command,0x0002..=0x000b | 0x000d..=0x0012) {return Err(Fault::WrongPhase);}
    verify_signed(m,previous,lookup)
}
fn verify_signed<'s>(m:&Message<'_>,previous:Option<NonZeroU64>,
    lookup:&mut impl FnMut(NonZeroU64)->Option<&'s SessionSigner>)->Result<NonZeroU64,Fault> {
    if m.header.flags&SIGNED==0 {return Err(Fault::SignatureRequired);}
    let id=if m.header.flags&RELATED!=0 {previous} else {
        NonZeroU64::new(m.header.session).filter(|v|v.get()!=u64::MAX)
    }.ok_or(Fault::UnknownSession)?;
    let signer=lookup(id).filter(|s|s.id==id).ok_or(Fault::UnknownSession)?;
    let mut mac=signer.mac()?;
    mac.update(&m.raw[..48]);mac.update(&[0;16]);mac.update(&m.raw[64..]);
    mac.verify_slice(&m.raw[48..64]).map_err(|_|Fault::BadSignature)?;
    Ok(id)
}

#[cfg(test)]
#[path = "../../test/smb/signing.rs"]
mod tests;

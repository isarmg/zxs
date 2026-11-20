//! Plaintext SMB 3.1.1, fresh authenticated NTLM session, AES-128-CMAC only.
//! No live authentication issuer, constructor from a wire key, credential lookup, connection
//! table or service entry point. Verification alone confers no storage rights.
mod der;
pub(in crate::smb) mod envelope;
mod ntlm_shape;
mod strict_verifier;
pub(in crate::smb) use strict_verifier::{MechanismProof,Pending,Rejected as ChallengeRejected,challenge::{ChallengeService,ChallengeStep}};

use crate::smb::wire::{
    message::{Compound, RELATED, SIGNED},
    preauth::RequestKeyContext,
};
use aes::Aes128;
use cmac::Cmac;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Sha256, digest::{FixedOutput, Output}};
use std::num::NonZeroU64;
use zeroize::{Zeroize, Zeroizing};

type KdfMac = Hmac<Sha256>;
type SigningMac = Cmac<Aes128>;
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
pub(in crate::smb) struct SessionSigner {
    id: NonZeroU64,
    key: Secret,
}

impl SessionSigner {
    fn mac(&self) -> Result<SigningMac, Fault> {
        SigningMac::new_from_slice(&self.key[..]).map_err(|_| Fault::UnusableKey)
    }
}

/// All-or-error signature preflight of ordinary authenticated requests on the
/// plaintext direct transport. Never dispatch during this pass. The lookup
/// borrows only already verified, non-revoked sessions of this connection.
/// Credits, body semantics, effective tree/open, cancel/handshake, encrypted
/// transforms and pending related-operation results are SEPARATE checks.
pub(in crate::smb) fn verify_ordinary<'s>(
    compound: &Compound<'_>, mut lookup: impl FnMut(NonZeroU64) -> Option<&'s SessionSigner>,
) -> Result<(), Fault> {
    let mut previous = None;
    for m in compound.messages() {
        // NEGOTIATE/SESSION_SETUP and CANCEL need different state/handling;
        // don't let a specially framed command pass the ordinary check.
        if !matches!(m.header.command, 0x0002..=0x000b | 0x000d..=0x0012) {
            return Err(Fault::WrongPhase);
        }
        let flags = m.header.flags;
        if flags & SIGNED == 0 { return Err(Fault::SignatureRequired); }
        let id = if flags & RELATED != 0 {
            previous
        } else {
            NonZeroU64::new(m.header.session).filter(|id| id.get() != u64::MAX)
        }.ok_or(Fault::UnknownSession)?;
        let signer = lookup(id).filter(|s| s.id == id).ok_or(Fault::UnknownSession)?;
        let mut mac = signer.mac()?;
        // Compound::parse proved a full header. Use each immutable segment
        // directly: effective/inherited IDs are NOT rewritten into signed bytes.
        mac.update(&m.raw[..48]);
        mac.update(&[0; 16]);
        mac.update(&m.raw[64..]); // includes every terminal/inter-member pad byte
        mac.verify_slice(&m.raw[48..64]).map_err(|_| Fault::BadSignature)?;
        previous = Some(id);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../test/smb/signing.rs"]
mod tests;

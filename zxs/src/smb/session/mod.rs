//! Plaintext SMB 3.1.1, fresh authenticated NTLM session, AES-128-CMAC only.
//! No authenticator, constructor from a wire key, credential lookup, connection
//! table or service entry point. Verification alone confers no storage rights.
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

/// The type deliberately has **no production issuer**. Only the future
/// authenticating mechanism inside this module may produce it, atomically
/// finishing its original attempt (subject, epoch, same connection and exact
/// final request) and releasing that attempt's exported NTLM session key.
/// Never construct this from a username, a bool, a wire key or two independent
/// lookup results. No Debug, Clone or key-byte getter.
#[must_use]
pub(in crate::smb) struct VerifiedExchange {
    request: RequestKeyContext,
    exported: Secret,
}

impl VerifiedExchange {
    fn signing_key(&self) -> Result<Secret, Fault> {
        // MS-SMB2 §§3.1.4.2, 3.3.5.5.3: SP800-108 counter mode with
        // HMAC-SHA-256, one block, 128 output bits. The LABEL includes its
        // terminator; the KDF's own zero separator is a *different* byte.
        let mut prf = KdfMac::new_from_slice(&self.exported[..]).map_err(|_| Fault::UnusableKey)?;
        prf.update(&1u32.to_be_bytes());
        prf.update(b"SMBSigningKey\0");
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
    pub(in crate::smb) fn sign_final(self, reply: &mut [u8]) -> Result<SessionSigner, Fault> {
        let sid = self.request.check_final_reply(reply).map_err(|_| Fault::InvalidFinalResponse)?;
        let id = NonZeroU64::new(sid).ok_or(Fault::InvalidFinalResponse)?;
        let signer = SessionSigner { id, key: self.signing_key()? };
        let mut mac = signer.mac()?;
        // check_final_reply proves a whole unsigned server header with Flags=1.
        // Substitute only its signed flag and Signature; all other bytes,
        // including the actual GSS output and padding, are exactly as sent.
        let flags = (1 | SIGNED).to_le_bytes();
        mac.update(&reply[..16]);
        mac.update(&flags);
        mac.update(&reply[20..48]);
        mac.update(&[0; 16]);
        mac.update(&reply[64..]);
        let tag = mac.finalize();
        reply[16..20].copy_from_slice(&flags);
        reply[48..64].copy_from_slice(tag.as_bytes());
        Ok(signer)
    }
}

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

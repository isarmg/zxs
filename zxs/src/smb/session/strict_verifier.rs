//! Strict development profile for one connection-oriented NTLMv2 exchange.
//! No enrollment, credential reader, production provider, live-session authority,
//! wire-key constructor, listener or conversion into a storage/session grant.
pub(super) mod challenge;
pub(super) mod completion;
use super::{
    Secret, envelope::{AwaitAuthenticate, HaveNegotiate, Layer, Saved, Unverified},
    ntlm_shape::{self as ntlm, Authenticate, Challenge},
};
use crate::smb::wire::{message::Compound, preauth::RequestKeyContext};
use hmac::{Hmac, KeyInit, Mac};
use md5::{Digest, Md5, digest::{FixedOutput, Output}};
use rc4::{Rc4, StreamCipher};
use std::{num::NonZeroU64, sync::Arc, time::Instant};
use zeroize::{Zeroize, Zeroizing};

type H = Hmac<Md5>;
pub(in crate::smb) type Result<T> = std::result::Result<T, Rejected>;
#[derive(Debug)]
pub(in crate::smb) struct Rejected;

// Only this partial, versioned profile is accepted. NTLM version itself is
// configured, not negotiated. Do not infer relaxed rules from UA or VERSION.
const UNICODE: u32 = 1;
const SIGN: u32 = 0x10;
const SEAL: u32 = 0x20;
const NTLM: u32 = 0x200;
const ALWAYS_SIGN: u32 = 0x8000;
const ESS: u32 = 0x0008_0000;
const TARGET_INFO: u32 = 0x0080_0000;
const VERSION: u32 = 0x0200_0000;
const BITS128: u32 = 0x2000_0000;
const KEY_EXCH: u32 = 0x4000_0000;
const BITS56: u32 = 0x8000_0000;
const MIN: u32 = UNICODE | SIGN | NTLM | ESS | VERSION | BITS128;
const AGREED: u32 = MIN | SEAL | ALWAYS_SIGN | KEY_EXCH | BITS56;
const FORBIDDEN: u32 = 0x40 | 0x80 | 0x800 | 0x0010_0000 | 0x0040_0000;
    // DATAGRAM, LM_KEY, ANONYMOUS, IDENTIFY, REQUEST_NON_NT_SESSION_KEY

/// A prederived, password-equivalent NTLMv2 verifier for exactly one
/// principal/domain and revision; not a password, Web hash, or identity proof.
/// No production constructor until the actual credential owner is integrated.
struct ProtectedRevision {
    user: Box<[u8]>, domain: Box<[u8]>, spn: Box<[u8]>, // trusted exact UTF-16LE
    epoch: NonZeroU64,
    response_key: Secret,
}

/// Sealed mechanism state. The actual connection reserves the SID and lifetime;
/// challenge::ChallengeService pins its own revision and original Type2. No
/// interface for application-supplied challenges, verifiers or credentials.
#[must_use]
pub(super) struct Issued {
    start: HaveNegotiate,
    revision: Arc<ProtectedRevision>,
    assigned: NonZeroU64,
    expected_type2: Saved,
    expires: Instant,
}

#[must_use]
pub(in crate::smb) struct Pending {
    phase: AwaitAuthenticate,
    revision: Arc<ProtectedRevision>,
    expires: Instant,
}

impl Issued {
    pub(in crate::smb) fn sent(self, serialized: &[u8]) -> Result<Pending> {
        if Instant::now() >= self.expires || self.assigned.get() == u64::MAX ||
            serialized.get(40..48) != Some(&self.assigned.get().to_le_bytes()[..]) {
            return Err(Rejected);
        }
        // This also verifies our previous request's MID, the actual MORE
        // status, selected wrapper/roles and the original SMB security field.
        let phase = self.start.challenged(serialized).map_err(|_| Rejected)?;
        if phase.actual_challenge() != self.expected_type2.bytes() { return Err(Rejected); }
        Ok(Pending { phase, revision: self.revision, expires: self.expires })
    }
}

impl Pending {
    /// A single terminal consumption, including on malformed input or expiry.
    /// Future runtime must remove this instance from its original connection
    /// (and check MessageId/credits) before this call. No retries with it.
    fn received(self, p: &Compound<'_>, server_time: u64) -> Result<MechanismProof> {
        if Instant::now() >= self.expires { return Err(Rejected); }
        let u = self.phase.received(p).map_err(|_| Rejected)?;
        let proof = verify(self.revision, u, self.expires, server_time)?;
        if Instant::now() >= self.expires { return Err(Rejected); }
        Ok(proof)
    }
}

/// This is neither a production identity nor an SMB session. A future trusted
/// owner must atomically recheck this very revision and its current connection,
/// serialize the exact final GSS token, sign THAT successful response with
/// this same request/key, and enforce signing before any file operation.
/// No external field access, key getter, Clone, or VerifiedExchange issuer.
#[must_use]
pub(in crate::smb) struct MechanismProof {
    revision: Arc<ProtectedRevision>,
    request: RequestKeyContext,
    exported: Secret,
    final_gss: FinalGss,
    expires: Instant, // original pending challenge; not a live-session expiry
}

enum FinalGss { Direct, Spnego([u8; 29]) }

fn hmac(key: &[u8], parts: &[&[u8]]) -> Result<H> {
    let mut m = H::new_from_slice(key).map_err(|_| Rejected)?;
    for &p in parts { Mac::update(&mut m,p); }
    Ok(m)
}

fn finish_hmac(m: H) -> Secret {
    let mut full = Output::<H>::default();
    FixedOutput::finalize_into(m, &mut full);
    let mut key = Zeroizing::new([0; 16]);
    key.copy_from_slice(&full);
    full.as_mut_slice().zeroize();
    key
}

fn rc4_apply(key: &[u8], data: &mut [u8]) -> Result<()> {
    let mut c = Rc4::new_from_slice(key).map_err(|_| Rejected)?;
    c.try_apply_keystream(data).map_err(|_| Rejected)
}

fn selected(offered: u32, s: u32, claimed: u32) -> Result<()> {
    if offered & MIN != MIN || s & (MIN | TARGET_INFO) != (MIN | TARGET_INFO) ||
       s & FORBIDDEN != 0 || claimed & FORBIDDEN != 0 ||
       (s & AGREED) & !offered != 0 || (s ^ claimed) & (AGREED | TARGET_INFO) != 0 {
        return Err(Rejected);
    }
    Ok(())
}

fn bind_targets(v: &ProtectedRevision, c: &Challenge<'_>, a: &Authenticate<'_>) -> Result<()> {
    if v.user.is_empty() || v.spn.is_empty() || a.user_utf16 != &*v.user || a.domain_utf16 != &*v.domain {
        return Err(Rejected);
    }
    // Keep all server-owned AVs (including the required Type 2 AV7) bound
    // to this exact original challenge. AV7 is not the v2 blob header time.
    if [1,2,3,4,5,7].into_iter().any(|id| c.avs.get(id) != a.avs.get(id)) {
        return Err(Rejected);
    }
    // Narrow single-service, non-TLS profile: a configured trusted CIFS SPN,
    // no untrusted-target bit, no invented channel binding. An authenticated
    // name is still not a storage path or an authorization claim.
    if c.avs.flags() & !3 != 0 || c.avs.get(9).is_some_and(|spn| spn != &*v.spn) ||
        c.avs.get(10).is_some_and(|cb| cb != [0;16]) { return Err(Rejected); }
    let flags = a.avs.flags();
    if !a.mic_indicated || flags & !3 != 0 || (flags ^ c.avs.flags()) & 1 != 0 ||
        a.avs.get(9) != Some(&v.spn[..]) ||
        a.avs.get(10).is_some_and(|v| v != [0;16]) ||
        (!a.lm.is_empty() && a.lm != [0;24]) {
        return Err(Rejected);
    }
    Ok(())
}

fn direction_key(exported: &Secret, magic: &[u8]) -> Secret {
    let mut md = Md5::new();
    Digest::update(&mut md, &exported[..]);
    Digest::update(&mut md, magic); // includes the NUL specified by MS-NLMP
    let mut out = Output::<Md5>::default();
    FixedOutput::finalize_into(md, &mut out);
    let mut key = Zeroizing::new([0;16]);
    key.copy_from_slice(&out);
    out.as_mut_slice().zeroize();
    key
}

const CLIENT_SIGN: &[u8] = b"session key to client-to-server signing key magic constant\0";
const SERVER_SIGN: &[u8] = b"session key to server-to-client signing key magic constant\0";
const CLIENT_SEAL: &[u8] = b"session key to client-to-server sealing key magic constant\0";
const SERVER_SEAL: &[u8] = b"session key to server-to-client sealing key magic constant\0";

fn list_exchange(exported: &Secret, key_exch: bool, list: &[u8], inbound: &[u8]) -> Result<FinalGss> {
    // RFC 4178 §5(c); MS-NLMP §§3.4.4.2, 3.4.5.{2,3}. First per-direction
    // GSS MIC has sequence zero. MS-SPNG §3.2.5.1 uses the initial RC4 state
    // for each list operation, without advancing application sealing state.
    if inbound.len() != 16 || inbound[..4] != 1u32.to_le_bytes() || inbound[12..] != [0;4] {
        return Err(Rejected);
    }
    let mut tag = [0;8];
    tag.copy_from_slice(&inbound[4..12]);
    if key_exch {
        let seal = direction_key(exported, CLIENT_SEAL);
        rc4_apply(&seal[..], &mut tag)?;
    }
    let sign = direction_key(exported, CLIENT_SIGN);
    hmac(&sign[..], &[&[0;4], list])?.verify_truncated_left(&tag).map_err(|_| Rejected)?;

    let sign = direction_key(exported, SERVER_SIGN);
    let mac = hmac(&sign[..], &[&[0;4], list])?.finalize();
    tag.copy_from_slice(&mac.as_bytes()[..8]);
    if key_exch {
        let seal = direction_key(exported, SERVER_SEAL);
        rc4_apply(&seal[..], &mut tag)?;
    }
    // NegTokenResp(accept-completed, mechListMIC = NTLM GSS signature).
    let mut der = [0;29];
    der[..13].copy_from_slice(&[0xa1,27,0x30,25,0xa0,3,10,1,0,0xa3,18,4,16]);
    der[13..17].copy_from_slice(&1u32.to_le_bytes());
    der[17..25].copy_from_slice(&tag);
    Ok(FinalGss::Spnego(der))
}

// [MS-NLMP] 3.2.5.2.2 bounds the client challenge header TimeStamp relative
// to server time. 120s in either direction is local policy; this wall-clock
// bound does not extend the original monotonic one-shot challenge deadline.
const MAX_CLOCK_SKEW_TICKS: u64 = 120 * 10_000_000;
fn fresh_header_time(a:&Authenticate<'_>,server_time:u64)->Result<()> {
    // NTProofStr (16 B), then the version/reserved prefix (8 B), then FILETIME.
    let t=a.nt_response.get(24..32).and_then(|v|v.try_into().ok()).map(u64::from_le_bytes).ok_or(Rejected)?;
    if server_time.abs_diff(t)>MAX_CLOCK_SKEW_TICKS {return Err(Rejected);}
    Ok(())
}

fn verify(v: Arc<ProtectedRevision>, u: Unverified<'_>, expires:Instant,server_time:u64) -> Result<MechanismProof> {
    let original1 = u.type1.bytes();
    let original2 = u.type2.bytes();
    let c = ntlm::challenge_fields(original2).map_err(|_| Rejected)?;
    let a = &u.type3;
    selected(ntlm::negotiate_flags(original1).map_err(|_| Rejected)?, c.flags, a.flags)?;
    bind_targets(&v, &c, a)?;
    fresh_header_time(a,server_time)?;
    let (proof, client_blob) = a.nt_response.split_at_checked(16).ok_or(Rejected)?;
    // Only NTLMv2, never LM as a fallback. The responding client's raw blob
    // includes the actual terminator/trailing reserved bytes that it sent.
    hmac(&v.response_key[..], &[c.nonce,client_blob])?.verify_slice(proof).map_err(|_| Rejected)?;
    let base = finish_hmac(hmac(&v.response_key[..], &[proof])?);
    let key_exch = c.flags & KEY_EXCH != 0;
    let exported = if key_exch {
        if a.encrypted_key.len() != 16 { return Err(Rejected); }
        let mut key = Zeroizing::new([0;16]);
        key.copy_from_slice(a.encrypted_key);
        rc4_apply(&base[..], &mut key[..])?;
        key
    } else { base };

    // VERSION is a negotiated requirement in ALL three messages in this
    // strict profile; its Type3 slot is 64..72 and the mandatory MIC 72..88.
    // Do not infer location from a client-controlled gap or zero other fields.
    let mic = a.raw.get(72..88).ok_or(Rejected)?;
    hmac(&exported[..], &[original1, original2, &a.raw[..72], &[0;16], &a.raw[88..]])?
        .verify_slice(mic).map_err(|_| Rejected)?;
    let final_gss = match &u.layer {
        Layer::Direct if u.list_mic.is_none() => FinalGss::Direct,
        Layer::Spnego { list, require_list_mic: true } => {
            list_exchange(&exported, key_exch, list.bytes(), u.list_mic.ok_or(Rejected)?)?
        }
        _ => return Err(Rejected),
    };
    Ok(MechanismProof { revision: v, request: u.request, exported, final_gss, expires })
}

#[cfg(test)]
#[path = "../../test/smb/strict_verifier.rs"]
mod tests;

//! SMB 3.1.1 new-session preauthentication transcript (MS-SMB2 §§3.3.5.4,
//! 3.3.5.5.1, 3.3.5.5.3). These values are hashes, NEVER authentication,
//! connection ownership, signing keys, or permission to run a command.
//! This slice has no network issuer. The future dispatcher must associate
//! each state with its original connection/attempt, consume credits and pass
//! the actual immutable serialized response, not a prediction of that reply.
use super::{Error, Result, bytes, message::{Compound, Routing, SIGNED}, negotiate::{CmacChoice, Offer},
            setup_reply::Echo, u16le, u32le, u64le};
use crate::smb::handshake_hash::Sha512;
use digest::Digest;

const MAX_PACKET: usize = 68 * 1024;
const MAX_CONTINUES: u8 = 8; // bounded ZXS new-session profile, not an SMB constant
const MORE: u32 = 0xc000_0016;
type Hash = [u8; 64];

fn extend(previous: &Hash, message: &[u8]) -> Hash {
    // Exactly H(previous || SMB2 packet). Do not include the TCP prefix,
    // truncate to a token/body, zero fields, or discard transmitted padding.
    let mut h = Sha512::new();
    h.update(previous);
    h.update(message);
    let mut out = [0; 64];
    out.copy_from_slice(&h.finalize());
    out
}

fn invalid<T>() -> Result<T> { Err(Error::InvalidRequest) }

fn bound_packet(p: &[u8]) -> Result<()> {
    if p.len() > MAX_PACKET { Err(Error::Limit) } else { Ok(()) }
}

// Return the actual declared security field and its end, relative to this
// one SMB2 header. The wire layer does not decode it or grant authentication.
fn security_window(p: &[u8], fields: usize, fixed_end: usize) -> Result<(usize, &[u8])> {
    let _ = bytes(p, 0, fixed_end)?;
    let at = usize::from(u16le(p, fields)?);
    let len = usize::from(u16le(p, fields + 2)?);
    if at == 0 && len == 0 { return Ok((fixed_end, &p[..0])); }
    if at < fixed_end { return invalid(); }
    let token = bytes(p, at, len)?;
    Ok((at + token.len(), token))
}

fn security_end(p: &[u8], fields: usize, fixed_end: usize) -> Result<usize> {
    security_window(p, fields, fixed_end).map(|(end, _)| end)
}

// Called only on the server's own finalized, unframed, standalone reply.
// This narrow view checks transcript structure, not a complete NEGOTIATE or
// SESSION_SETUP implementation. No success response is generated here.
fn server_header(p: &[u8], cmd: u16, status: u32, mid: u64) -> Result<u64> {
    bound_packet(p)?;
    if bytes(p, 0, 4)? != b"\xfeSMB" || u16le(p, 4)? != 64 ||
       u32le(p, 8)? != status || u16le(p, 12)? != cmd ||
       (u32le(p, 16)? & 0x0f) != 1 || u32le(p, 20)? != 0 ||
       u64le(p, 24)? != mid || u32le(p, 36)? != 0 {
        return invalid();
    }
    u64le(p, 40)
}

fn selected_suite(p: &[u8], choice: CmacChoice) -> Result<()> {
    if u16le(p, 64)? != 65 || u16le(p, 66)? != 3 || u16le(p, 68)? != 0x0311 { return invalid(); }
    let n = usize::from(u16le(p, 70)?);
    if n == 0 { return invalid(); }
    if n > 64 { return Err(Error::Limit); }
    let token_end = security_end(p, 120, 128)?;
    let mut at = u32le(p, 124)? as usize;
    if at % 8 != 0 || at < token_end { return invalid(); }
    let mut selected = false;
    let mut cmac = false;
    for i in 0..n {
        let hdr = bytes(p, at, 8)?;
        let start = at + hdr.len();
        let data = bytes(p, start, usize::from(u16le(hdr, 2)?))?;
        let kind = u16le(hdr, 0)?;
        if kind == 1 {
            if selected || u16le(data, 0)? != 1 || u16le(data, 4)? != 1 {
                return invalid();
            }
            // Response has exactly one selected algorithm and its declared salt.
            if data.len() != 6 + usize::from(u16le(data, 2)?) { return invalid(); }
            selected = true;
        } else if kind == 8 {
            // The sender is ours. Require exactly one CMAC ID and no other bytes.
            if cmac || data != [1, 0, 1, 0] { return invalid(); }
            cmac = true;
        }
        if i + 1 != n { at = (start + data.len() + 7) & !7; }
    }
    if !selected || cmac != (choice != CmacChoice::Absent) { return invalid(); }
    Ok(())
}

#[must_use]
pub(in crate::smb) struct NegotiateRequestHash { value: Hash, mid: u64, choice: CmacChoice }

impl NegotiateRequestHash {
    /// Selected from this exact received offer, not recomputed using a client name or a second request.
    pub(in crate::smb) fn signing_choice(&self) -> CmacChoice { self.choice }

    pub(in crate::smb) fn received(offer: &Offer<'_>) -> Result<Self> {
        // The initial, direct SMB2 connection has one credit / MessageId 0.
        // Do not silently accept the second leg of an SMB1 multi-protocol
        // opener as a fresh connection. It needs a separate, reviewed path.
        let mid = u64le(offer.request, 24)?;
        if mid != 0 || u32le(offer.request, 36)? != 0 { return invalid(); }
        Ok(Self { value: extend(&[0; 64], offer.request), mid, choice: offer.cmac_choice()? })
    }

    pub(in crate::smb) fn reply(self, serialized: &[u8]) -> Result<ConnectionHash> {
        if server_header(serialized, 0, 0, self.mid)? != 0 { return invalid(); }
        selected_suite(serialized, self.choice)?;
        Ok(ConnectionHash(extend(&self.value, serialized)))
    }
}

/// Forked only at the completed NEGOTIATE boundary for each *new* setup.
/// Never reused after an endpoint reconnects; hash its new wire exchange.
#[must_use]
pub(in crate::smb) struct ConnectionHash(Hash);

impl ConnectionHash {
    pub(in crate::smb) fn new_attempt(&self) -> AwaitingRequest {
        AwaitingRequest { value: self.0, expected_sid: 0, continues: 0 }
    }
}

#[must_use]
pub(in crate::smb) struct AwaitingRequest { value: Hash, expected_sid: u64, continues: u8 }

#[must_use]
pub(in crate::smb) struct SetupRequestHash { value: Hash, mid: u64, sid: u64, echo:Echo, continues: u8 }

impl AwaitingRequest {
    pub(in crate::smb) fn received(self, envelope: &Compound<'_>) -> Result<SetupRequestHash> {
        self.received_token(envelope).map(|(step, _)| step)
    }

    /// Return the security field and hash from the *same* validated message;
    /// the mechanism layer never accepts a detached token from another request.
    pub(in crate::smb) fn received_token<'a>(
        self, envelope: &Compound<'a>,
    ) -> Result<(SetupRequestHash, &'a [u8])> {
        // Only isolated new-auth requests, not binding, reauthentication,
        // reconnect labelling or setup-plus-I/O. Do not hash unrelated bytes.
        let [m] = envelope.messages() else { return invalid(); };
        let p = m.raw;
        bound_packet(p)?;
        if m.header.command != 1 || m.header.flags & SIGNED != 0 ||
            m.header.routing != (Routing::Sync { tree: 0 }) ||
            m.header.session != self.expected_sid || u16le(p, 64)? != 25 ||
            bytes(p, 66, 1)? != b"\0" ||
            u64le(p, 80)? != 0 {
            return invalid();
        }
        let (_, token) = security_window(p, 76, 88)?;
        Ok((SetupRequestHash { value: extend(&self.value, p), mid: m.header.message,
                               sid: self.expected_sid, echo:Echo::from_request(m)?, continues: self.continues }, token))
    }
}

impl SetupRequestHash {
    pub(in crate::smb) fn more(self, serialized: &[u8]) -> Result<AwaitingRequest> {
        self.more_token(serialized).map(|(step, _)| step)
    }

    pub(in crate::smb) fn more_token(self, serialized: &[u8]) -> Result<(AwaitingRequest, &[u8])> {
        if self.continues == MAX_CONTINUES { return Err(Error::Limit); }
        let sid = server_header(serialized, 1, MORE, self.mid)?;
        if sid == 0 || sid == u64::MAX || (self.sid != 0 && sid != self.sid) ||
            u16le(serialized, 64)? != 9 || u16le(serialized, 66)? != 0 {
            return invalid();
        }
        let (_, token) = security_window(serialized, 68, 72)?;
        Ok((AwaitingRequest { value: extend(&self.value, serialized),
                              expected_sid: sid, continues: self.continues + 1 }, token))
    }

    /// KDF input when the *separate* authenticator proves this same exchange
    /// and releases its session key. No hash step accepts a final success reply:
    /// that reply is signed USING a key derived from this request-side hash.
    /// Dropping this value on auth failure publishes neither a session nor a key.
    pub(in crate::smb) fn into_kdf_context(self) -> RequestKeyContext {
        RequestKeyContext { value:self.value, sid:self.sid, mid:self.mid, echo:self.echo }
    }
}

/// Deliberately not a session key or authentication result. No public constructor.
pub(in crate::smb) struct RequestKeyContext { value:Hash, sid:u64, mid:u64, echo:Echo }

impl RequestKeyContext {
    pub(in crate::smb) fn as_bytes(&self) -> &[u8; 64] { &self.value }

    /// Unsigned template only. No caller can transmit or authenticate with a hash.
    pub(in crate::smb) fn final_template(&self,credits:u16,gss:&[u8])->Result<Vec<u8>> {
        self.echo.unsigned_final(credits,gss)
    }

    /// This profile's NTLM new session has already received a non-zero
    /// server session ID in its MORE reply. Only the successful unsigned
    /// serialized response to that exact final request can be signed next.
    pub(in crate::smb) fn check_final_reply(&self, p: &[u8]) -> Result<u64> {
        let sid = server_header(p, 1, 0, self.mid)?;
        if self.sid == 0 || sid != self.sid || !self.echo.same_unsigned(p) ||
            bytes(p, 48, 16)? != [0; 16] || u16le(p, 64)? != 9 || u16le(p, 66)? != 0 {
            return invalid();
        }
        security_end(p, 68, 72)?;
        Ok(sid)
    }
}

#[cfg(test)]
#[path = "../../test/smb/preauth.rs"]
pub(in crate::smb) mod tests;

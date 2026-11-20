//! SMB2 NEGOTIATE success bytes, not a transport or an authentication token.
//! MS-SMB2 2.2.1.2, 2.2.4, 3.3.4.3 and 3.3.5.4. The caller selects and
//! binds the result on the same Opening. No Direct TCP prefix enters here.
use super::{Error, Result, field, negotiate::{CmacChoice,Offer}, u16le, u32le, u64le};

const MODE: u16 = 3;       // signing enabled AND required
const LARGE_MTU: u32 = 4;  // sole global capability of our direct TCP profile
const MAX_IO: u32 = 65_536; // conservative per-operation limits; supports multi-credit
const FIXED_END: usize = 128;
const SALT_BEGIN: usize = FIXED_END + 8 + 6;
const PREAUTH_END: usize = SALT_BEGIN + 32;
const SIGN_BEGIN: usize = (PREAUTH_END + 7) & !7;
const SIGN_END: usize = SIGN_BEGIN + 8 + 4;

pub(in crate::smb) struct OriginalHeader { charge: u16, priority: u32, mid: u64, reserved: [u8;4] }
impl OriginalHeader {
    pub(in crate::smb) fn from_offer(offer: &Offer<'_>) -> Result<Self> {
        let p = offer.request;
        let flags = u32le(p,16)?;
        // Only a standalone unsigned sync NEGOTIATE is supported here, not a
        // DFS, replay or unrelated future header mode. Never echo arbitrary flags.
        if flags & !0x70 != 0 || field::<16>(p,48)? != [0;16] { return Err(Error::InvalidRequest); }
        Ok(Self { charge: u16le(p,6)?, priority: flags, mid: u64le(p,24)?, reserved: field(p,32)? })
    }
}

pub(in crate::smb) struct Selection {
    pub(in crate::smb) dialect:u16,
    pub(in crate::smb) guid: [u8;16],
    pub(in crate::smb) security_mode: u16,
    pub(in crate::smb) capabilities: u32,
}
impl Selection {
    /// Local sender's actual bytes, not the client's capabilities or a grant.
    pub(in crate::smb) fn emitted(p: &[u8]) -> Result<Self> {
        let fields = Self { dialect:u16le(p,68)?, guid: field(p,72)?, security_mode: u16le(p,66)?, capabilities: u32le(p,88)? };
        if fields.dialect!=0x0311 || fields.guid == [0;16] || fields.security_mode != MODE || fields.capabilities != LARGE_MTU {
            return Err(Error::InvalidRequest);
        }
        Ok(fields)
    }
}

fn word(p: &mut [u8], at: usize, n: u16) { p[at..at+2].copy_from_slice(&n.to_le_bytes()); }
fn dword(p: &mut [u8], at: usize, n: u32) { p[at..at+4].copy_from_slice(&n.to_le_bytes()); }
fn qword(p: &mut [u8], at: usize, n: u64) { p[at..at+8].copy_from_slice(&n.to_le_bytes()); }

/// Exact fixed profile for client-initiated authentication: zero-length GSS
/// token, no invented mechanism advertisement. The only optional response
/// context is the explicit CMAC selection when a signing context was processed.
pub(in crate::smb) fn encode(h: &OriginalHeader, choice: CmacChoice, grant: u16,
    guid_wire: &[u8;16], system_time: u64, salt: &[u8;32]) -> Result<Vec<u8>> {
    if h.mid != 0 || grant == 0 || *guid_wire == [0;16] { return Err(Error::InvalidRequest); }
    let explicit = choice != CmacChoice::Absent;
    let len = if explicit { SIGN_END } else { PREAUTH_END };
    let mut p = Vec::new();
    p.try_reserve_exact(len).map_err(|_| Error::Limit)?;
    p.resize(len,0);
    // Status SUCCESS, command NEGOTIATE, NextCommand, TreeId, SessionId and
    // the unsigned Signature are zero; never copy a request token/signature.
    p[..4].copy_from_slice(b"\xfeSMB"); word(&mut p,4,64); word(&mut p,6,h.charge);
    word(&mut p,14,grant); dword(&mut p,16,h.priority | 1); qword(&mut p,24,h.mid);
    p[32..36].copy_from_slice(&h.reserved);
    word(&mut p,64,65); word(&mut p,66,MODE); word(&mut p,68,0x0311);
    word(&mut p,70,if explicit { 2 } else { 1 });
    p[72..88].copy_from_slice(guid_wire); dword(&mut p,88,LARGE_MTU);
    for at in [92,96,100] { dword(&mut p,at,MAX_IO); }
    qword(&mut p,104,system_time); // ServerStartTime remains zero
    word(&mut p,120,FIXED_END as u16); // empty Buffer before the first aligned context
    dword(&mut p,124,FIXED_END as u32);
    word(&mut p,FIXED_END,1); word(&mut p,FIXED_END+2,38);
    word(&mut p,FIXED_END+8,1); word(&mut p,FIXED_END+10,32);
    word(&mut p,FIXED_END+12,1); // SHA-512, the only common preauth hash we accept
    p[SALT_BEGIN..PREAUTH_END].copy_from_slice(salt);
    if explicit {
        // MS-SMB2 requires CMAC even for a valid list with no common signer.
        // The client must reject an unoffered selection; the connection owner
        // also refuses session establishment in that case.
        word(&mut p,SIGN_BEGIN,8); word(&mut p,SIGN_BEGIN+2,4);
        word(&mut p,SIGN_BEGIN+8,1); word(&mut p,SIGN_BEGIN+10,1);
    }
    Ok(p)
}

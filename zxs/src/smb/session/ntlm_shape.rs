//! Structure only: an in-bounds field, UTF-16 name, version marker or MIC
//! flag is not an authentication proof. No passwords, account lookups or
//! cryptographic verification in this module.
use super::envelope::{Failure, Result};
pub(super) const MAGIC: &[u8; 8] = b"NTLMSSP\0";
const UNICODE: u32 = 1;
const REQUEST_TARGET: u32 = 0x4;
const DOMAIN_SUPPLIED: u32 = 0x1000;
const WORKSTATION_SUPPLIED: u32 = 0x2000;
const NTLM: u32 = 0x200;
const ANONYMOUS: u32 = 0x800;
const ESS: u32 = 0x0008_0000;
const TARGET_INFO: u32 = 0x0080_0000;
const VERSION: u32 = 0x0200_0000;
const KEY_EXCH: u32 = 0x4000_0000;
const REQUIRED: u32 = UNICODE | NTLM | ESS;

fn le16(p: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(p.get(at..at+2).and_then(|v| v.try_into().ok()).ok_or(Failure::Malformed)?))
}
fn le32(p: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(p.get(at..at+4).and_then(|v| v.try_into().ok()).ok_or(Failure::Malformed)?))
}
fn head(p: &[u8], kind: u32, flags_at: usize, fixed: usize) -> Result<(u32, usize)> {
    if !p.starts_with(MAGIC) || le32(p, 8)? != kind { return Err(Failure::Stage); }
    let f = le32(p, flags_at)?;
    if f & REQUIRED != REQUIRED || f & ANONYMOUS != 0 { return Err(Failure::Mechanism); }
    let end = fixed + if f & VERSION != 0 { 8 } else { 0 };
    if p.len() < end { return Err(Failure::Malformed); }
    Ok((f, end))
}

// The wire MaxLen is advisory and ignored on receipt. A zero-length field
// does not dereference its offset. Positive intervals cannot alias the
// fixed header or one another, even when their payload order is unusual.
fn fields<const N: usize>(p: &[u8], at: [usize; N], floor: usize, active: [bool; N])
    -> Result<[&[u8]; N]>
{
    let mut ranges = [(0, 0); N];
    let mut out = [&p[..0]; N];
    for (i, at) in at.into_iter().enumerate() {
        // Some descriptors MUST be ignored entirely if their flag is absent.
        if !active[i] { continue; }
        let n = usize::from(le16(p, at)?);
        if n == 0 { continue; }
        let a = usize::try_from(le32(p, at + 4)?).map_err(|_| Failure::Malformed)?;
        let b = a.checked_add(n).ok_or(Failure::Malformed)?;
        if a < floor || ranges[..i].iter().any(|&(lo, hi)| lo < b && a < hi) {
            return Err(Failure::Malformed);
        }
        out[i] = p.get(a..b).ok_or(Failure::Malformed)?;
        ranges[i] = (a, b);
    }
    Ok(out)
}

pub(super) fn negotiate(p: &[u8]) -> Result<()> {
    let (f, floor) = head(p, 1, 12, 32)?;
    fields(p, [16, 24], floor, [f & DOMAIN_SUPPLIED != 0, f & WORKSTATION_SUPPLIED != 0])?;
    Ok(())
}

/// Return the advertised NTLM MIC presence bit, never a verified MIC.
/// §3.3.2 may append one extra reserved Z(4) after the client's EOL.
/// Keep the original bytes for the later proof; don't normalize them.
fn av(mut p: &[u8], client: bool) -> Result<bool> {
    let mut seen = 0u16;
    let mut indicated = false;
    for _ in 0..64 {
        let id = le16(p, 0)?;
        let n = usize::from(le16(p, 2)?);
        let (v, tail) = p[4..].split_at_checked(n).ok_or(Failure::Malformed)?;
        if id == 0 {
            if n != 0 || !(tail.is_empty() || client && tail == [0; 4]) { return Err(Failure::Malformed); }
            // Our serialized server target info includes both mandatory NetBIOS
            // entries and a timestamp. Matching the client's response to that
            // exact issued data is a later proof check, not an AV-shape check.
            let required = (1 << 1) | (1 << 2) | (1 << 7);
            if !client && seen & required != required { return Err(Failure::Malformed); }
            return Ok(indicated);
        }
        if id > 10 { return Err(Failure::Unsupported); }
        let bit = 1u16 << id;
        if seen & bit != 0 { return Err(Failure::Malformed); }
        seen |= bit;
        match id {
            1..=5 | 9 if n % 2 == 0 => {}
            6 if n == 4 => indicated = le32(v, 0)? & 2 != 0,
            7 if n == 8 => {}
            8 if n >= 48 => {} // ignore remote Single_Host_Data after MachineID (§2.2.2.2)
            10 if n == 16 => {}
            _ => return Err(Failure::Malformed),
        }
        p = tail;
    }
    Err(Failure::Limit)
}

pub(super) fn challenge(p: &[u8]) -> Result<()> {
    let (f, floor) = head(p, 2, 20, 48)?;
    if f & TARGET_INFO == 0 { return Err(Failure::Mechanism); }
    let [target, info] = fields(p, [12, 40], floor, [f & REQUEST_TARGET != 0, true])?;
    if !target.is_empty() && (target.len() % 2 != 0 || le32(p, 16)? % 2 != 0) {
        return Err(Failure::Malformed);
    }
    av(info, false)?;
    Ok(())
}

pub(super) struct Authenticate<'a> {
    pub(super) raw: &'a [u8],
    pub(super) nt_response: &'a [u8],
    pub(super) domain_utf16: &'a [u8],
    pub(super) user_utf16: &'a [u8],
    pub(super) mic_indicated: bool,
}

pub(super) fn authenticate(p: &[u8]) -> Result<Authenticate<'_>> {
    let (f, floor) = head(p, 3, 60, 64)?;
    let descriptors = [12, 20, 28, 36, 44, 52];
    let key_present = f & KEY_EXCH != 0;
    let active = [true, true, true, true, true, key_present];
    let [lm, nt, domain, user, workstation, wrapped_key] = fields(p, descriptors, floor, active)?;
    if user.is_empty() || nt.len() < 48 || nt.get(16..18) != Some(&[1, 1][..]) {
        return Err(Failure::Mechanism); // no anonymous, LM-only or 24-byte NTLMv1 path
    }
    if !matches!(lm.len(), 0 | 24) || (key_present && wrapped_key.len() != 16) {
        return Err(Failure::Malformed);
    }
    for (at, v) in [(28, domain), (36, user), (44, workstation)] {
        if !v.is_empty() && (v.len() % 2 != 0 || le32(p, at + 4)? % 2 != 0) {
            return Err(Failure::Malformed);
        }
    }
    let mic_indicated = av(&nt[44..], true)?;
    if mic_indicated {
        // Prove there is room at least for the MIC before the payload.
        // Its exact placement (incl. the optional VERSION field), actual
        // contents, and the original three-message proof belong to the
        // mechanism verifier, NEVER to a flag or a guessed 16-byte gap.
        fields(p, descriptors, floor + 16, active)?;
    }
    Ok(Authenticate { raw: p, nt_response: nt, domain_utf16: domain, user_utf16: user, mic_indicated })
}

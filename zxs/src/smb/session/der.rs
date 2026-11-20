//! Fixed-schema, bounded DER views for RFC 4178. This is not general ASN.1.
use super::envelope::{Failure, Result};
const SPNEGO: &[u8] = &[0x2b, 6, 1, 5, 5, 2]; // 1.3.6.1.5.5.2
const NTLM: &[u8] = &[0x2b, 6, 1, 4, 1, 0x82, 0x37, 2, 2, 10];
const MAX_LIST: usize = 4096;
const MAX_MECHS: usize = 32;

struct Item<'a> { tag: u8, number: u32, whole: &'a [u8], contents: &'a [u8] }

fn take<'a>(input: &mut &'a [u8]) -> Result<Item<'a>> {
    let p = *input;
    let (&tag, mut rest) = p.split_first().ok_or(Failure::Malformed)?;
    let mut number = u32::from(tag & 31);
    if number == 31 {
        // Permit future context tags, but not non-minimal or unbounded IDs.
        if rest.first().is_some_and(|b| *b & 0x7f == 0) { return Err(Failure::Malformed); }
        number = 0;
        loop {
            let (&b, tail) = rest.split_first().ok_or(Failure::Malformed)?;
            rest = tail;
            number = number.checked_mul(128).and_then(|n| n.checked_add(u32::from(b & 127)))
                .ok_or(Failure::Limit)?;
            if b & 128 == 0 { break; }
        }
        if number < 31 { return Err(Failure::Malformed); }
    }
    let (&len, extra) = rest.split_first().ok_or(Failure::Malformed)?;
    // Definite shortest length; SMB SecurityBufferLength is a u16.
    let (length_bytes, n) = match len {
        0..=127 => (0, usize::from(len)),
        0x81 if extra.first().is_some_and(|b| *b >= 128) => (1, usize::from(extra[0])),
        0x82 if extra.len() >= 2 && extra[0] != 0 => (2, usize::from(u16::from_be_bytes([extra[0], extra[1]]))),
        _ => return Err(Failure::Malformed),
    };
    let start = p.len() - extra.len() + length_bytes;
    let (whole, tail) = p.split_at_checked(start + n).ok_or(Failure::Malformed)?;
    *input = tail;
    Ok(Item { tag, number, whole, contents: &whole[start..] })
}

fn one(mut p: &[u8], tag: u8) -> Result<Item<'_>> {
    let item = take(&mut p)?;
    if item.tag != tag || !p.is_empty() { return Err(Failure::Malformed); }
    Ok(item)
}

fn oid(v: &[u8]) -> Result<&[u8]> {
    if v.is_empty() || v.len() > 128 { return Err(Failure::Malformed); }
    let mut start = true;
    for &b in v {
        if start && b == 0x80 { return Err(Failure::Malformed); } // non-minimal base-128 arc
        start = b & 0x80 == 0;
    }
    if !start { return Err(Failure::Malformed); }
    Ok(v)
}

/// RFC 4178 §4.2.1: a named bit string ends at its last set bit; do not
/// demand exactly 32 encoded bits. reqFlags itself is ignored for policy.
fn flags(v: &[u8]) -> Result<()> {
    let (&unused, bits) = v.split_first().ok_or(Failure::Malformed)?;
    if unused > 7 || bits.len() > 4 || match bits.last() {
        None => unused != 0,
        Some(&last) => last == 0 || last.trailing_zeros() != u32::from(unused),
    } { return Err(Failure::Malformed); }
    Ok(())
}

pub(super) struct Init<'a> {
    pub(super) list: &'a [u8], // exact DER SEQUENCE, NOT its [0] wrapper
    pub(super) preferred: bool,
    pub(super) optimistic: Option<&'a [u8]>,
}

pub(super) fn initiator_initial(p: &[u8]) -> Result<Init<'_>> {
    let mut gss = one(p, 0x60)?.contents;
    let mech = take(&mut gss)?;
    if mech.tag != 6 || oid(mech.contents)? != SPNEGO { return Err(Failure::Mechanism); }
    let mut seq = one(one(gss, 0xa0)?.contents, 0x30)?.contents;
    let list_wrap = take(&mut seq)?;
    if list_wrap.tag != 0xa0 { return Err(Failure::Malformed); }
    let list = one(list_wrap.contents, 0x30)?;
    if list.whole.len() > MAX_LIST { return Err(Failure::Limit); }
    let mut items = list.contents;
    let (mut count, mut first_ntlm) = (0, None);
    while !items.is_empty() {
        if count == MAX_MECHS { return Err(Failure::Limit); }
        let item = take(&mut items)?;
        if item.tag != 6 { return Err(Failure::Malformed); }
        let value = oid(item.contents)?;
        if value == SPNEGO { return Err(Failure::Mechanism); } // must not offer SPNEGO to itself
        if value == NTLM && first_ntlm.is_none() { first_ntlm = Some(count); }
        count += 1;
    }
    if count == 0 { return Err(Failure::Malformed); }
    let preferred = first_ntlm.ok_or(Failure::Mechanism)? == 0;
    let mut prev = 0;
    let mut optimistic = None;
    let mut early_mic = false;
    while !seq.is_empty() {
        let f = take(&mut seq)?;
        if f.tag & 0xe0 != 0xa0 || f.number <= prev { return Err(Failure::Malformed); }
        prev = f.number;
        match f.tag {
            0xa1 => flags(one(f.contents, 3)?.contents)?,
            0xa2 => optimistic = Some(one(f.contents, 4)?.contents),
            // MS-SPNG §3.2.5: ignore a bounded Init2 negHints SEQUENCE
            // even if the client supplied it. Its names are not a mechanism
            // token, trusted target, identity, or permission.
            0xa3 if f.contents.first() == Some(&0x30) => { one(f.contents, 0x30)?; }
            // Early integrity data from another optimistic mechanism is not
            // evidence for NTLM. Never accept two early MICs.
            0xa3 | 0xa4 if !preferred && !early_mic => {
                one(f.contents, 4)?;
                early_mic = true;
            }
            0xa3 | 0xa4 => return Err(Failure::Stage),
            _ => {} // RFC 4178 §6: ignore unknown, bounded extension fields

        }
    }
    Ok(Init { list: list.whole, preferred, optimistic })
}

pub(super) struct Response<'a> {
    pub(super) state: Option<u8>,
    pub(super) selection: bool,
    pub(super) token: Option<&'a [u8]>,
    pub(super) mic: Option<&'a [u8]>,
}

pub(super) fn subsequent(p: &[u8]) -> Result<Response<'_>> {
    // Subsequent tokens MUST NOT get another outer GSS 0x60 wrapper.
    let mut seq = one(one(p, 0xa1)?.contents, 0x30)?.contents;
    let mut r = Response { state: None, selection: false, token: None, mic: None };
    let mut prev = None;
    while !seq.is_empty() {
        let f = take(&mut seq)?;
        if f.tag & 0xe0 != 0xa0 || prev.replace(f.number).is_some_and(|old| f.number <= old) {
            return Err(Failure::Malformed);
        }
        match f.tag {
            0xa0 => match one(f.contents, 0x0a)?.contents {
                [state @ 0..=3] => r.state = Some(*state),
                _ => return Err(Failure::Malformed),
            }
            0xa1 => {
                if oid(one(f.contents, 6)?.contents)? != NTLM { return Err(Failure::Mechanism); }
                r.selection = true;
            }
            0xa2 => r.token = Some(one(f.contents, 4)?.contents),
            0xa3 => r.mic = Some(one(f.contents, 4)?.contents),
            _ => {} // cannot select a new mechanism through an unknown extension
        }
    }
    Ok(r)
}

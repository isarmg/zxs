//! Credit-related fields only. Checking a variable data budget does not
//! validate a command, bind an OPEN, authenticate a session or grant credits.
use super::{Error, Result, bytes, message::Message, u16le, u32le, u64le};

#[derive(Clone, Copy)]
pub(in crate::smb) enum Cost { Small, Bytes(u64), Malformed }

impl Message<'_> {
    /// MS-SMB2 §§3.3.5.12/.13/.15/.18/.19/.20/.21: use the original
    /// declared payload/expected response, never an EOF, filtered page or a
    /// blanket "one credit for every command other than read/write".
    pub(in crate::smb) fn credit_cost(&self) -> Cost {
        let n = |at| u32le(self.raw, at).map(u64::from);
        let size = match self.header.command {
            0x08 | 0x09 | 0x0f | 0x11 => n(68),  // READ, WRITE, CHANGE_NOTIFY, SET_INFO
            0x0e => n(92),                        // QUERY_DIRECTORY
            0x10 => n(68).and_then(|out| n(76).map(|input| out.max(input))), // QUERY_INFO
            0x0b => (|| -> Result<u64> {         // IOCTL: both sums in u64
                Ok((n(92)? + n(104)?).max(n(96)? + n(108)?))
            })(),
            _ => return Cost::Small,
        };
        match size { Ok(v) => Cost::Bytes(v), Err(_) => Cost::Malformed }
    }
}

pub(in crate::smb) struct Limits {
    pub(in crate::smb) transact: u32,
    pub(in crate::smb) read: u32,
    pub(in crate::smb) write: u32,
}
pub(in crate::smb) fn direct_445_grant(p: &[u8]) -> Result<(u16, Limits)> {
    // The sibling preauth validator checks the original MID, successful 3.1.1
    // NEGOTIATE, signing selection and SHA-512 context from these SAME bytes.
    // §3.3.5.4 makes multi-credit mandatory on direct port 445 for 3.x.
    if u32le(p, 88)? & 4 == 0 { return Err(Error::InvalidRequest); }
    for at in [92,96,100] {
        let size = u32le(p,at)? as usize;
        if !(65_536..=(super::direct::MAX_FRAME_BYTES - 256)).contains(&size) {
            return Err(Error::InvalidRequest);
        }
    }
    let limits = Limits { transact: u32le(p,92)?, read: u32le(p,96)?, write: u32le(p,100)? };
    // Our receive path observes the whole inbound MaxTransactSize + 256 bound;
    // never advertise a single Write payload larger than it can admit. Read
    // data is outbound; this local consistency rule does not cap it at transact.
    if limits.write > limits.transact { return Err(Error::InvalidRequest); }
    Ok((u16le(p, 14)?, limits))
}

pub(in crate::smb) struct ReplyCredit {
    pub(in crate::smb) mid: u64,
    pub(in crate::smb) command: u16,
    pub(in crate::smb) grant: u16,
    pub(in crate::smb) status: u32,
}

/// The actual immutable outbound member (does not sign, construct or send
/// it). Async PENDING/interim/final replies require a separate owner so that
/// an interim grant can never also be released by a final response.
pub(in crate::smb) fn split_sync_reply(p: &[u8]) -> Result<(ReplyCredit, &[u8])> {
    if bytes(p, 0, 4)? != b"\xfeSMB" || u16le(p, 4)? != 64 ||
        u32le(p, 16)? & 3 != 1 || u32le(p, 8)? == 0x103 { return Err(Error::InvalidRequest); }
    let next = u32le(p, 20)? as usize;
    if next != 0 && (!next.is_multiple_of(8) || next >= p.len()) { return Err(Error::InvalidRequest); }
    let len = if next == 0 { p.len() } else { next };
    if len < 66 { return Err(Error::InvalidRequest); }
    Ok((ReplyCredit { mid: u64le(p,24)?, command: u16le(p,12)?, grant: u16le(p,14)?,
                      status: u32le(p,8)? }, &p[len..]))
}

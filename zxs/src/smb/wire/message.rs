//! MS-SMB2 §§2.2.1, 3.2.4.1.4, 3.3.5.2.7. Check the entire immutable
//! envelope before returning even the first command. Body validation, credits,
//! related-ID resolution and signatures are distinct, later steps.
use super::{Error, Result, bytes, direct::Frame, u16le, u32le, u64le};

pub(in crate::smb) const MAX_COMMANDS: usize = 32;
const HEADER: usize = 64;
const SERVER_TO_REDIR: u32 = 1;
const ASYNC: u32 = 2;
pub(in crate::smb) const RELATED: u32 = 4;
pub(in crate::smb) const SIGNED: u32 = 8;
pub(in crate::smb) const CANCEL: u16 = 0x000c;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum Routing {
    Sync { tree: u32 },
    AsyncCancel { id: u64 },
}

pub(in crate::smb) struct Header {
    pub(in crate::smb) command: u16,
    pub(in crate::smb) flags: u32,
    pub(in crate::smb) credit_charge: u16,
    pub(in crate::smb) credit_request: u16,
    pub(in crate::smb) channel_sequence: u16,
    pub(in crate::smb) message: u64,
    pub(in crate::smb) session: u64,
    pub(in crate::smb) routing: Routing,
}

pub(in crate::smb) struct Message<'a> {
    pub(in crate::smb) header: Header,
    // Includes all header, command bytes and compound padding, but never the
    // TCP length prefix or bytes of the next command. Never reconstruct it.
    pub(in crate::smb) raw: &'a [u8],
}

pub(in crate::smb) struct Compound<'a>(Vec<Message<'a>>);

impl<'a> Compound<'a> {
    pub(in crate::smb) fn parse(frame: Frame<'a>) -> Result<Self> {
        let mut rest = frame.payload();
        let mut parts = Vec::new();
        let mut related_style = None;
        loop {
            if parts.len() == MAX_COMMANDS {
                return Err(Error::Limit);
            }
            let magic = bytes(rest, 0, 4)?;
            if magic != b"\xfeSMB" {
                // Transforms need negotiated keys/capabilities; SMB1 is not
                // a downgrade path. No magic scan or recursive unwrap.
                return Err(Error::UnsupportedEnvelope);
            }
            if u16le(rest, 4)? != HEADER as u16 {
                return Err(Error::InvalidRequest);
            }
            let flags = u32le(rest, 16)?;
            let command = u16le(rest, 12)?;
            if flags & SERVER_TO_REDIR != 0 || (flags & ASYNC != 0 && command != CANCEL) {
                return Err(Error::InvalidRequest);
            }
            let related = flags & RELATED != 0;
            if parts.is_empty() {
                if related {
                    return Err(Error::InvalidRequest);
                }
            } else if related_style.replace(related).is_some_and(|prior| prior != related) {
                return Err(Error::InvalidRequest);
            }

            let next = u32le(rest, 20)? as usize;
            let length = if next == 0 { rest.len() } else { next };
            // Every SMB2 request begins its body with a two-byte StructureSize.
            if length < HEADER + 2 || (next != 0 && next % 8 != 0) {
                return Err(Error::InvalidRequest);
            }
            let (raw, tail) = rest.split_at_checked(length).ok_or(Error::InvalidRequest)?;
            let routing = if flags & ASYNC == 0 {
                Routing::Sync { tree: u32le(raw, 36)? }
            } else {
                Routing::AsyncCancel { id: u64le(raw, 32)? }
            };
            let header = Header {
                command, flags, routing,
                credit_charge: u16le(raw, 6)?,
                credit_request: u16le(raw, 14)?,
                channel_sequence: u16le(raw, 8)?,
                message: u64le(raw, 24)?,
                session: u64le(raw, 40)?,
            };
            parts.try_reserve(1).map_err(|_| Error::Limit)?;
            parts.push(Message { header, raw });
            if next == 0 {
                return Ok(Self(parts));
            }
            rest = tail;
        }
    }

    pub(in crate::smb) fn messages(&self) -> &[Message<'a>] {
        &self.0
    }
}

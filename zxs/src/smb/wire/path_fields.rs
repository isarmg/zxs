//! Only the bounded name fields of an already separated SMB2 request.
//! These views do not validate all CREATE contexts, grant an open, verify
//! signatures, resolve a related FileId, or authorize a rename.
use super::{bytes, field, message::{Message, RELATED, Routing}, u16le, u32le, u64le};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum FieldError { WrongCommand, Malformed, RenameInformationLength, Unsupported }

pub(in crate::smb) struct CreateNameField<'a> {
    pub(in crate::smb) name: &'a [u8],
    pub(in crate::smb) contexts: &'a [u8],
}

pub(in crate::smb) struct RenameNameField<'a> {
    /// Raw SMB2_FILEID in this request, not a ZXS FileId or an already
    /// resolved open. Compound related operations may carry all ones.
    pub(in crate::smb) source_wire_id: [u8; 16],
    pub(in crate::smb) name: &'a [u8],
    pub(in crate::smb) replace_if_exists: bool,
}

pub(in crate::smb) const TREE_SERVER_UNITS: usize = 255;
pub(in crate::smb) const TREE_SHARE_UNITS: usize = 80;
const TREE_RECONNECT: u16 = 1;
const TREE_REDIRECT: u16 = 2;
const TREE_EXTENSION: u16 = 4;
const REPLAY: u32 = 0x2000_0000;
const DFS: u32 = 0x1000_0000;
const OPEN_BY_FILE_ID: u32 = 0x2000;

impl<'a> Message<'a> {
    /// MS-SMB2 2.2.9: a plain UNC field of one TREE_CONNECT. The caller
    /// still needs the authenticated member and the real share resolver.
    pub(in crate::smb) fn tree_connect_name_field(&self) -> Result<&'a [u8], FieldError> {
        use FieldError::*;
        const FIXED: usize = 72;
        const MAX_NAME_BYTES: usize = (2 + TREE_SERVER_UNITS + 1 + TREE_SHARE_UNITS) * 2;
        if self.header.command != 3 { return Err(WrongCommand); }
        // A related request requires dispatch with its actual predecessor;
        // no such contract is implemented for a new TREE_CONNECT yet.
        if self.header.flags & (RELATED | DFS | REPLAY) != 0 { return Err(Unsupported); }
        if self.header.routing != (Routing::Sync { tree: 0 }) { return Err(Malformed); }
        let p = self.raw;
        if u16le(p, 64).map_err(|_| Malformed)? != 9 { return Err(Malformed); }
        let flags = u16le(p, 66).map_err(|_| Malformed)?;
        if flags & !(TREE_RECONNECT | TREE_REDIRECT | TREE_EXTENSION) != 0 { return Err(Malformed); }
        // An advisory redirect bit does not request a cluster connection.
        // Neither remoted identity nor reconnect is implemented; never
        // reinterpret an extension's bytes as the ordinary path.
        if flags & (TREE_RECONNECT | TREE_EXTENSION) != 0 { return Err(Unsupported); }
        let at = usize::from(u16le(p, 68).map_err(|_| Malformed)?);
        let n = usize::from(u16le(p, 70).map_err(|_| Malformed)?);
        if at < FIXED || n == 0 || n % 2 != 0 || n > MAX_NAME_BYTES { return Err(Malformed); }
        bytes(p, at, n).map_err(|_| Malformed)
    }

    /// MS-SMB2 2.2.13. Offsets start at this message's header, not the
    /// Direct TCP frame or the first member of a compound.
    pub(in crate::smb) fn create_name_fields(&self) -> Result<CreateNameField<'a>, FieldError> {
        use FieldError::*;
        if self.header.command != 5 { return Err(WrongCommand); }
        let p = self.raw;
        if p.len() < 121 || u16le(p, 64).map_err(|_| Malformed)? != 57 { return Err(Malformed); }
        if self.header.flags & DFS != 0 || u32le(p, 104).map_err(|_| Malformed)? & OPEN_BY_FILE_ID != 0 {
            return Err(Unsupported); // never interpret DFS names or binary IDs as business paths
        }
        let start = usize::from(u16le(p, 108).map_err(|_| Malformed)?);
        let n = usize::from(u16le(p, 110).map_err(|_| Malformed)?);
        let ctx = u32le(p, 112).map_err(|_| Malformed)? as usize;
        let count = u32le(p, 116).map_err(|_| Malformed)? as usize;
        let name = if n == 0 { &p[0..0] } else {
            if start < 120 || start % 8 != 0 { return Err(Malformed); }
            bytes(p, start, n).map_err(|_| Malformed)?
        };
        let contexts = if count == 0 {
            if ctx != 0 { return Err(Malformed); }
            &p[0..0]
        } else {
            if ctx < 120 || ctx % 8 != 0 || count < 16 { return Err(Malformed); }
            let c = bytes(p, ctx, count).map_err(|_| Malformed)?;
            // Both nonempty ranges were proven inside this one message.
            if n != 0 && start < ctx + c.len() && ctx < start + name.len() { return Err(Malformed); }
            c
        };
        Ok(CreateNameField { name, contexts })
    }

    /// MS-SMB2 2.2.39; MS-FSCC 2.4.42.2. Padding is not part of the name.
    pub(in crate::smb) fn basic_rename_name_field(&self) -> Result<RenameNameField<'a>, FieldError> {
        use FieldError::*;
        if self.header.command != 0x11 { return Err(WrongCommand); }
        if self.header.flags & DFS != 0 { return Err(Unsupported); }
        let p = self.raw;
        if p.len() < 96 || u16le(p,64).map_err(|_| Malformed)? != 33 { return Err(Malformed); }
        if p[66] != 1 || p[67] != 10 { return Err(Unsupported); } // FILE / FileRenameInformation only
        if u32le(p,76).map_err(|_| Malformed)? != 0 { return Err(Malformed); }
        let n = u32le(p,68).map_err(|_| Malformed)? as usize;
        let at = usize::from(u16le(p,72).map_err(|_| Malformed)?);
        if at < 96 { return Err(Malformed); }
        let b = bytes(p,at,n).map_err(|_| Malformed)?;
        if b.len() < 24 { return Err(RenameInformationLength); }
        if u64le(b,8).map_err(|_| Malformed)? != 0 { return Err(Malformed); }
        let len = u32le(b,16).map_err(|_| Malformed)? as usize;
        let name = bytes(b,20,len).map_err(|_| RenameInformationLength)?;
        Ok(RenameNameField { source_wire_id: field(p,80).map_err(|_| Malformed)?, name,
                             replace_if_exists: b[0] != 0 })
    }
}

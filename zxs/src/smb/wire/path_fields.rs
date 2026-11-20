//! Only the bounded name fields of an already separated SMB2 request.
//! These views do not validate all CREATE contexts, grant an open, verify
//! signatures, resolve a related FileId, or authorize a rename.
use super::{bytes, field, message::Message, u16le, u32le, u64le};

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

const DFS: u32 = 0x1000_0000;
const OPEN_BY_FILE_ID: u32 = 0x2000;

impl<'a> Message<'a> {
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

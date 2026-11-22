//! Complete MS-FSCC records for authorized actual logical children.
//! This is data for one original query, not a status, header, or acknowledgement.
use super::{RootCursorEffect,RootPage,RootPendingToken,Selection};
use crate::smb::{backend::{DIRECTORY_ATTRIBUTES,FILE_ATTRIBUTES,file_open::allocation},
    wire::query_fields::DirectoryClass};

#[derive(Debug, Eq, PartialEq)]
pub(in crate::smb::backend) enum EncodeFault { NotComplete, Resources, InconsistentPage }

/// The bytes cannot be separated from the same pending search position.
/// The final signed transport may only advance it after writing this result.
#[must_use]
pub(in crate::smb) struct FullRootRecords {
    proposal: RootPage,
    bytes: Vec<u8>,
}
impl FullRootRecords {
    pub(in crate::smb) fn data(&self) -> &[u8] { &self.bytes }
    pub(in crate::smb) fn pending(&self)->RootPendingToken<'_>{self.proposal.pending()}
    pub(in crate::smb) fn into_effect(self)->RootCursorEffect{self.proposal.into_effect()}

}

impl RootPage {
    pub(in crate::smb::backend) fn encode_complete(self) -> Result<FullRootRecords, EncodeFault> {
        use DirectoryClass::*;
        let Selection::Rows { entries, bytes, .. } = self.output() else { return Err(EncodeFault::NotComplete); };
        if entries.is_empty() { return Err(EncodeFault::InconsistentPage); }
        let fixed = self.class.fixed_bytes();
        let mut out = Vec::new();
        out.try_reserve_exact(*bytes).map_err(|_| EncodeFault::Resources)?;
        out.resize(*bytes, 0);
        for (i, record) in entries.iter().enumerate() {
            let at = record.space.offset;
            let end = at.checked_add(fixed).and_then(|v| v.checked_add(record.space.name_bytes))
                .ok_or(EncodeFault::InconsistentPage)?;
            let next = match entries.get(i + 1) {
                Some(n) if n.space.offset >= end && n.space.offset % 8 == 0 =>
                    u32::try_from(n.space.offset - at).map_err(|_| EncodeFault::InconsistentPage)?,
                None if end == out.len() => 0,
                _ => return Err(EncodeFault::InconsistentPage),
            };
            let bytes = out.get_mut(at..end).ok_or(EncodeFault::InconsistentPage)?;
            let n = u32::try_from(record.space.name_bytes).map_err(|_| EncodeFault::InconsistentPage)?;
            bytes[..4].copy_from_slice(&next.to_le_bytes());
            // No stable FileIndex or short-name aliases. Reserve and EA
            // fields are zero. The virtual folder has no data stream or
            // supplied timestamps; never substitute a host or current time.
            if self.class == Names {
                bytes[8..12].copy_from_slice(&n.to_le_bytes());
            } else {
                let attributes=if let Some(eof)=record.child.eof{
                    if eof>i64::MAX as u64{return Err(EncodeFault::InconsistentPage);}
                    bytes[40..48].copy_from_slice(&eof.to_le_bytes());
                    bytes[48..56].copy_from_slice(&allocation(eof).to_le_bytes());
                    FILE_ATTRIBUTES
                }else{DIRECTORY_ATTRIBUTES};
                bytes[56..60].copy_from_slice(&attributes.to_le_bytes());
                bytes[60..64].copy_from_slice(&n.to_le_bytes());
                let number = record.child.number.to_le_bytes();
                match self.class {
                    IdBoth => bytes[96..104].copy_from_slice(&number),
                    IdFull => bytes[72..80].copy_from_slice(&number),
                    _ => {}
                }
            }
            let mut words = record.child.name.encode_utf16();
            let (units, tail) = bytes[fixed..].as_chunks_mut::<2>();
            for dst in units {
                let word = words.next().ok_or(EncodeFault::InconsistentPage)?;
                *dst = word.to_le_bytes();
            }
            if words.next().is_some() || !tail.is_empty() { return Err(EncodeFault::InconsistentPage); }
        }
        Ok(FullRootRecords { proposal: self, bytes: out })
    }
}

#[cfg(test)]
include!("../../../../../tests/unit/attached/smb/backend/query/root/records/items.rs");

//! Complete MS-FSCC records for authorized logical namespace folders only.
//! This is data for one original query, not a status, header, or acknowledgement.
use super::{RootPage, Selection};
use crate::smb::wire::query_fields::DirectoryClass;

const DIRECTORY: u32 = 0x10;

#[derive(Debug, Eq, PartialEq)]
pub(in crate::smb::backend) enum EncodeFault { NotComplete, Resources, InconsistentPage }

/// The bytes cannot be separated from the same pending search position.
/// The final signed transport may only advance it after writing this result.
#[must_use]
pub(in crate::smb) struct FullRootRecords<'open> {
    proposal: RootPage<'open>,
    bytes: Vec<u8>,
}
impl FullRootRecords<'_> {
    pub(in crate::smb) fn data(&self) -> &[u8] { &self.bytes }

    #[cfg(test)]
    pub(in crate::smb::backend) fn written_for_fixture(self) { self.proposal.written_for_fixture(); }
}

impl<'open> RootPage<'open> {
    pub(in crate::smb::backend) fn encode_complete(self) -> Result<FullRootRecords<'open>, EncodeFault> {
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
                bytes[56..60].copy_from_slice(&DIRECTORY.to_le_bytes());
                bytes[60..64].copy_from_slice(&n.to_le_bytes());
                let number = record.child.number.to_le_bytes();
                match self.class {
                    IdBoth => bytes[96..104].copy_from_slice(&number),
                    IdFull => bytes[72..80].copy_from_slice(&number),
                    _ => {}
                }
            }
            let mut words = record.child.name.encode_utf16();
            let mut units = bytes[fixed..].chunks_exact_mut(2);
            while let Some(dst) = units.next() {
                let word = words.next().ok_or(EncodeFault::InconsistentPage)?;
                dst.copy_from_slice(&word.to_le_bytes());
            }
            if words.next().is_some() || !units.into_remainder().is_empty() { return Err(EncodeFault::InconsistentPage); }
        }
        Ok(FullRootRecords { proposal: self, bytes: out })
    }
}

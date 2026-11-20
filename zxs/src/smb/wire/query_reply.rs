//! SMB2 QUERY_DIRECTORY response body for a completed ZXS namespace page.
//! No header, status, signing, cursor commit, or I/O here.
use super::{Error, Result};
use crate::smb::backend::FullRootRecords;

const OUTPUT_OFFSET: u16 = 72; // 64-byte SMB2 header + 8-byte fixed body

#[must_use]
pub(in crate::smb) struct DirectoryReply<'open> {
    body: [u8; 8],
    records: FullRootRecords<'open>,
}

impl<'open> DirectoryReply<'open> {
    /// MS-SMB2 §2.2.34. The count covers these records, not any padding
    /// later needed between SMB2 compound response members.
    pub(in crate::smb) fn complete_root(records: FullRootRecords<'open>) -> Result<Self> {
        let n = u32::try_from(records.data().len()).map_err(|_| Error::Limit)?;
        if n == 0 {
            return Err(Error::InvalidRequest);
        }
        let mut body = [0; 8];
        body[..2].copy_from_slice(&9u16.to_le_bytes());
        body[2..4].copy_from_slice(&OUTPUT_OFFSET.to_le_bytes());
        body[4..8].copy_from_slice(&n.to_le_bytes());
        Ok(Self { body, records })
    }

    /// The page and its still-unsubmitted search position are not split.
    /// These bytes are not authorization to respond to any request.
    pub(in crate::smb) fn parts(&self) -> (&[u8], &[u8]) {
        (&self.body, self.records.data())
    }
}

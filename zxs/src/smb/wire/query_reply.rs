//! SMB2 QUERY_DIRECTORY body for an actual ZXS namespace query result.
//! No header, signing, cursor commit, or I/O here.
use super::{Error, Result};
use crate::smb::backend::{EndOfRoot, FullRootRecords, RootScanEnd};

const OUTPUT_OFFSET: u16 = 72; // 64-byte SMB2 header + 8-byte fixed body
const EMPTY_ERROR: [u8; 8] = [9, 0, 0, 0, 0, 0, 0, 0];
const NO_SUCH_FILE: u32 = 0xc000_000f;
const NO_MORE_FILES: u32 = 0x8000_0006;

enum ResultBody<'open> {
    Complete { body: [u8; 8], records: FullRootRecords<'open> },
    End(RootScanEnd<'open>),
}

#[must_use]
pub(in crate::smb) struct DirectoryReply<'open>(ResultBody<'open>);

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
        Ok(Self(ResultBody::Complete { body, records }))
    }

    /// MS-FSA §2.1.5.6.3 / MS-SMB2 §§2.2.2, 3.3.4.4. A real no-match
    /// outcome has an ERROR body and no directory data, including the
    /// NO_MORE_FILES warning. An overflow never passes this constructor.
    pub(in crate::smb) fn no_matching_root(end: RootScanEnd<'open>) -> Self {
        Self(ResultBody::End(end))
    }

    pub(in crate::smb) fn status(&self) -> u32 {
        match &self.0 {
            ResultBody::Complete { .. } => 0,
            ResultBody::End(end) => match end.kind() {
                EndOfRoot::NoInitialMatch => NO_SUCH_FILE,
                EndOfRoot::Exhausted => NO_MORE_FILES,
            },
        }
    }

    /// The original query result and still-unsubmitted search position
    /// stay owned together. These bytes are not authorization to respond.
    pub(in crate::smb) fn parts(&self) -> (&[u8], &[u8]) {
        match &self.0 {
            ResultBody::Complete { body, records } => (body, records.data()),
            ResultBody::End(_) => (&EMPTY_ERROR, &[]),
        }
    }
}

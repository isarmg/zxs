//! MS-SMB2 §2.1: Direct TCP, not the NetBIOS session service.
use super::{Error, Result};

// Local receive budget, not a claimed negotiated MaxReadSize/MaxWriteSize.
pub(in crate::smb) const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
pub(in crate::smb) struct Frame<'a>(&'a [u8]);

impl<'a> Frame<'a> {
    /// A stream reader must check this prefix before reserving the body.
    pub(in crate::smb) fn payload_len(prefix: [u8; 4]) -> Result<usize> {
        if prefix[0] != 0 {
            return Err(Error::InvalidFrame);
        }
        let length = u32::from_be_bytes(prefix) as usize;
        if length == 0 {
            return Err(Error::InvalidFrame);
        }
        if length > MAX_FRAME_BYTES {
            return Err(Error::Limit);
        }
        Ok(length)
    }

    /// None means incomplete input; a successful split consumes exactly one
    /// frame and leaves any coalesced TCP messages untouched.
    pub(in crate::smb) fn split(input: &'a [u8]) -> Result<Option<(Self, &'a [u8])>> {
        let Some((prefix, data)) = input.split_first_chunk::<4>() else {
            return Ok(None);
        };
        let length = Self::payload_len(*prefix)?;
        let Some((body, following)) = data.split_at_checked(length) else {
            return Ok(None);
        };
        Ok(Some((Self(body), following)))
    }

    pub(super) fn payload(&self) -> &'a [u8] {
        self.0
    }
}

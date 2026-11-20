//! SMB2 file response bodies only. These pieces are not packets or a
//! send capability. The owner must pair the command and status with the
//! original authenticated request, set final compound offsets/padding, and
//! sign that exact member, including its header, before any transmission.
use crate::smb::backend::{CloseCompletion, ReadCompletion, ReadFault};

const READ: u16 = 0x0008;
const CLOSE: u16 = 0x0006;

// MS-SMB2 §§2.2.2 and 3.3.4.4: StructureSize 9, eight actual fixed
// bytes, ErrorContextCount = ByteCount = 0. Only for the explicit
// contextless errors below; not for buffer-size, redirect or symlink errors.
const NO_ERROR_DATA: [u8; 8] = [9, 0, 0, 0, 0, 0, 0, 0];

#[derive(Clone, Copy)]
#[repr(u32)]
enum PlainReadError {
    EndOfFile = 0xc000_0011,
    AccessDenied = 0xc000_0022,
    InsufficientResources = 0xc000_009a,
    FileIsADirectory = 0xc000_00ba,
    NotSupported = 0xc000_00bb,
    InternalError = 0xc000_00e5,
}

enum Form<'a> {
    Read { fixed: [u8; 16], data: &'a [u8] },
    Close([u8; 60]),
    ReadError(PlainReadError),
}

/// An immutable command/status/body tuple. Success cannot be built from
/// arbitrary bytes, attributes, a wire handle, or a status value.
#[must_use]
pub(in crate::smb) struct FileReply<'a>(Form<'a>);

impl<'a> FileReply<'a> {
    /// MS-SMB2 §§2.2.20 and 3.3.5.12. Keep the verified payload borrowed.
    /// Concrete original errors go back to their real mapper unchanged;
    /// the generic codec does not guess whether a failure meant EOF.
    pub(in crate::smb) fn read<B: AsRef<[u8]>, E>(
        result: &'a Result<ReadCompletion<B>, ReadFault<E>>,
    ) -> Result<Self, &'a E> {
        let receipt = match result {
            Ok(receipt) => receipt,
            Err(fault) => {
                let status = match fault {
                    ReadFault::EndOfFile => PlainReadError::EndOfFile,
                    ReadFault::AccessDenied => PlainReadError::AccessDenied,
                    ReadFault::ResponseSpace => PlainReadError::InsufficientResources,
                    ReadFault::NotDataFile => PlainReadError::FileIsADirectory,
                    ReadFault::UnbufferedUnavailable => PlainReadError::NotSupported,
                    ReadFault::InconsistentLength => PlainReadError::InternalError,
                    ReadFault::Original(e) => return Err(e),
                };
                return Ok(Self(Form::ReadError(status)));
            }
        };
        let data = receipt.data();
        let count = receipt.count();
        if u32::try_from(data.len()).ok() != Some(count.get()) {
            return Ok(Self(Form::ReadError(PlainReadError::InternalError)));
        }
        let mut fixed = [0; 16];
        fixed[..2].copy_from_slice(&17u16.to_le_bytes());
        fixed[2] = 64 + 16; // DataOffset from this member's SMB2 header
        fixed[4..8].copy_from_slice(&count.get().to_le_bytes());
        // CHANNEL_NONE: DataRemaining and 3.1.1 response Flags are zero.
        Ok(Self(Form::Read { fixed, data }))
    }

    /// MS-SMB2 §§2.2.16 and 3.3.5.10. A faulted close has no receipt.
    /// Fetch failure or a plain close leaves the flag and attributes zero;
    /// flag 1 can only carry actual attributes from the original close.
    pub(in crate::smb) fn close(receipt: CloseCompletion) -> Self {
        let mut fixed = [0; 60];
        fixed[..2].copy_from_slice(&60u16.to_le_bytes());
        if let Some(a) = receipt.attributes() {
            fixed[2..4].copy_from_slice(&1u16.to_le_bytes());
            for (field, value) in fixed[8..56].chunks_exact_mut(8).zip([
                a.created, a.accessed, a.written, a.changed, a.allocation_size, a.end_of_file,
            ]) {
                field.copy_from_slice(&value.to_le_bytes());
            }
            fixed[56..60].copy_from_slice(&a.file_attributes.to_le_bytes());
        }
        Self(Form::Close(fixed))
    }

    pub(in crate::smb) fn command(&self) -> u16 {
        match &self.0 { Form::Read { .. } | Form::ReadError(_) => READ, Form::Close(_) => CLOSE }
    }

    /// NTSTATUS for the eventual SMB2 response header, not part of its body.
    pub(in crate::smb) fn status(&self) -> u32 {
        match &self.0 {
            Form::ReadError(code) => *code as u32,
            _ => 0, // STATUS_SUCCESS, only for the receipt-derived forms
        }
    }

    /// Unframed fixed body and optional verified data, without compound padding.
    pub(in crate::smb) fn parts(&self) -> (&[u8], &[u8]) {
        match &self.0 {
            Form::Read { fixed, data } => (fixed.as_slice(), data),
            Form::Close(body) => (body.as_slice(), &[]),
            Form::ReadError(_) => (&NO_ERROR_DATA, &[]),
        }
    }
}

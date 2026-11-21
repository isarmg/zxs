//! SMB2 file response bodies only. These pieces are not packets or a
//! send capability. The owner must pair the command and status with the
//! original authenticated request, set final compound offsets/padding, and
//! sign that exact member, including its header, before any transmission.
use crate::smb::backend::{
    CloseCompletion, FlushCompletion, PreparedDirectoryRename, PreparedDisposition, PreparedFileEof,
    PreparedOrdinaryRename, PreparedRetainedRestore, ReadCompletion, ReadFault, WriteCompletion, WriteFault,
};

const READ: u16 = 0x0008;
const CLOSE: u16 = 0x0006;
pub(in crate::smb) const WRITE: u16 = 0x0009;
pub(in crate::smb) const FLUSH: u16 = 0x0007;

// MS-SMB2 §§2.2.2 and 3.3.4.4: StructureSize 9, eight actual fixed
// bytes, ErrorContextCount = ByteCount = 0. Only for the explicit
// contextless errors below; not for buffer-size, redirect or symlink errors.
const NO_ERROR_DATA: [u8; 8] = [9, 0, 0, 0, 0, 0, 0, 0];

#[derive(Clone, Copy)]
#[repr(u32)]
enum PlainError {
    EndOfFile = 0xc000_0011,
    InvalidParameter = 0xc000_000d,
    AccessDenied = 0xc000_0022,
    InsufficientResources = 0xc000_009a,
    FileIsADirectory = 0xc000_00ba,
    NotSupported = 0xc000_00bb,
    InternalError = 0xc000_00e5,
}

enum Form<'a> {
    Read { fixed: [u8; 16], data: &'a [u8] },
    Close([u8; 60]),
    Write([u8; 16]),
    Flush([u8; 4]),
    SetInfo([u8;2]),
    Failure { command: u16, code: PlainError },
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
                    ReadFault::EndOfFile => PlainError::EndOfFile,
                    ReadFault::AccessDenied => PlainError::AccessDenied,
                    ReadFault::ResponseSpace => PlainError::InsufficientResources,
                    ReadFault::NotDataFile => PlainError::FileIsADirectory,
                    ReadFault::UnbufferedUnavailable => PlainError::NotSupported,
                    ReadFault::InconsistentLength => PlainError::InternalError,
                    ReadFault::Original(e) => return Err(e),
                };
                return Ok(Self(Form::Failure { command: READ, code: status }));
            }
        };
        let data = receipt.data();
        let count = receipt.count();
        if u32::try_from(data.len()).ok() != Some(count.get()) {
            return Ok(Self(Form::Failure { command: READ, code: PlainError::InternalError }));
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
    pub(in crate::smb) fn close(receipt: &CloseCompletion) -> Self {
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

    /// MS-SMB2 §§2.2.22 and 3.3.5.13. Count is the checked accepted count
    /// from the original writer, never the request's unverified Length.
    pub(in crate::smb) fn write<E>(
        result: &Result<WriteCompletion, WriteFault<E>>,
    ) -> Result<Self, &WriteFault<E>> {
        let receipt = match result {
            Ok(receipt) => receipt,
            Err(fault) => return Self::write_failure(WRITE, fault),
        };
        Ok(Self::completed_write(receipt))
    }

    /// MS-SMB2 §§2.2.18 and 3.3.5.11. No success before the actual flush.
    pub(in crate::smb) fn flush<E>(
        result: &Result<FlushCompletion, WriteFault<E>>,
    ) -> Result<Self, &WriteFault<E>> {
        match result {
            Ok(checked) => Ok(Self::completed_flush(checked)),
            Err(fault) => Self::write_failure(FLUSH, fault),
        }
    }

    pub(in crate::smb) fn completed_write(receipt:&WriteCompletion)->Self {
        let mut fixed = [0; 16];
        fixed[..2].copy_from_slice(&17u16.to_le_bytes());
        fixed[4..8].copy_from_slice(&receipt.count().to_le_bytes());
        // Remaining, reserved and WriteChannelInfo are zero.
        Self(Form::Write(fixed))
    }

    pub(in crate::smb) fn completed_flush(_receipt:&FlushCompletion)->Self {
        Self(Form::Flush([4, 0, 0, 0]))
    }

    /// MS-SMB2 §2.2.40; no resulting EOF or host metadata in this body.
    pub(in crate::smb) fn completed_end_of_file(_receipt:&PreparedFileEof)->Self {
        Self(Form::SetInfo([2,0]))
    }

    /// Acceptance of an exact original mark/clear is not a deletion result.
    pub(in crate::smb) fn completed_disposition(_receipt:&PreparedDisposition)->Self {
        Self(Form::SetInfo([2,0]))
    }

    /// Only this exact confirmed kind-12 movement can encode this SET_INFO.
    pub(in crate::smb) fn completed_restore(_receipt:&PreparedRetainedRestore)->Self {
        Self(Form::SetInfo([2,0]))
    }

    /// Only the confirmed same-parent rename of the actual held FILE.
    pub(in crate::smb) fn completed_rename(_receipt:&PreparedOrdinaryRename)->Self {
        Self(Form::SetInfo([2,0]))
    }

    /// Same fixed body, but only the confirmed original directory move.
    pub(in crate::smb) fn completed_directory_rename(_receipt:&PreparedDirectoryRename)->Self {
        Self(Form::SetInfo([2,0]))
    }

    fn write_failure<E>(command: u16, fault: &WriteFault<E>) -> Result<Self, &WriteFault<E>> {
        let code = match fault {
            WriteFault::AccessDenied => PlainError::AccessDenied,
            WriteFault::NotDataFile => PlainError::FileIsADirectory,
            WriteFault::UnbufferedUnavailable | WriteFault::DirectoryFlushUnavailable => PlainError::NotSupported,
            WriteFault::InvalidRange | WriteFault::InvalidWriteCombination => PlainError::InvalidParameter,
            WriteFault::InconsistentLength => PlainError::InternalError,
            // Keep failed mutation and failed confirmation apart. Only the
            // actual adapter knows their NTSTATUS and recovery semantics.
            WriteFault::Original(_) | WriteFault::Confirmation(_) => return Err(fault),
        };
        Ok(Self(Form::Failure { command, code }))
    }

    pub(in crate::smb) fn command(&self) -> u16 {
        match &self.0 {
            Form::Read { .. } => READ,
            Form::Close(_) => CLOSE,
            Form::Write(_) => WRITE,
            Form::Flush(_) => FLUSH,
            Form::SetInfo(_)=>super::set_info::SET_INFO,
            Form::Failure { command, .. } => *command,
        }
    }

    /// NTSTATUS for the eventual SMB2 response header, not part of its body.
    pub(in crate::smb) fn status(&self) -> u32 {
        match &self.0 {
            Form::Failure { code, .. } => *code as u32,
            Form::Read { .. } | Form::Close(_) | Form::Write(_) | Form::Flush(_) | Form::SetInfo(_) => 0,
        }
    }

    /// Unframed fixed body and optional verified data, without compound padding.
    pub(in crate::smb) fn parts(&self) -> (&[u8], &[u8]) {
        match &self.0 {
            Form::Read { fixed, data } => (fixed.as_slice(), data),
            Form::Close(body) => (body.as_slice(), &[]),
            Form::Write(body) => (body.as_slice(), &[]),
            Form::Flush(body) => (body.as_slice(), &[]),
            Form::SetInfo(body)=>(body.as_slice(),&[]),
            Form::Failure { .. } => (&NO_ERROR_DATA, &[]),
        }
    }
}

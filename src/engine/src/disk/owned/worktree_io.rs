//! Fresh relative full-file copies under the owning disk's revocable I/O gate.
use crate::disk::owned::{DiskRoot, OperationCopyReader};
use platform_io::fs_safety::EntryName;
use platform_io::payload::{Expectation, Receipt};
use std::io::{self, Read};

#[derive(Debug, error_definition::Error)]
pub(crate) enum NewEntryCopyError {
    #[error("admitting full-file copy: {0}")]
    Admission(#[source] crate::Error),
    #[error("copying and publishing complete verified bytes: {0}")]
    Payload(#[source] platform_io::payload::Error),
    #[error("full-file copy was published before its disk admission was revoked: {source}")]
    Published {
        receipt: Receipt,
        #[source]
        source: crate::Error,
    },
}

/// A callback can revoke work, not authorize a disk, replace an input or issue
/// a receipt. The underlying disk operation already bounds each read to 64 KiB.
struct CheckedRead<'a, R, C> {
    source: &'a mut R,
    check: &'a mut C,
}

impl<R: Read, C: FnMut() -> crate::Result<()>> Read for CheckedRead<'_, R, C> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        (self.check)().map_err(io::Error::other)?;
        let count = self.source.read(bytes)?;
        (self.check)().map_err(io::Error::other)?;
        Ok(count)
    }
}

impl DiskRoot {
    pub(crate) fn copy_new_entry_verified_synced_observed(
        &self,
        name: &EntryName,
        input: &mut impl Read,
        expectation: Expectation,
        check: &mut impl FnMut() -> crate::Result<()>,
    ) -> Result<Receipt, NewEntryCopyError> {
        check().map_err(NewEntryCopyError::Admission)?;
        let mut reader = CheckedRead { source: input, check };
        self.copy_new_entry_verified_synced(name, &mut reader, expectation)
    }

    /// Copy into a fresh entry only. Matching existing bytes are not adoption
    /// authority. The caller owns the parent namespace fence until completion.
    /// Payload publication errors preserve their unknown/published phase; this
    /// adapter never removes the destination or retries a new name.
    pub(crate) fn copy_new_entry_verified_synced(
        &self,
        name: &EntryName,
        input: &mut impl Read,
        expectation: Expectation,
    ) -> Result<Receipt, NewEntryCopyError> {
        let operation = self.operation().map_err(NewEntryCopyError::Admission)?;
        let directory = operation.private_directory(&self.path)
            .map_err(NewEntryCopyError::Admission)?;
        let mut reader = OperationCopyReader { input, operation: &operation };
        let receipt = platform_io::payload::write_full(&directory, name, &mut reader, expectation)
            .map_err(NewEntryCopyError::Payload)?;
        match operation.check_running() {
            Ok(()) => Ok(receipt),
            Err(source) => Err(NewEntryCopyError::Published { receipt, source }),
        }
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/disk/new_entry_copy.rs"]
mod tests;

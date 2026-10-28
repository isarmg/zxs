//! Count the real atomic persistence boundary only in the test build.
use platform_io::fs_safety::{Error, PrivateDirectory, RelativePath};
use std::{cell::Cell, fs::File};

thread_local! {
    static REPLACEMENTS: Cell<usize> = const { Cell::new(0) };
    static FAIL_AFTER_REPLACE: Cell<bool> = const { Cell::new(false) };
}

pub(in crate::smb_rpc) fn replacements() -> usize {
    REPLACEMENTS.get()
}

pub(in crate::smb_rpc) fn fail_after_replace() {
    FAIL_AFTER_REPLACE.set(true);
}

pub(super) struct AtomicFile;
impl AtomicFile {
    pub(super) fn replace_held(
        directory: &PrivateDirectory,
        destination: &RelativePath,
        bytes: &[u8],
    ) -> Result<File, Error> {
        REPLACEMENTS.set(REPLACEMENTS.get() + 1);
        let result =
            platform_io::fs_safety::AtomicFile::replace_held(directory, destination, bytes);
        if FAIL_AFTER_REPLACE.replace(false) {
            result?;
            return Err(Error::PublishedDurabilityUnknown(std::io::Error::other(
                "injected after real recovery record replacement",
            )));
        }
        result
    }
}

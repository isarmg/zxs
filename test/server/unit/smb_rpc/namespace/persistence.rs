//! Exercise orchestration after real publication; actual barrier EIO lives in
//! the platform's isolated interposer tests, not this forwarding wrapper.
use platform_io::fs_safety::{EntryName, Error, PrivateDirectory};

thread_local! {
    static FAIL_AFTER_PUBLICATION: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    static FAIL_BEFORE_PUBLICATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(feature = "core")]
pub(in crate::smb_rpc) fn fail_before_record_write() {
    FAIL_BEFORE_PUBLICATION.set(true);
}

#[cfg(feature = "core")]
pub(in crate::smb_rpc) fn fail_next_record_sync(directory: bool) {
    FAIL_AFTER_PUBLICATION.set(Some(directory));
}

pub(super) struct AtomicFile;
impl AtomicFile {
    pub(super) fn create_recovery_intent(
        directory: &PrivateDirectory,
        name: &EntryName,
        bytes: &[u8],
    ) -> Result<(), Error> {
        if FAIL_BEFORE_PUBLICATION.replace(false) {
            return Err(Error::Io(std::io::Error::other(
                "injected failure before namespace record write",
            )));
        }
        platform_io::fs_safety::AtomicFile::create_recovery_intent(directory, name, bytes)?;
        if let Some(directory) = FAIL_AFTER_PUBLICATION.take() {
            return Err(Error::PublishedDurabilityUnknown(std::io::Error::other(
                if directory {
                    "injected directory durability uncertainty after real publication"
                } else {
                    "injected file durability uncertainty after real publication"
                },
            )));
        }
        Ok(())
    }
}

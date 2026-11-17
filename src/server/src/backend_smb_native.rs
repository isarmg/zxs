//! Connection-owned native work. Only the actual core backend can create its handle.
use name_policy::LookupPath;
use storage_contract::file_draft::DraftLocation;

pub struct SmbNativeOpen<'a> {
    pub branch: &'a str,
    pub path: &'a LookupPath,
    pub writable: bool,
    pub create: bool,
    pub truncate: bool,
    pub exclusive: bool,
}

pub struct SmbNativeHandle {
    pub(crate) target: DraftLocation,
    #[cfg(feature = "core")]
    pub(crate) held: storage_engine::NativeWorkingFile,
}

impl SmbNativeHandle {
    pub(crate) fn operation(&self) -> Option<storage_contract::OperationId> {
        #[cfg(feature = "core")]
        { self.held.operation() }
        #[cfg(not(feature = "core"))]
        { None }
    }
}

pub enum SmbNativeEdit<'a> {
    Write { offset: u64, bytes: &'a [u8] },
    Append { bytes: &'a [u8] },
    Resize { length: u64 },
}

//! Create an upload inode exclusively through its already admitted container.
//! The provisional filename is not a Save. Never adopt or unlink a collision.
use super::{DiskFile, DiskRoot, RootOperation};
use crate::Result;
use crate::persist::layout::container::working_pack;
use platform_io::fs_safety::linux::SingleLinkRequirement;
use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
use std::fs::File;
use storage_contract::file_draft::manifest::packed::PackId;

impl DiskRoot {
    pub(crate) fn create_private_working_pack(&self, id: PackId) -> Result<DiskFile> {
        let operation = self.operation_with(Some(self.admit_io()?))?;
        let name = working_pack(id);
        let file = File::from(openat2(
            operation.held().directory(), name.as_os_str(),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        ).map_err(std::io::Error::from)?);
        SingleLinkRequirement::check(&file)?;
        let RootOperation { directory, lease, .. } = operation;
        drop(directory);
        let file = lease.expect("invariant: pack creation requires managed disk I/O").hold(file);
        file.check_running()?;
        Ok(file)
    }
}

use crate::Result;
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
use std::{fs::File, io::Write, os::unix::fs::PermissionsExt};

pub(crate) fn open() -> Result<File> {
    let payload = super::payload::required()?;
    sealed(payload.bytes)
}
fn sealed(bytes: &[u8]) -> Result<File> {
    let mut file = File::from(memfd_create(
        "zxs-matching-maintenance",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )?);
    file.write_all(bytes)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o555))?;
    fcntl_add_seals(
        &file,
        SealFlags::WRITE | SealFlags::SHRINK | SealFlags::GROW | SealFlags::SEAL,
    )?;
    Ok(file)
}

#[cfg(test)]
#[path = "../../../test/helper/unit/installation/worker_tests.rs"]
mod tests;

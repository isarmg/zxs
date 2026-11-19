//! Observations from real, held local descriptors. These facts are not a claim or a volume role.
use crate::pinned::{Entity, Mount, invalid, local_data_filesystem};
use helper_linux_evidence::{Birth, read_birth};
use std::{fs::File, io, os::unix::fs::MetadataExt};

#[derive(PartialEq, Eq)]
pub(super) struct Here { pub(super) entity: Entity, pub(super) mount: u64 }

pub(super) fn mount_id(file: &File) -> io::Result<u64> {
    Mount::on(file)?.map(|m| m.id).ok_or_else(|| io::Error::new(
        io::ErrorKind::Unsupported, "original descriptor has no reported mount identity"))
}

pub(super) fn check_root(system: &File, storage_uid: u32) -> io::Result<Here> {
    let m = system.metadata()?;
    if !m.is_dir() || m.uid() != storage_uid || m.nlink() == 0 || m.mode() & 0o022 != 0 {
        return Err(invalid("expected a protected directory owned by the selected storage service"));
    }
    local_data_filesystem(system)?;
    Ok(Here { entity: Entity::of(&m)?, mount: mount_id(system)? })
}

#[derive(PartialEq, Eq)]
pub(super) struct ObservedOriginal { pub(super) at: Here, pub(super) birth: Birth }
impl ObservedOriginal {
    pub(super) fn root(dir: &File, uid: u32) -> io::Result<Self> {
        let at = check_root(dir, uid)?;
        Ok(Self { birth: read_birth(dir, at.mount)?, at })
    }
    pub(super) fn fields(&self) -> (u64, u64, &[u8]) {
        (self.at.entity.dev, self.at.entity.ino, self.birth.payload())
    }
}

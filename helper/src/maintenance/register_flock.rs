//! First-inode portion of an offline initialization, not a pool-level writer permit.
//! No constructor adopts an externally opened file. The bare allocation does not link a name or claim a volume.
pub(super) mod staging;

use super::genesis::FreshGenesis;
use super::identity::{Here, ObservedOriginal, check_root, mount_id};
use helper_linux_evidence::read_birth;
use crate::pinned::{Entity, invalid};
use rustix::fs::{FlockOperation, Mode, OFlags, fcntl_getfl, flock, openat};
use rustix::io::{FdFlags, fcntl_getfd};
use std::{fs::File, io, os::unix::fs::MetadataExt};

fn check_new(file: &File, root: &Here, storage_uid: u32) -> io::Result<Here> {
    check_unlinked(file, root, storage_uid, 0)
}

fn check_unlinked(file: &File, root: &Here, storage_uid: u32, expected_length: u64) -> io::Result<Here> {
    let flags = fcntl_getfl(file)?;
    let m = file.metadata()?;
    if !flags.contains(OFlags::TMPFILE) || flags & OFlags::ACCMODE != OFlags::RDWR
        || !fcntl_getfd(file)?.contains(FdFlags::CLOEXEC)
        || !m.is_file() || m.mode() & 0o7777 != 0o600 || m.uid() != storage_uid
        || m.nlink() != 0 || m.len() != expected_length || m.dev() != root.entity.dev {
        return Err(invalid("expected the original unnamed private registration inode at its exact stage length"));
    }
    let here = Here { entity: Entity::of(&m)?, mount: mount_id(file)? };
    if here.entity == root.entity || here.mount != root.mount {
        return Err(invalid("registration is not a distinct inode on the selected mount"));
    }
    Ok(here)
}

/// Holds the exact open file description until its last owner closes it. The
/// handle is private, not Clone/AsFd, and does not expose unlock or write.
#[must_use]
pub(super) struct UnpublishedRegisterFlock<'s> { held: File, system: &'s File, storage_uid: u32 }
impl<'s> UnpublishedRegisterFlock<'s> {
    /// Internal primitive only. The future initializer must first authenticate
    /// the operator, take the entire ordered set of root locks, and revalidate
    /// the held original roots, names and claims. This primitive alone cannot
    /// authorize that call: independent fresh inodes do not exclude other pools.
    pub(super) fn create_in_held_system(system: &'s File, storage_uid: u32) -> io::Result<Self> {
        let root = check_root(system, storage_uid)?;
        // No O_EXCL: on O_TMPFILE it would prohibit a later first publication.
        // No fallback to a named tempfile. This allocates an inode, even though
        // this constructor never writes or links it.
        let held = File::from(openat(system, c".", OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
                                    Mode::RUSR | Mode::WUSR)?);
        let original = check_new(&held, &root, storage_uid)?;
        flock(&held, FlockOperation::NonBlockingLockExclusive)?;
        if check_root(system, storage_uid)? != root || check_new(&held, &root, storage_uid)? != original {
            return Err(invalid("source registration or SYSTEM changed during lock acquisition"));
        }
        Ok(Self { held, system, storage_uid })
    }

    /// Take both births from the live original descriptors under our own EX,
    /// never by reopening a path. The registration must still be unnamed and empty;
    /// this method does not inspect the contents of any directory.
    pub(super) fn fresh_origins(&self) -> io::Result<(ObservedOriginal, ObservedOriginal)> {
        self.unlinked_origins(0)
    }

    fn unlinked_origins(&self, expected_length: u64) -> io::Result<(ObservedOriginal, ObservedOriginal)> {
        let s = check_root(self.system, self.storage_uid)?;
        let r = check_unlinked(&self.held, &s, self.storage_uid, expected_length)?;
        let system = ObservedOriginal { birth: read_birth(self.system, s.mount)?, at: s };
        let registration = ObservedOriginal { birth: read_birth(&self.held, r.mount)?, at: r };
        if system.birth.filesystem_uuid() != registration.birth.filesystem_uuid()
            || system.birth == registration.birth
            || check_root(self.system, self.storage_uid)? != system.at
            || check_unlinked(&self.held, &system.at, self.storage_uid, expected_length)? != registration.at {
            return Err(invalid("registration or SYSTEM is not its distinct original on the same filesystem"));
        }
        Ok((registration, system))
    }

    /// Compare the supplied fresh image with this allocation before any
    /// first write. This does not verify original root names, labels, other
    /// volumes, operator authority, or exclusivity shared with the online service.
    pub(super) fn check_image_origins(&self, image: &FreshGenesis<'_>) -> io::Result<()> {
        self.check_unlinked_image(image, 0)
    }

    fn check_unlinked_image(&self, image: &FreshGenesis<'_>, expected_length: u64) -> io::Result<()> {
        let (registration, system) = self.unlinked_origins(expected_length)?;
        if !image.matches_origin_pair(self.storage_uid, registration.fields(), system.fields()) {
            return Err(invalid("fresh image does not name the two observed original objects"));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/maintenance/register_flock.rs"]
mod tests;

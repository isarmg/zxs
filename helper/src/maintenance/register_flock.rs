//! First-inode portion of an offline initialization, not a pool-level writer permit.
//! No constructor adopts an externally opened file. Nothing here links a name or claims a volume.
use super::genesis::FreshGenesis;
use helper_linux_evidence::{Birth, read_birth};
use crate::pinned::{Entity, Mount, invalid, local_data_filesystem};
use rustix::fs::{FlockOperation, Mode, OFlags, fcntl_getfl, flock, openat};
use rustix::io::{FdFlags, fcntl_getfd};
use std::{fs::File, io, os::unix::fs::MetadataExt};

#[derive(PartialEq, Eq)]
struct Here { entity: Entity, mount: u64 }

fn mount_id(file: &File) -> io::Result<u64> {
    Mount::on(file)?.map(|m| m.id).ok_or_else(|| io::Error::new(
        io::ErrorKind::Unsupported, "original descriptor has no reported mount identity"))
}

fn check_root(system: &File, storage_uid: u32) -> io::Result<Here> {
    let m = system.metadata()?;
    if !m.is_dir() || m.uid() != storage_uid || m.nlink() == 0 || m.mode() & 0o022 != 0 {
        return Err(invalid("expected a protected directory owned by the selected storage service"));
    }
    local_data_filesystem(system)?;
    Ok(Here { entity: Entity::of(&m)?, mount: mount_id(system)? })
}

fn check_new(file: &File, root: &Here, storage_uid: u32) -> io::Result<Here> {
    let flags = fcntl_getfl(file)?;
    let m = file.metadata()?;
    if !flags.contains(OFlags::TMPFILE) || flags & OFlags::ACCMODE != OFlags::RDWR
        || !fcntl_getfd(file)?.contains(FdFlags::CLOEXEC)
        || !m.is_file() || m.mode() & 0o7777 != 0o600 || m.uid() != storage_uid
        || m.nlink() != 0 || m.len() != 0 || m.dev() != root.entity.dev {
        return Err(invalid("expected the original empty, unnamed private registration inode"));
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
        // it is never linked or written by this stage.
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
    fn fresh_origins(&self) -> io::Result<(ObservedOriginal, ObservedOriginal)> {
        let s = check_root(self.system, self.storage_uid)?;
        let r = check_new(&self.held, &s, self.storage_uid)?;
        let system = ObservedOriginal { birth: read_birth(self.system, s.mount)?, at: s };
        let registration = ObservedOriginal { birth: read_birth(&self.held, r.mount)?, at: r };
        if system.birth.filesystem_uuid() != registration.birth.filesystem_uuid()
            || system.birth == registration.birth
            || check_root(self.system, self.storage_uid)? != system.at
            || check_new(&self.held, &system.at, self.storage_uid)? != registration.at {
            return Err(invalid("registration or SYSTEM is not its distinct original on the same filesystem"));
        }
        Ok((registration, system))
    }

    /// Compare the supplied fresh image with this allocation before any
    /// first write. This does not verify original root names, labels, other
    /// volumes, operator authority, or exclusivity shared with the online service.
    pub(super) fn check_image_origins(&self, image: &FreshGenesis<'_>) -> io::Result<()> {
        let (registration, system) = self.fresh_origins()?;
        if !image.matches_origin_pair(self.storage_uid, registration.fields(), system.fields()) {
            return Err(invalid("fresh image does not name the two observed original objects"));
        }
        Ok(())
    }
}

struct ObservedOriginal { at: Here, birth: Birth }
impl ObservedOriginal {
    fn fields(&self) -> (u64, u64, &[u8]) {
        (self.at.entity.dev, self.at.entity.ino, self.birth.payload())
    }
}

#[cfg(test)]
#[path = "../../tests/maintenance/register_flock.rs"]
mod tests;

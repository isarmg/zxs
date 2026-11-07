//! Descriptor ancestry with stable mount-root identity, never transient MNT_ID.
use super::{Error, Result};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::linux::{FileIdentity, MountPolicy, OpenAt2Root};
use rustix::fs::{Mode, OFlags, ResolveFlags};
use std::{fs::File, path::Path};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct MountRoot {
    device: u64,
    inode: u64,
}
impl From<FileIdentity> for MountRoot {
    fn from(identity: FileIdentity) -> Self {
        Self {
            device: identity.device,
            inode: identity.inode,
        }
    }
}

pub(super) struct Location {
    _root: OpenAt2Root,
    identity: FileIdentity,
    mount_root: MountRoot,
    ancestors: Vec<FileIdentity>,
}
impl Location {
    pub(super) fn open(path: &Path) -> Result<Self> {
        Self::from_root(OpenAt2Root::open(path, MountPolicy::SameMount)?)
    }
    pub(super) fn from_root(root: OpenAt2Root) -> Result<Self> {
        let identity = root.identity();
        let mut current = root.directory().try_clone()?;
        let mut ancestors = Vec::new();
        for _ in 0..256 {
            let here = FileIdentity::from_fd(&current)?;
            ancestors.push(here);
            let parent = match rustix::fs::openat2(
                &current,
                "..",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::NO_XDEV | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            ) {
                Ok(parent) => File::from(parent),
                Err(rustix::io::Errno::XDEV) => {
                    return Ok(Self {
                        _root: root,
                        identity,
                        mount_root: here.into(),
                        ancestors,
                    });
                }
                Err(error) => return Err(std::io::Error::from(error).into()),
            };
            if FileIdentity::from_fd(&parent)? == here {
                return Ok(Self {
                    _root: root,
                    identity,
                    mount_root: here.into(),
                    ancestors,
                });
            }
            current = parent;
        }
        Err(Error::Budget)
    }
    pub(super) fn refreshed(&self) -> Result<Self> {
        Self::from_root(OpenAt2Root::from_directory(
            self._root.directory().try_clone()?,
            MountPolicy::SameMount,
        )?)
    }
    pub(super) fn create_child_exclusive(
        &self,
        name: &platform_io::fs_safety::EntryName,
        path: &Path,
    ) -> Result<(platform_io::fs_safety::PrivateDirectory, Self)> {
        // Unlike PrivateDirectory::create_child, an appeared entry is never
        // adopted. The opened child's physical separation is checked again by
        // the caller before any marker or cache bytes are written.
        rustix::fs::mkdirat(
            self._root.directory(),
            name.as_os_str(),
            Mode::from_raw_mode(0o700),
        )
        .map_err(std::io::Error::from)?;
        let child = Self::from_root(OpenAt2Root::from_directory(
            self._root.open_directory(&name.as_relative())?,
            MountPolicy::SameMount,
        )?)?;
        let directory = child.private_directory(path)?;
        Ok((directory, child))
    }
    pub(super) fn private_directory(
        &self,
        path: &Path,
    ) -> Result<platform_io::fs_safety::PrivateDirectory> {
        let directory = platform_io::fs_safety::PrivateDirectory::from_directory(
            self._root.directory().try_clone()?,
            path.to_owned(),
        )?;
        directory.verify_identity()?;
        Ok(directory)
    }
    /// Open an existing child relative to the same held root; never create it.
    pub(super) fn private_child(
        &self,
        name: &platform_io::fs_safety::EntryName,
        path: &Path,
    ) -> Result<platform_io::fs_safety::PrivateDirectory> {
        Ok(platform_io::fs_safety::PrivateDirectory::from_directory(
            self._root.open_directory(&name.as_relative())?,
            path.join(name.as_path()),
        )?)
    }
    pub(super) fn identity(&self) -> FileIdentity {
        self.identity
    }
    pub(super) fn mount_root(&self) -> MountRoot {
        self.mount_root
    }
    pub(super) fn overlaps(&self, other: &Self) -> Result<bool> {
        Ok(self.descends_from(other.identity, other.mount_root)?
            || other.descends_from(self.identity, self.mount_root)?)
    }
    pub(super) fn descends_from(
        &self,
        ancestor: FileIdentity,
        mount_root: MountRoot,
    ) -> Result<bool> {
        if self.identity.device != ancestor.device {
            return Ok(false);
        }
        // A bind of a subdirectory hides ancestry above its physical mount root.
        // Reject that unproved relation instead of assuming names are disjoint.
        // Complete-filesystem aliases retain the same physical root and every
        // intervening physical ancestor, even when their current MNT_ID differs.
        if self.mount_root != mount_root {
            return Err(Error::Invalid(
                "same filesystem selected through different physical mount roots",
            ));
        }
        Ok(self.ancestors.contains(&ancestor))
    }
}

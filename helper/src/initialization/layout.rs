use crate::Result;
use platform_io::{
    fs_safety::{
        AtomicCommitError, EntryName, Error,
        linux::{FileIdentity, MountPolicy, OpenAt2Root},
    },
    service_lease::{HOST_ROOT, ServiceLease},
};
use rustix::{
    fs::{AtFlags, Mode, RenameFlags, fchown, mkdirat, renameat_with, statat},
    process::{Gid, Uid},
};
use std::{
    fs::File,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

pub(super) const CHILDREN: &[&str] = &["state", "management", "disks"];
pub(super) struct Destination {
    root: OpenAt2Root,
    lease: ServiceLease,
    owner: (u32, u32),
}
#[derive(Debug)]
struct PublicationFailure {
    parent: FileIdentity,
    pending: String,
    source: AtomicCommitError,
}
impl std::fmt::Display for PublicationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "private data layout publication under {:?}, pending {:?}, target data; preserve all evidence",
            self.parent, self.pending
        )
    }
}
impl std::error::Error for PublicationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
impl Destination {
    pub(super) fn host(owner: (u32, u32)) -> Result<Self> {
        Self::open(Path::new(HOST_ROOT), owner)
    }
    fn open(path: &Path, owner: (u32, u32)) -> Result<Self> {
        let lease = ServiceLease::acquire(path, rustix::process::geteuid().as_raw())?;
        let root = OpenAt2Root::open(path, MountPolicy::AllowNestedMounts)?;
        if root.identity() != lease.identity() {
            return Err("private layout parent differs from the held service lease".into());
        }
        Ok(Self { root, lease, owner })
    }
    pub(super) fn identity(&self) -> FileIdentity {
        self.root.identity()
    }
    fn open_named(&self, name: &EntryName) -> Result<Option<OpenAt2Root>> {
        match self.root.open_directory(&name.as_relative()) {
            Ok(file) => Ok(Some(OpenAt2Root::from_directory(
                file,
                MountPolicy::SameMount,
            )?)),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    pub(super) fn matching_or_absent(&self) -> Result<bool> {
        self.lease.recheck()?;
        let Some(directory) = self.open_named(&EntryName::new("data")?)? else {
            return Ok(false);
        };
        check_owner(directory.directory(), self.owner)?;
        self.complete_children(&directory)?;
        check_named(&self.root, &EntryName::new("data")?, directory.identity())?;
        self.lease.recheck()?;
        Ok(true)
    }
    pub(super) fn publish(
        &self,
        digest: &str,
        revalidate: impl Fn() -> Result<()>,
    ) -> Result<bool> {
        let target = EntryName::new("data")?;
        let pending_name = format!(".zxs-helper-data-{digest}.pending");
        let pending = EntryName::new(&pending_name)?;
        self.lease.recheck()?;
        if let Some(existing) = self.open_named(&target)? {
            if self.open_named(&pending)?.is_some() {
                return Err(
                    "both final and pending private data layouts exist; neither was changed".into(),
                );
            }
            check_owner(existing.directory(), self.owner)?;
            self.complete_children(&existing)?;
            for name in CHILDREN {
                existing
                    .open_directory(&EntryName::new(*name)?.as_relative())?
                    .sync_all()?;
            }
            existing.directory().sync_all()?;
            self.root.directory().sync_all()?;
            check_named(&self.root, &target, existing.identity())?;
            self.lease.recheck()?;
            revalidate()?;
            return Ok(false);
        }
        let directory = match self.open_named(&pending)? {
            Some(directory) => directory,
            None => {
                mkdirat(
                    self.root.directory(),
                    pending.as_os_str(),
                    Mode::from_raw_mode(0o700),
                )?;
                let file = self.root.open_directory(&pending.as_relative())?;
                file.set_permissions(std::fs::Permissions::from_mode(0o700))?;
                OpenAt2Root::from_directory(file, MountPolicy::SameMount)?
            }
        };
        self.root.directory().sync_all()?;
        let metadata = directory.directory().metadata()?;
        if metadata.mode() & 0o7777 != 0o700
            || (metadata.uid() != rustix::process::geteuid().as_raw()
                && (metadata.uid(), metadata.gid()) != self.owner)
        {
            return Err(
                "pending private layout has unexpected owner/mode; it was not repaired".into(),
            );
        }
        check_names(&directory)?;
        // Inspect every already present child before creating any missing child.
        for name in CHILDREN {
            match directory.open_directory(&EntryName::new(*name)?.as_relative()) {
                Ok(child) => {
                    check_owner(&child, self.owner)?;
                    empty(&OpenAt2Root::from_directory(child, MountPolicy::SameMount)?)?;
                }
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        for name in CHILDREN {
            let entry = EntryName::new(*name)?;
            match directory.open_directory(&entry.as_relative()) {
                Ok(child) => child.sync_all()?,
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    mkdirat(
                        directory.directory(),
                        entry.as_os_str(),
                        Mode::from_raw_mode(0o700),
                    )?;
                    let child = directory.open_directory(&entry.as_relative())?;
                    // Only the inode exclusively created by this call is handed off.
                    set_owner(&child, self.owner)?;
                    child.sync_all()?;
                    check_named(&directory, &entry, FileIdentity::from_fd(&child)?)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        self.complete_children(&directory)?;
        revalidate()?;
        // This dedicated, exact-request pending tree is never a preexisting live
        // data root. An interrupted top-level handoff can finish the same plan.
        if (metadata.uid(), metadata.gid()) != self.owner {
            set_owner(directory.directory(), self.owner)?;
        }
        check_owner(directory.directory(), self.owner)?;
        directory.directory().sync_all()?;
        self.complete_children(&directory)?;
        check_named(&self.root, &pending, directory.identity())?;
        self.lease.recheck()?;
        let failure = |source| PublicationFailure {
            parent: self.identity(),
            pending: pending_name.clone(),
            source,
        };
        renameat_with(
            self.root.directory(),
            pending.as_os_str(),
            self.root.directory(),
            target.as_os_str(),
            RenameFlags::NOREPLACE,
        )
        .map_err(|error| failure(AtomicCommitError::Unknown(Error::Io(error.into()))))?;
        check_named(&self.root, &target, directory.identity())
            .map_err(|error| failure(AtomicCommitError::Published(error)))?;
        self.root.directory().sync_all().map_err(|error| {
            failure(AtomicCommitError::Published(
                Error::PublishedDurabilityUnknown(error),
            ))
        })?;
        self.lease.recheck()?;
        self.complete_children(&directory)?;
        Ok(true)
    }
    fn complete_children(&self, directory: &OpenAt2Root) -> Result<()> {
        check_names(directory)?;
        for name in CHILDREN {
            let child = directory.open_directory(&EntryName::new(*name)?.as_relative())?;
            check_owner(&child, self.owner)?;
            empty(&OpenAt2Root::from_directory(child, MountPolicy::SameMount)?)?;
        }
        Ok(())
    }
}
fn check_owner(file: &File, owner: (u32, u32)) -> Result<()> {
    let metadata = file.metadata()?;
    if (metadata.uid(), metadata.gid()) != owner || metadata.mode() & 0o7777 != 0o700 {
        return Err("private layout requires the exact selected UID/GID and mode 0700; existing permissions were not changed".into());
    }
    Ok(())
}
fn set_owner(file: &File, owner: (u32, u32)) -> Result<()> {
    fchown(
        file,
        Some(Uid::from_raw(owner.0)),
        Some(Gid::from_raw(owner.1)),
    )?;
    file.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn check_names(directory: &OpenAt2Root) -> Result<()> {
    for entry in directory.entries()? {
        let name = entry?;
        if !CHILDREN
            .iter()
            .any(|expected| name.as_os_str() == std::ffi::OsStr::new(*expected))
        {
            return Err(
                "private layout contains an unexpected entry; existing data was preserved".into(),
            );
        }
    }
    Ok(())
}
fn empty(directory: &OpenAt2Root) -> Result<()> {
    if let Some(entry) = directory.entries()?.next() {
        let _ = entry?;
        return Err("private layout is not pristine; no existing data was cleared".into());
    }
    Ok(())
}
fn check_named(
    parent: &OpenAt2Root,
    name: &EntryName,
    identity: FileIdentity,
) -> std::result::Result<(), Error> {
    let stat = statat(
        parent.directory(),
        name.as_os_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(std::io::Error::from)?;
    if (stat.st_dev, stat.st_ino) != (identity.device, identity.inode) {
        return Err(Error::IdentityChanged(name.as_path().to_owned()));
    }
    Ok(())
}
#[cfg(test)]
#[path = "../../../test/helper/unit/initialization/layout_tests.rs"]
mod tests;

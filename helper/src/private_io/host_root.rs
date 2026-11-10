use super::PublicationFailure;
use crate::Result;
use platform_io::{
    fs_safety::{
        AtomicCommitError, EntryName, Error,
        linux::{AdvisoryLock, FileIdentity, MountPolicy, OpenAt2Root},
    },
    service_lease::{HOST_ROOT, ServiceLease},
};
use rustix::fs::{AtFlags, Mode, RenameFlags, mkdirat, renameat_with, statat};
use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// Root creation is helper-only. The generic lease reader never creates it.
pub(crate) struct HostRoot {
    parent_path: PathBuf,
    parent: OpenAt2Root,
    target: EntryName,
    pending: EntryName,
    _lock: AdvisoryLock,
}
impl HostRoot {
    pub(crate) fn fixed() -> Result<Self> {
        let path = Path::new(HOST_ROOT);
        Self::open(
            path.parent()
                .expect("invariant: fixed host root has a parent"),
            path.file_name()
                .expect("invariant: fixed host root has a name"),
        )
    }
    fn open(parent_path: &Path, name: &std::ffi::OsStr) -> Result<Self> {
        let parent = OpenAt2Root::open(parent_path, MountPolicy::AllowNestedMounts)?;
        let metadata = parent.directory().metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o022 != 0 {
            return Err(
                "host parent must belong to this user without group/other write access".into(),
            );
        }
        let lock = AdvisoryLock::directory(parent.directory())?;
        let result = Self {
            parent_path: parent_path.to_owned(),
            parent,
            target: EntryName::new(name)?,
            pending: EntryName::new(".zxs-helper-host.pending")?,
            _lock: lock,
        };
        result.recheck_parent()?;
        Ok(result)
    }
    pub(crate) fn parent_identity(&self) -> FileIdentity {
        self.parent.identity()
    }
    fn recheck_parent(&self) -> Result<()> {
        let current = OpenAt2Root::open(&self.parent_path, MountPolicy::AllowNestedMounts)?;
        if current.identity() != self.parent.identity() {
            return Err(
                "host root parent identity changed; inspect retained evidence before retrying"
                    .into(),
            );
        }
        let metadata = current.directory().metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o022 != 0 {
            return Err("host root parent permissions changed".into());
        }
        Ok(())
    }
    fn open_named(&self, name: &EntryName) -> Result<Option<OpenAt2Root>> {
        let file = match self.parent.open_directory(&name.as_relative()) {
            Ok(file) => file,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        let mode = metadata.mode() & 0o7777;
        // Only the fixed, unpublished staging name may still have mkdir's
        // private mode. The live root must already have its final attributes.
        let initial_pending = name == &self.pending && mode == 0o700;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || (mode != 0o755 && !initial_pending)
        {
            return Err("host root requires this user's ownership and mode 0755; only the original unpublished pending root may retain mode 0700; neither was repaired or replaced".into());
        }
        Ok(Some(OpenAt2Root::from_directory(
            file,
            MountPolicy::AllowNestedMounts,
        )?))
    }
    pub(crate) fn present(&self) -> Result<bool> {
        Ok(self.open_named(&self.target)?.is_some())
    }
    pub(crate) fn prepare(&self) -> Result<(bool, ServiceLease)> {
        self.recheck_parent()?;
        if let Some(root) = self.open_named(&self.target)? {
            if self.open_named(&self.pending)?.is_some() {
                return Err(
                    "both host root and pending root exist; neither was removed or changed".into(),
                );
            }
            let lease = ServiceLease::acquire(
                &self.parent_path.join(self.target.as_os_str()),
                rustix::process::geteuid().as_raw(),
            )?;
            if root.identity() != lease.identity() {
                return Err("host root differs from the held service lease".into());
            }
            root.directory().sync_all()?;
            self.parent.directory().sync_all()?;
            self.check_named(&self.target, root.identity())?;
            self.recheck_parent()?;
            return Ok((false, lease));
        }
        let root = match self.open_named(&self.pending)? {
            Some(root) => root,
            None => {
                mkdirat(
                    self.parent.directory(),
                    self.pending.as_os_str(),
                    Mode::from_raw_mode(0o700),
                )?;
                self.open_named(&self.pending)?
                    .ok_or("new pending host root disappeared; no replacement was accepted")?
            }
        };
        if let Some(entry) = root.entries()?.next() {
            return Err(format!(
                "pending host root is not empty: {:?}; retain it for explicit recovery",
                entry?.as_os_str()
            )
            .into());
        }
        // Lock the future daemon inode before publishing its public name.
        let lease = ServiceLease::acquire_directory(
            &self.parent_path.join(self.pending.as_os_str()),
            rustix::process::geteuid().as_raw(),
            root,
        )?;
        let root = lease.root();
        self.check_named(&self.pending, root.identity())?;
        self.recheck_parent()?;
        // A retry after mkdir resumes this same empty, administrative-owned
        // pending inode. Never chmod the final root or any nonempty staging tree.
        root.directory()
            .set_permissions(std::fs::Permissions::from_mode(0o755))?;
        root.directory().sync_all()?;
        self.parent.directory().sync_all()?;
        self.check_named(&self.pending, root.identity())?;
        self.recheck_parent()?;
        let failure = |source| PublicationFailure {
            directory: self.parent.identity(),
            staging: self.pending.as_os_str().to_string_lossy().into_owned(),
            target: self.target.as_path().to_owned(),
            identity: root.identity(),
            source,
        };
        renameat_with(
            self.parent.directory(),
            self.pending.as_os_str(),
            self.parent.directory(),
            self.target.as_os_str(),
            RenameFlags::NOREPLACE,
        )
        .map_err(|error| failure(AtomicCommitError::Unknown(Error::Io(error.into()))))?;
        self.check_named(&self.target, root.identity())
            .map_err(|error| failure(AtomicCommitError::Published(error)))?;
        self.parent.directory().sync_all().map_err(|error| {
            failure(AtomicCommitError::Published(
                Error::PublishedDurabilityUnknown(error),
            ))
        })?;
        self.recheck_parent()?;
        Ok((
            true,
            lease.published_at(&self.parent_path.join(self.target.as_os_str()))?,
        ))
    }
    fn check_named(
        &self,
        name: &EntryName,
        expected: FileIdentity,
    ) -> std::result::Result<(), Error> {
        let stat = statat(
            self.parent.directory(),
            name.as_os_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(std::io::Error::from)?;
        if (stat.st_dev, stat.st_ino) != (expected.device, expected.inode) {
            return Err(Error::IdentityChanged(name.as_path().to_owned()));
        }
        Ok(())
    }
}
#[cfg(test)]
#[path = "../../../test/helper/unit/host/root_tests.rs"]
mod tests;

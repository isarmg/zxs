use crate::Result;
use platform_io::{
    fs_safety::{
        EntryName, Error,
        linux::{AdvisoryLock, FileIdentity, MountPolicy, OpenAt2Root},
    },
    publication::publish_directory,
    service_lease::{HOST_ROOT, ServiceLease},
};
use rustix::fs::{Mode, mkdirat};
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
            self.parent
                .verify_entry_identity(&self.target, root.identity())?;
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
        self.parent
            .verify_entry_identity(&self.pending, root.identity())?;
        self.recheck_parent()?;
        // A retry after mkdir resumes this same empty, administrative-owned
        // pending inode. Never chmod the final root or any nonempty staging tree.
        root.directory()
            .set_permissions(std::fs::Permissions::from_mode(0o755))?;
        root.directory().sync_all()?;
        self.parent.directory().sync_all()?;
        self.parent
            .verify_entry_identity(&self.pending, root.identity())?;
        self.recheck_parent()?;
        publish_directory(&self.parent, &self.pending, &self.target, root)?;
        self.recheck_parent()?;
        Ok((
            true,
            lease.published_at(&self.parent_path.join(self.target.as_os_str()))?,
        ))
    }
}
#[cfg(test)]
#[path = "../../../test/helper/unit/host/root_tests.rs"]
mod tests;

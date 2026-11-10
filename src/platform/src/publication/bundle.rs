//! Publish one inert configuration directory. The deterministic pending name
//! is retained on failure; the same exact request can finish missing files.
use super::Result;
use super::{publish_directory, write_atomic_at};
use crate::fs_safety::{
    EntryName, Error,
    linux::{AdvisoryLock, FileIdentity, MountPolicy, OpenAt2Root, SingleLinkRequirement},
};
use crate::service_lease::ServiceLease;
use rustix::fs::{Mode, mkdirat};
use std::{
    fs::File,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub enum BundlePublication {
    Created,
    Resumed,
    Existing,
}
/// Current retained publication stage, without completing or syncing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleState {
    Absent,
    Pending,
    Published,
}
pub struct BundleDestination {
    path: PathBuf,
    parent: OpenAt2Root,
    _lock: Exclusion,
}
enum Exclusion {
    Independent { _lock: AdvisoryLock },
    Admitted { _lease: ServiceLease },
}
#[derive(Debug)]
struct PendingBundle {
    parent: FileIdentity,
    staging: String,
    target: String,
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for PendingBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "inactive deployment preparation failed under {:?}; retain pending {:?} and target {:?}; retry only the exact request after inspecting the cause",
            self.parent, self.staging, self.target
        )
    }
}
impl std::error::Error for PendingBundle {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
impl BundleDestination {
    pub fn open(path: &Path) -> Result<Self> {
        let parent = OpenAt2Root::open(path, MountPolicy::AllowNestedMounts)?;
        trusted_parent(parent.directory())?;
        let lock = match ServiceLease::share_directory(&parent)? {
            Some(lease) => Exclusion::Admitted { _lease: lease },
            None => Exclusion::Independent {
                _lock: AdvisoryLock::directory(parent.directory())?,
            },
        };
        let result = Self {
            path: path.to_owned(),
            parent,
            _lock: lock,
        };
        result.recheck()?;
        Ok(result)
    }
    pub fn identity(&self) -> FileIdentity {
        self.parent.identity()
    }
    /// A partial record is evidence, not permission to invent a new baseline.
    pub fn operation_started(&self, name: &str) -> Result<bool> {
        self.recheck()?;
        let target = self.open_bundle(&EntryName::new(name)?)?;
        let pending = self.open_bundle(&EntryName::new(format!("{name}.pending"))?)?;
        if target.is_some() && pending.is_some() {
            return Err("both operation names exist; preserve the conflicting records".into());
        }
        Ok(target.is_some() || pending.is_some())
    }
    pub fn recheck(&self) -> Result<()> {
        let current = OpenAt2Root::open(&self.path, MountPolicy::AllowNestedMounts)?;
        if current.identity() != self.identity() {
            return Err(format!("deployment parent identity changed; inspect held device {} inode {} before retrying", self.identity().device, self.identity().inode).into());
        }
        trusted_parent(current.directory())
    }
    fn open_bundle(&self, name: &EntryName) -> Result<Option<OpenAt2Root>> {
        let directory = match self.parent.open_directory(&name.as_relative()) {
            Ok(file) => file,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        private_directory(&directory)?;
        Ok(Some(OpenAt2Root::from_directory(
            directory,
            MountPolicy::SameMount,
        )?))
    }
    /// Read one bounded private operation record from its final or deterministic
    /// pending directory. Only the requested entry is admitted; unknown staging
    /// material is preserved and refuses automatic continuation.
    pub fn read_operation(
        &self,
        name: &str,
        entry: &str,
        maximum: usize,
    ) -> Result<Option<Vec<u8>>> {
        self.recheck()?;
        let target = EntryName::new(name)?;
        let pending = EntryName::new(format!("{name}.pending"))?;
        let final_directory = self.open_bundle(&target)?;
        let pending_directory = self.open_bundle(&pending)?;
        let (named, directory) = match (final_directory, pending_directory) {
            (Some(_), Some(_)) => {
                return Err(
                    "both final and pending operation records exist; neither was modified".into(),
                );
            }
            (Some(directory), None) => (target, directory),
            (None, Some(directory)) => (pending, directory),
            (None, None) => return Ok(None),
        };
        check_entries(&directory, &[(entry, &[])])?;
        let entry = EntryName::new(entry)?;
        let file = match directory.open_file(&entry.as_relative(), false) {
            Ok(file) => file,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7777 != 0o600
            || metadata.len() > maximum as u64
        {
            return Err("operation record has unsafe ownership, permissions or size".into());
        }
        let identity = SingleLinkRequirement::check(&file)?;
        let mut bytes = Vec::new();
        (&file).take(maximum as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > maximum || bytes.len() as u64 != metadata.len() {
            return Err("operation record changed length or exceeded its budget".into());
        }
        directory.verify_entry_identity(&entry, identity)?;
        self.parent
            .verify_entry_identity(&named, directory.identity())?;
        self.recheck()?;
        Ok(Some(bytes))
    }
    /// Read-only verification of the exact expected bundle. Pending directories
    /// may lack expected files, but unknown or differing material is rejected.
    /// Published directories must be complete. No bytes or durability barriers
    /// are written, and the held parent/name identities are rechecked.
    pub fn inspect(&self, name: &str, files: &[(&str, &[u8])]) -> Result<BundleState> {
        self.recheck()?;
        let target = EntryName::new(name)?;
        let pending = EntryName::new(format!("{name}.pending"))?;
        let (named, directory, state) =
            match (self.open_bundle(&target)?, self.open_bundle(&pending)?) {
                (None, None) => return Ok(BundleState::Absent),
                (Some(directory), None) => (target, directory, BundleState::Published),
                (None, Some(directory)) => (pending, directory, BundleState::Pending),
                (Some(_), Some(_)) => {
                    return Err(
                        "both final and pending bundles exist; preserve the conflicting records"
                            .into(),
                    );
                }
            };
        check_entries(&directory, files)?;
        for (entry, bytes) in files {
            if verify_file(&directory, entry, bytes)?.is_none() && state == BundleState::Published {
                return Err("published bundle is missing a required original file; no material was reconstructed".into());
            }
        }
        self.parent
            .verify_entry_identity(&named, directory.identity())?;
        self.recheck()?;
        Ok(state)
    }
    pub fn publish(&self, name: &str, files: &[(&str, &[u8])]) -> Result<BundlePublication> {
        let target = EntryName::new(name)?;
        let staging_name = format!("{name}.pending");
        let staging = EntryName::new(&staging_name)?;
        self.publish_at(&target, &staging, files).map_err(|source| {
            Box::new(PendingBundle {
                parent: self.identity(),
                staging: staging_name,
                target: name.into(),
                source,
            }) as Box<dyn std::error::Error + Send + Sync>
        })
    }
    fn publish_at(
        &self,
        target: &EntryName,
        staging: &EntryName,
        files: &[(&str, &[u8])],
    ) -> Result<BundlePublication> {
        self.recheck()?;
        if let Some(existing) = self.open_bundle(target)? {
            if self.open_bundle(staging)?.is_some() {
                return Err(
                    "both final and pending bundle names exist; neither was removed or replaced"
                        .into(),
                );
            }
            check_entries(&existing, files)?;
            for (name, expected) in files {
                verify_file(&existing, name, expected)?
                    .ok_or("published bundle is missing a required file")?
                    .sync_all()?;
            }
            existing.directory().sync_all()?;
            self.parent
                .verify_entry_identity(target, existing.identity())?;
            self.parent.directory().sync_all()?;
            self.recheck()?;
            return Ok(BundlePublication::Existing);
        }
        let (pending, resumed) = match self.open_bundle(staging)? {
            Some(pending) => (pending, true),
            None => {
                mkdirat(
                    self.parent.directory(),
                    staging.as_os_str(),
                    Mode::from_raw_mode(0o700),
                )?;
                // A crash after mkdir keeps this exact name for the same-request retry.
                let directory = self.parent.open_directory(&staging.as_relative())?;
                directory.set_permissions(std::fs::Permissions::from_mode(0o700))?;
                private_directory(&directory)?;
                (
                    OpenAt2Root::from_directory(directory, MountPolicy::SameMount)?,
                    false,
                )
            }
        };
        // Re-sync an existing pending name as well: a prior mkdir reply may have
        // been interrupted before its parent became durable.
        self.parent.directory().sync_all()?;
        check_entries(&pending, files)?;
        // Inspect all existing files before completing any missing one. No
        // differing, extra, linked or unsafe file is overwritten or discarded.
        for (name, bytes) in files {
            let _ = verify_file(&pending, name, bytes)?;
        }
        for (name, bytes) in files {
            match verify_file(&pending, name, bytes)? {
                Some(file) => file.sync_all()?,
                None => write_atomic_at(&pending, &EntryName::new(*name)?, bytes, false)?,
            }
        }
        pending.directory().sync_all()?;
        check_entries(&pending, files)?;
        for (name, bytes) in files {
            verify_file(&pending, name, bytes)?
                .ok_or("pending bundle lost a required file before publication")?;
        }
        private_directory(pending.directory())?;
        self.parent
            .verify_entry_identity(staging, pending.identity())?;
        self.recheck()?;
        publish_directory(&self.parent, staging, target, &pending)?;
        self.recheck()?;
        Ok(if resumed {
            BundlePublication::Resumed
        } else {
            BundlePublication::Created
        })
    }
}
fn trusted_parent(file: &File) -> Result<()> {
    let metadata = file.metadata()?;
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o022 != 0 {
        return Err("deployment parent must already belong to this user and not be writable by another user or group".into());
    }
    Ok(())
}
fn private_directory(file: &File) -> Result<()> {
    let metadata = file.metadata()?;
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o7777 != 0o700 {
        return Err("deployment directory must be owned by this user with exact mode 0700".into());
    }
    Ok(())
}
fn check_entries(directory: &OpenAt2Root, files: &[(&str, &[u8])]) -> Result<()> {
    for entry in directory.entries()? {
        let entry = entry?;
        if !files
            .iter()
            .any(|(name, _)| entry.as_os_str() == std::ffi::OsStr::new(*name))
        {
            return Err(format!(
                "unexpected deployment bundle entry {:?}; retain it for explicit recovery",
                entry.as_os_str()
            )
            .into());
        }
    }
    Ok(())
}
fn verify_file(directory: &OpenAt2Root, name: &str, expected: &[u8]) -> Result<Option<File>> {
    let name = EntryName::new(name)?;
    let file = match directory.open_file(&name.as_relative(), false) {
        Ok(file) => file,
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.len() != expected.len() as u64
    {
        return Err(format!(
            "bundle file {:?} has unexpected owner, mode or length; it was not replaced",
            name.as_os_str()
        )
        .into());
    }
    let identity = SingleLinkRequirement::check(&file)?;
    let mut bytes = Vec::new();
    (&file)
        .take(expected.len() as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes != expected {
        return Err(format!(
            "bundle file {:?} differs from the exact request; it was not replaced",
            name.as_os_str()
        )
        .into());
    }
    directory.verify_entry_identity(&name, identity)?;
    Ok(Some(file))
}

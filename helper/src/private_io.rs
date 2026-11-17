//! Descriptor-anchored CLI files. Output parents may be public (for example
//! /tmp); files are owner-only and single-linked. Existing outputs are never
//! clobbered; draft replacements require a held transaction.
mod configuration_files;
mod fixed_program;
mod host_root;
mod program_repair;
mod rescue_image;
mod vfs_module;
use crate::Result;
pub(crate) use configuration_files::{
    ConfigurationFiles, ParentSnapshot, SAMBA_PATH, SYSTEMD_PATH,
};
pub(crate) use fixed_program::FixedProgramParents;
pub(crate) use host_root::HostRoot;
use platform_io::fs_safety::{
    AtomicCommitError, EntryName,
    linux::{AdvisoryLock, FileIdentity, MountPolicy, OpenAt2Root, SingleLinkRequirement},
};
pub(crate) use platform_io::publication::{BundleDestination, BundlePublication, BundleState};
use platform_io::publication::{
    write_atomic_at, write_atomic_at_mode, write_atomic_at_mode_receipt,
};
pub(crate) use program_repair::ProgramSnapshot;
pub(crate) use rescue_image::{publish_rescue_sqlite, rescue_digest, verify_rescue_sqlite};
use rustix::fs::{AtFlags, RenameFlags, renameat_with, statat};
use std::{ffi::OsStr, fs::File, io::Read, os::unix::fs::MetadataExt, path::Path};
pub(crate) use vfs_module::VfsDestination;

struct AnchoredPath {
    directory: OpenAt2Root,
    name: EntryName,
}
impl AnchoredPath {
    fn open(path: &Path) -> Result<Self> {
        let name = EntryName::new(path.file_name().ok_or("file name is required")?)?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        // Resolve the caller-selected parent once. All subsequent file opens,
        // publication and cleanup use the held directory descriptor.
        let parent = parent.canonicalize()?;
        Ok(Self {
            directory: OpenAt2Root::open(&parent, MountPolicy::AllowNestedMounts)?,
            name,
        })
    }
}

/// Read an explicitly selected owned draft without following links. Drafts
/// need not have secret-file permissions, but still use the regular-file and
/// single-link checks of the descriptor-anchored I/O layer.
impl AnchoredPath {
    fn read_owned_bytes(&self, maximum: usize) -> Result<Vec<u8>> {
        let file = self.directory.open_file(&self.name.as_relative(), false)?;
        let metadata = file.metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err("configuration draft must belong to this user".into());
        }
        if metadata.len() > maximum as u64 {
            return Err("configuration draft exceeds its byte limit".into());
        }
        let mut bytes = Vec::new();
        file.take((maximum as u64).saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() > maximum {
            return Err("configuration draft exceeds its byte limit".into());
        }
        Ok(bytes)
    }
}
/// Draft read/validate/backup/replace transaction. The stable parent lock
/// spans inode replacement and covers aliases and overlapping backup names.
/// External writers must remain stopped for the transaction.
pub(crate) struct DraftTransaction {
    target: AnchoredPath,
    _lock: DraftExclusion,
}
enum DraftExclusion {
    Independent {
        _lock: AdvisoryLock,
    },
    Admitted {
        _lease: platform_io::service_lease::ServiceLease,
    },
}
impl DraftTransaction {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let target = AnchoredPath::open(path)?;
        let lock = match platform_io::service_lease::ServiceLease::share_directory(&target.directory)? {
            Some(lease) => DraftExclusion::Admitted { _lease: lease },
            None => DraftExclusion::Independent {
                _lock: AdvisoryLock::directory(target.directory.directory()).map_err(|error| {
                    match error {
                        platform_io::fs_safety::Error::AlreadyLocked(_) => {
                            "another configuration draft edit is active in this directory; retry after it finishes".into()
                        }
                        other => Box::new(other) as Box<dyn std::error::Error + Send + Sync>,
                    }
                })?,
            },
        };
        Ok(Self {
            target,
            _lock: lock,
        })
    }

    pub(crate) fn read_owned_bytes(&self, maximum: usize) -> Result<Vec<u8>> {
        self.target.read_owned_bytes(maximum)
    }

    /// Inspect markers and backup names relative to the same held parent as
    /// the draft. Never recanonicalize a parent that could have been rebound.
    pub(crate) fn entry_exists(&self, name: &OsStr) -> Result<bool> {
        let name = EntryName::new(name)?;
        match statat(
            self.target.directory.directory(),
            name.as_os_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(_) => Ok(true),
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn write_backup(&self, name: &OsStr, original: &[u8]) -> Result<()> {
        let replace = self.entry_exists(name)?;
        write_atomic_at(
            &self.target.directory,
            &EntryName::new(name)?,
            original,
            replace,
        )
    }

    pub(crate) fn replace_private(&self, bytes: &[u8]) -> Result<()> {
        write_atomic_at(&self.target.directory, &self.target.name, bytes, true)
    }
}

/// Read a user-selected bounded, owned regular input without following its leaf.
pub(crate) fn read_owned(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    AnchoredPath::open(path)?.read_owned_bytes(maximum)
}

/// Publish only a complete 0600 file; existing files, directories and links win.
/// A directory-sync failure after rename preserves PublishedDurabilityUnknown.
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let target = AnchoredPath::open(path)?;
    write_atomic_at(&target.directory, &target.name, bytes, false)
}

#[cfg(test)]
#[path = "../../test/helper/unit/private_io/tests.rs"]
mod tests;

/// Program-only publication into an already trusted parent. Callers choose a
/// fixed installation path, not an arbitrary browser-supplied destination.
/// Existing nonmatching files, links, modes or owners are never replaced.
pub(crate) struct ProgramDestination {
    chain: Option<FixedProgramParents>,
    parent_path: std::path::PathBuf,
    target: AnchoredPath,
    _lock: AdvisoryLock,
}
/// In-memory proof only; the next invocation must open and admit its own
/// current executable. No persisted baseline or receipt fields are invented.
pub(crate) struct HeldProgram {
    file: File,
    identity: FileIdentity,
    stamp: crate::system_program::FileStamp,
}
impl ProgramDestination {
    pub(crate) fn installed() -> Result<Self> {
        Self::fixed(
            Path::new(crate::installation::DESTINATION),
            FixedProgramParents::executable()?,
        )
    }
    fn fixed(path: &Path, parents: FixedProgramParents) -> Result<Self> {
        if path.parent() != Some(parents.path()) {
            return Err("the fixed destination differs from its declared directory chain".into());
        }
        let mut result = Self::open(path)?;
        if result.parent_identity() != parents.leaf().identity() {
            return Err("fixed executable parent was replaced before it could be locked".into());
        }
        result.chain = Some(parents);
        result.recheck_parent()?;
        Ok(result)
    }
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let target = AnchoredPath::open(path)?;
        let metadata = target.directory.directory().metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o022 != 0 {
            return Err("program parent must belong to this user and not be writable by another user or group".into());
        }
        let lock = AdvisoryLock::directory(target.directory.directory())?;
        let parent_path = path
            .parent()
            .ok_or("program parent path is required")?
            .to_owned();
        let result = Self {
            chain: None,
            parent_path,
            target,
            _lock: lock,
        };
        result.recheck_parent()?;
        Ok(result)
    }
    fn recheck_parent(&self) -> Result<()> {
        if let Some(parents) = &self.chain {
            parents.require_current()?;
        }
        let current = OpenAt2Root::open(&self.parent_path, MountPolicy::AllowNestedMounts)?;
        if current.identity() != self.target.directory.identity() {
            return Err(format!("program parent path changed; retain and inspect publication under parent device {} inode {}", self.target.directory.identity().device, self.target.directory.identity().inode).into());
        }
        let metadata = current.directory().metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o022 != 0 {
            return Err(
                "program parent permissions changed; no successful installation is claimed".into(),
            );
        }
        Ok(())
    }
    pub(crate) fn parent_identity(&self) -> FileIdentity {
        self.target.directory.identity()
    }
    pub(crate) fn matches_or_absent(&self, expected: &[u8]) -> Result<bool> {
        Ok(self.matching_file(expected)?.is_some())
    }
    pub(crate) fn matching_file(&self, expected: &[u8]) -> Result<Option<HeldProgram>> {
        self.recheck_parent()?;
        let file = match self
            .target
            .directory
            .open_file(&self.target.name.as_relative(), false)
        {
            Ok(file) => file,
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7022 != 0
            || metadata.mode() & 0o555 != 0o555
            || metadata.len() != expected.len() as u64
        {
            return Err("existing program does not have the exact safe owner, executable mode and expected size; ordinary installation does not repair it; use explicit same-payload repair only with the original installation baseline".into());
        }
        let identity = SingleLinkRequirement::check(&file)?;
        let stamp = crate::system_program::FileStamp::of(&metadata);
        let mut input = &file;
        let mut scratch = [0u8; 32 * 1024];
        for chunk in expected.chunks(scratch.len()) {
            input.read_exact(&mut scratch[..chunk.len()])?;
            if scratch[..chunk.len()] != *chunk {
                return Err("existing program differs from the embedded payload; ordinary installation did not overwrite it; explicit same-payload repair requires the original installation baseline".into());
            }
        }
        if input.read(&mut scratch[..1])? != 0
            || crate::system_program::FileStamp::of(&file.metadata()?) != stamp
        {
            return Err(
                "executable changed during bounded payload verification; preserve both names"
                    .into(),
            );
        }
        self.target
            .directory
            .verify_entry_identity(&self.target.name, identity)?;
        self.recheck_parent()?;
        Ok(Some(HeldProgram {
            file,
            identity,
            stamp,
        }))
    }
    pub(crate) fn require_held(&self, held: &HeldProgram) -> Result<()> {
        self.recheck_parent()?;
        self.target
            .directory
            .verify_entry_identity(&self.target.name, held.identity)?;
        if crate::system_program::FileStamp::of(&held.file.metadata()?) != held.stamp {
            return Err(
                "held executable changed during publication; do not claim the current file".into(),
            );
        }
        Ok(())
    }
    pub(crate) fn install_captured(
        &self,
        bytes: &[u8],
        captured: Option<HeldProgram>,
        must_exist: bool,
    ) -> Result<(bool, HeldProgram)> {
        self.recheck_parent()?;
        if let Some(held) = captured {
            self.require_held(&held)?;
            held.file.sync_all()?;
            self.target.directory.directory().sync_all()?;
            self.require_held(&held)?;
            return Ok((false, held));
        }
        if must_exist {
            return Err("the recorded installed executable is missing; do not reinstall from a historical receipt".into());
        }
        let publication = write_atomic_at_mode_receipt(
            &self.target.directory,
            &self.target.name,
            bytes,
            false,
            0o555,
        )?;
        let held = self
            .matching_file(bytes)?
            .ok_or("new executable disappeared after publication")?;
        if held.identity != publication.identity() {
            return Err("new executable no longer has the original published inode".into());
        }
        self.require_held(&held)?;
        Ok((true, held))
    }
}

//! Exact bounded inventories for ordinary target-only native recovery files.
//! Saved descriptors never supply filesystem paths or a writer capability.
use super::*;
use crate::engine::backup_preparation::native_control::databases::materials::FileReference;
use crate::persist::layout;
use std::{collections::BTreeSet, ffi::OsStr, path::Path};

const SELECTED: &str = "backup-target-generation-selected.json";
const LOCAL_COMPLETE: &str = "backup-target-generation-installed.json";
const MAX_LOCAL: usize = 8 * 1024;
const MAX_FILES: usize = 65_536;
const MAX_MANIFEST: usize = 32 * 1024 * 1024;
const NAMESPACES: [&str; 4] = [
    "pool-membership",
    "membership-generations",
    "membership-pending",
    "membership-history",
];

pub(in super::super) fn is_namespace(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|value| NAMESPACES.contains(&value))
}

#[derive(Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
enum SelectionFormat {
    BackupTargetGeneration,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Selection {
    format: SelectionFormat,
    plan_hash: String,
    native_installation_hash: String,
    authority_hash: String,
    transition_hash: String,
    archive_hash: String,
    files_hash: String,
    physical: PhysicalBinding,
}

pub(super) struct SelectionInputs<'a> {
    pub plan_hash: &'a str,
    pub native_installation_hash: &'a str,
    pub authority_hash: &'a str,
    pub transition_hash: &'a str,
    pub archive_hash: &'a str,
    pub files_hash: &'a str,
}

pub(super) struct LocalSelection {
    bytes: Vec<u8>,
}

fn require_hash(hash: &str) -> Result<()> {
    hash.parse::<storage_contract::ContentHash>()
        .map(|_| ())
        .map_err(|source| {
            Error::from(std::io::Error::other(source)).context(
                ErrorCode::ObjectCorrupt,
                "Backup target locator contains an invalid native digest",
            )
        })
}

/// Layout ownership only. The full original task still reconstructs every
/// expected file and all actual proofs before any write or successful report.
pub(in super::super) fn require_staging(root: &DiskRoot, native: Option<&str>) -> Result<bool> {
    if !preparation_path_exists(root, &root.path().join(SELECTED))? {
        if preparation_path_exists(root, &root.path().join(LOCAL_COMPLETE))? {
            return Err(invalid(
                "Backup target locator completion lost its original selection",
            ));
        }
        return Ok(false);
    }
    let bytes = root.read_bytes(&root.path().join(SELECTED), MAX_LOCAL)?;
    let selected: Selection = crate::journal::decode_proof(&bytes)?;
    for hash in [
        &selected.plan_hash,
        &selected.native_installation_hash,
        &selected.authority_hash,
        &selected.transition_hash,
        &selected.archive_hash,
        &selected.files_hash,
    ] {
        require_hash(hash)?;
    }
    if native != Some(selected.native_installation_hash.as_str())
        || selected.physical != PhysicalBinding::of(root)?
        || canonical_json(&selected)? != bytes
    {
        return Err(invalid(
            "Backup target locator changed its exact native stage or physical root",
        ));
    }
    let seal = decode_seal(&root.read_entry_bytes(&layout::DISK_IDENTITY, MAX_METADATA)?)?;
    if !matches!(&seal.format, DiskSealFormat::MembershipGeneration { authority_hash }
        if authority_hash == &selected.authority_hash)
        || seal.device != selected.physical.device
        || seal.inode != selected.physical.inode
    {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "Backup target locator differs from the installed native seal",
        ));
    }
    if preparation_path_exists(root, &root.path().join(LOCAL_COMPLETE))?
        && root.read_bytes(&root.path().join(LOCAL_COMPLETE), MAX_LOCAL)? != bytes
    {
        return Err(invalid(
            "Backup target locator completion belongs to another local selection",
        ));
    }
    Ok(true)
}

impl LocalSelection {
    pub(super) fn capture(root: &DiskRoot, input: &SelectionInputs<'_>) -> Result<Self> {
        let bytes = canonical_json(&Selection {
            format: SelectionFormat::BackupTargetGeneration,
            plan_hash: input.plan_hash.into(),
            native_installation_hash: input.native_installation_hash.into(),
            authority_hash: input.authority_hash.into(),
            transition_hash: input.transition_hash.into(),
            archive_hash: input.archive_hash.into(),
            files_hash: input.files_hash.into(),
            physical: PhysicalBinding::of(root)?,
        })?;
        if bytes.len() > MAX_LOCAL {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "Backup local locator selection exceeds 8 KiB",
            ));
        }
        Ok(Self { bytes })
    }

    pub(super) fn preflight(&self, root: &DiskRoot) -> Result<()> {
        if preparation_path_exists(root, &root.path().join(SELECTED))? {
            self.require(root, SELECTED)?;
        } else {
            for namespace in NAMESPACES {
                if preparation_path_exists(root, &root.path().join("metadata").join(namespace))? {
                    return Err(Error::new(
                        ErrorCode::ObjectCollision,
                        "unowned Backup target native namespace retained without adoption",
                    ));
                }
            }
            if preparation_path_exists(root, &root.path().join(LOCAL_COMPLETE))? {
                return Err(invalid("Backup target locator lost its selected intent"));
            }
        }
        if self.completed(root)? {
            self.require(root, LOCAL_COMPLETE)?;
        }
        Ok(())
    }

    pub(super) fn select(&self, root: &DiskRoot) -> Result<()> {
        self.preflight(root)?;
        install_preparation_bytes(root, &root.path().join(SELECTED), &self.bytes)
    }
    pub(super) fn completed(&self, root: &DiskRoot) -> Result<bool> {
        preparation_path_exists(root, &root.path().join(LOCAL_COMPLETE))
    }
    pub(super) fn complete(&self, root: &DiskRoot) -> Result<()> {
        self.require(root, SELECTED)?;
        install_preparation_bytes(root, &root.path().join(LOCAL_COMPLETE), &self.bytes)
    }
    pub(super) fn require_complete(&self, root: &DiskRoot) -> Result<()> {
        self.require(root, SELECTED)?;
        self.require(root, LOCAL_COMPLETE)
    }
    fn require(&self, root: &DiskRoot, name: &str) -> Result<()> {
        if root.read_bytes(&root.path().join(name), MAX_LOCAL)? != self.bytes {
            return Err(invalid(
                "Backup target locator does not retain its exact original selection",
            ));
        }
        Ok(())
    }
}

pub(in super::super) struct Inventory {
    files: BTreeMap<PathBuf, FileReference>,
    directories: BTreeSet<PathBuf>,
    parents: BTreeSet<PathBuf>,
    pub hash: String,
    pub bytes: u64,
}

#[derive(Clone, Copy)]
pub(in super::super) enum Presence {
    Existing,
    Complete,
}

impl Inventory {
    pub(in super::super) fn capture<'a>(
        files: impl Iterator<Item = (PathBuf, &'a [u8])>,
    ) -> Result<Self> {
        let mut expected = BTreeMap::new();
        let mut directories = BTreeSet::new();
        let mut parents = BTreeSet::new();
        let mut bytes = 0u64;
        for (path, value) in files {
            let mut components = path.components();
            if components.next() != Some(std::path::Component::Normal(OsStr::new("metadata")))
                || components
                    .next()
                    .is_none_or(|part| !is_namespace(part.as_os_str()))
                || components
                    .clone()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
                || components.next().is_none()
            {
                return Err(invalid(
                    "generated Backup locator file is outside its fixed native namespaces",
                ));
            }
            bytes = bytes.checked_add(value.len() as u64).ok_or_else(limit)?;
            let parent = path
                .parent()
                .expect("invariant: generated locator has a native parent");
            parents.insert(parent.to_owned());
            for directory in path
                .ancestors()
                .skip(1)
                .take_while(|parent| *parent != Path::new("metadata"))
            {
                directories.insert(directory.to_owned());
            }
            if expected
                .insert(path, FileReference::from_bytes(value))
                .is_some()
                || expected.len() > MAX_FILES
                || bytes > 512 * 1024 * 1024
            {
                return Err(limit());
            }
        }
        if directories.iter().any(|path| expected.contains_key(path)) {
            return Err(invalid("Backup locator file overlaps a directory"));
        }
        let manifest = canonical_json(&expected)?;
        if manifest.len() > MAX_MANIFEST {
            return Err(limit());
        }
        Ok(Self {
            hash: blake3::hash(&manifest).to_hex().to_string(),
            files: expected,
            directories,
            parents,
            bytes,
        })
    }

    pub(in super::super) fn count(&self) -> usize {
        self.files.len()
    }

    pub(super) fn inspect(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        let scopes = NAMESPACES.map(|namespace| PathBuf::from("metadata").join(namespace));
        self.inspect_under(root, &scopes, &[], stop)
    }

    /// Only the sealed original recipe supplies these scope boundaries. The
    /// one permitted existing journal is observed separately, never overwritten.
    pub(in super::super) fn inspect_under(
        &self,
        root: &DiskRoot,
        scopes: &[PathBuf],
        existing: &[PathBuf],
        stop: &AtomicBool,
    ) -> Result<()> {
        let mut pending = Vec::new();
        for path in scopes {
            if preparation_path_exists(root, &root.path().join(path))? {
                if !self.directories.contains(path) {
                    return Err(Error::new(
                        ErrorCode::ObjectCollision,
                        "unexpected Backup native control scope retained",
                    ));
                }
                root.private_child(&root.path().join(path))?;
                pending.push(path.clone());
            }
        }
        let mut count = 0usize;
        while let Some(parent) = pending.pop() {
            for entry in root.entries(&root.path().join(&parent))? {
                preparation_checkpoint(stop)?;
                count += 1;
                if count > MAX_FILES * 4 {
                    return Err(limit());
                }
                let name = entry.map_err(fs_error)?;
                let path = parent.join(name.as_path());
                if self.directories.contains(&path) {
                    root.private_child(&root.path().join(&path))?;
                    pending.push(path);
                } else if self.files.contains_key(&path)
                    || existing.contains(&path)
                    || (self.parents.contains(&parent)
                        && platform_io::fs_safety::AtomicFile::is_temporary_name(&name))
                {
                    root.open_read_regular(&root.path().join(path))?;
                } else {
                    return Err(Error::new(
                        ErrorCode::ObjectCollision,
                        "unknown Backup native control file retained without adoption or deletion",
                    ));
                }
            }
        }
        Ok(())
    }

    pub(in super::super) fn require_absent_files(
        &self,
        root: &DiskRoot,
        stop: &AtomicBool,
    ) -> Result<()> {
        for path in self.files.keys() {
            preparation_checkpoint(stop)?;
            if preparation_path_exists(root, &root.path().join(path))? {
                return Err(Error::new(
                    ErrorCode::ObjectCollision,
                    "unselected original Backup control file cannot be adopted even when bytes match",
                ));
            }
        }
        Ok(())
    }

    pub(in super::super) fn require_capacity(&self, root: &DiskRoot, reserve: u64) -> Result<()> {
        let mut missing = 0u64;
        let mut largest = 0u64;
        let mut count = 0u64;
        for (path, expected) in &self.files {
            if !preparation_path_exists(root, &root.path().join(path))? {
                missing = missing
                    .checked_add(expected.bytes() as u64)
                    .ok_or_else(limit)?;
                largest = largest.max(expected.bytes() as u64);
                count += 1;
            }
        }
        let required = missing
            .checked_add(largest)
            .and_then(|n| n.checked_add(reserve))
            .and_then(|n| n.checked_add(count * 8192 + 1024 * 1024))
            .ok_or_else(limit)?;
        if root.available()? < required {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "each Backup participant needs a complete independent native control copy, atomic staging and original reserve",
            ));
        }
        Ok(())
    }

    pub(super) fn verify_present(
        &self,
        root: &DiskRoot,
        presence: Presence,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.inspect(root, stop)?;
        self.verify_files(root, presence, stop)
    }

    pub(in super::super) fn verify_files(
        &self,
        root: &DiskRoot,
        presence: Presence,
        stop: &AtomicBool,
    ) -> Result<()> {
        for (path, expected) in &self.files {
            preparation_checkpoint(stop)?;
            let path = root.path().join(path);
            if matches!(presence, Presence::Existing) && !preparation_path_exists(root, &path)? {
                continue;
            }
            let actual = root.read_bytes(&path, expected.bytes())?;
            if !expected.matches(&actual) {
                return Err(Error::new(
                    ErrorCode::ObjectCollision,
                    "Backup native control files differ from their exact original recipe",
                ));
            }
        }
        Ok(())
    }
}

fn limit() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "Backup target locator exceeds 65536 files, 512 MiB or its 32 MiB descriptor bound",
    )
}

#[cfg(test)]
#[path = "../../../../../../../../../../../test/engine/unit/backup_target_locator_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../../../../../../../test/engine/unit/backup_original_control_tests.rs"]
mod original_control_tests;

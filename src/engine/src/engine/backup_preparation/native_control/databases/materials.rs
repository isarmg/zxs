//! Immutable preparation names. No active SQLite/namespace is replaced and no
//! stored database opens by pathname through the SQLite library.
use super::*;
use crate::history_package::{
    install_preparation_bytes, preparation_checkpoint, preparation_path_exists,
};
use data_serialization::Serialize;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// A borrowed root record recipe. Reading it does not admit its related data
/// or establish that any other participant completed the publication stage.
pub(super) struct RootRecord<'a> {
    pub(super) name: &'a str,
    pub(super) bytes: &'a [u8],
    pub(super) maximum: usize,
}
impl RootRecord<'_> {
    pub(super) fn check_existing(&self, root: &DiskRoot) -> Result<()> {
        let path = root.path().join(self.name);
        if preparation_path_exists(root, &path)? {
            self.compare(root, &path, ErrorCode::ObjectCollision)?;
        }
        Ok(())
    }
    pub(super) fn require(&self, root: &DiskRoot) -> Result<()> {
        self.compare(
            root,
            &root.path().join(self.name),
            ErrorCode::ConfigConflict,
        )
    }
    fn compare(&self, root: &DiskRoot, path: &std::path::Path, code: ErrorCode) -> Result<()> {
        if root.read_bytes(path, self.maximum)? != self.bytes {
            return Err(Error::new(
                code,
                format!(
                    "Backup record {} differs from the expected bytes; existing record retained",
                    self.name
                ),
            ));
        }
        Ok(())
    }
    pub(super) fn install(&self, root: &DiskRoot) -> Result<()> {
        install_preparation_bytes(root, &root.path().join(self.name), self.bytes)
    }
}

#[derive(Clone, Copy)]
pub(super) enum Namespace {
    Objects,
    Scopes,
    BackupGeneration,
    GenerationControl,
    GenerationAuthority,
    GenerationFirstCandidate,
    GenerationFirstPublication,
    PublishedRegistry,
    PublishedScopes,
    CommonDatabases,
}
impl Namespace {
    fn name(self) -> &'static str {
        match self {
            Self::Objects => "object-databases",
            Self::Scopes => "scoped-databases",
            Self::BackupGeneration => "backup-generation-inputs",
            Self::GenerationControl => "backup-generation-control",
            Self::GenerationAuthority => "backup-generation-authority",
            Self::GenerationFirstCandidate => "backup-generation-first-candidate",
            Self::GenerationFirstPublication => "backup-generation-first-publication",
            Self::PublishedRegistry => "backup-published-registry",
            Self::PublishedScopes => "published-scoped-databases",
            Self::CommonDatabases => "backup-common-databases",
        }
    }
}

#[derive(Serialize)]
pub(super) struct FileReference {
    bytes: usize,
    digest: String,
}
impl FileReference {
    pub(super) fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.len(),
            digest: blake3::hash(bytes).to_hex().to_string(),
        }
    }
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }
    pub(super) fn matches(&self, bytes: &[u8]) -> bool {
        bytes.len() == self.bytes && blake3::hash(bytes).to_hex().as_str() == self.digest
    }
}

enum Entry<'a> {
    Directory,
    File(Cow<'a, [u8]>),
}

pub(super) struct PreparedFiles<'a> {
    namespace: Namespace,
    entries: BTreeMap<PathBuf, Entry<'a>>,
}
impl<'a> PreparedFiles<'a> {
    pub(super) fn new(
        namespace: Namespace,
        manifest: impl Into<Cow<'a, [u8]>>,
        files: impl IntoIterator<Item = (PathBuf, &'a [u8])>,
    ) -> Result<Self> {
        let mut entries = BTreeMap::from([(PathBuf::from(MANIFEST), Entry::File(manifest.into()))]);
        for (path, bytes) in files {
            if path.as_os_str().is_empty()
                || path
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
                || entries
                    .insert(path.clone(), Entry::File(Cow::Borrowed(bytes)))
                    .is_some()
            {
                return Err(invalid(
                    "prepared database file names overlap or escape their scope",
                ));
            }
            for parent in path
                .ancestors()
                .skip(1)
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                match entries.entry(parent.to_owned()) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(Entry::Directory);
                    }
                    std::collections::btree_map::Entry::Occupied(entry) => {
                        if matches!(entry.get(), Entry::File(_)) {
                            return Err(invalid("prepared database and directory names overlap"));
                        }
                    }
                }
            }
        }
        Ok(Self { namespace, entries })
    }
    fn manifest(&self) -> &[u8] {
        let Entry::File(bytes) = &self.entries[Path::new(MANIFEST)] else {
            unreachable!("invariant: preparation manifest is an immutable file");
        };
        bytes
    }
    fn data_files(&self) -> impl Iterator<Item = (&Path, &[u8])> {
        self.entries.iter().filter_map(|(path, entry)| match entry {
            Entry::Directory => None,
            Entry::File(bytes) => {
                (path != Path::new(MANIFEST)).then_some((path.as_path(), bytes.as_ref()))
            }
        })
    }
}
const MANIFEST: &str = "manifest.json";

fn inventory(
    root: &DiskRoot,
    prepared: &PreparedFiles<'_>,
    complete: bool,
    stop: &AtomicBool,
) -> Result<()> {
    let path = root.path().join(prepared.namespace.name());
    if !preparation_path_exists(root, &path)? {
        return if complete {
            Err(invalid(
                "prepared database preparation directory is missing",
            ))
        } else {
            Ok(())
        };
    }
    let held = root.private_child(&path)?;
    let expected = &prepared.entries;
    let mut seen = BTreeSet::new();
    let mut retained_staging = 0usize;
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        preparation_checkpoint(stop)?;
        for entry in held.entries(&held.path().join(&relative))? {
            preparation_checkpoint(stop)?;
            let name = entry.map_err(fs_error)?;
            if matches!(
                prepared.namespace,
                Namespace::GenerationControl
                    | Namespace::GenerationAuthority
                    | Namespace::GenerationFirstCandidate
                    | Namespace::GenerationFirstPublication
                    | Namespace::PublishedRegistry
                    | Namespace::PublishedScopes
                    | Namespace::CommonDatabases
            ) && platform_io::fs_safety::AtomicFile::is_temporary_name(&name)
            {
                // Preserve interrupted private atomic writes, never promote,
                // count as evidence, overwrite or remove them. Bound hostile
                // staging inventories independently of the exact material set.
                retained_staging += 1;
                if retained_staging > 1024 {
                    return Err(Error::new(
                        ErrorCode::ResourceLimit,
                        "native control preparation has too many retained atomic staging files",
                    ));
                }
                held.open_read_regular(&held.path().join(&relative).join(name.as_path()))?;
                continue;
            }
            let child = relative.join(name.as_path());
            let entry = expected.get(&child).ok_or_else(|| {
                Error::new(
                    ErrorCode::ObjectCollision,
                    "unknown prepared database material is preserved, not adopted or removed",
                )
            })?;
            if !seen.insert(child.clone()) {
                return Err(invalid("repeated prepared database directory entry"));
            }
            match entry {
                Entry::File(_) => {
                    held.open_regular(&held.path().join(&child))?;
                }
                Entry::Directory => {
                    held.private_child(&held.path().join(&child))?;
                    pending.push(child);
                }
            }
        }
    }
    if complete && seen.len() != expected.len() {
        return Err(invalid(
            "prepared database material is missing part of the complete unit",
        ));
    }
    Ok(())
}

pub(super) fn required_capacity(
    root: &DiskRoot,
    prepared: &PreparedFiles<'_>,
    stop: &AtomicBool,
) -> Result<u64> {
    inventory(root, prepared, false, stop)?;
    let base = root.path().join(prepared.namespace.name());
    let manifest = prepared.manifest();
    let mut missing = 0u64;
    for (relative, bytes) in
        std::iter::once((Path::new(MANIFEST), manifest)).chain(prepared.data_files())
    {
        preparation_checkpoint(stop)?;
        let path = base.join(relative);
        if preparation_path_exists(root, &path)? {
            if root.read_bytes(&path, bytes.len())? != bytes {
                return Err(Error::new(
                    ErrorCode::ObjectCollision,
                    "retained prepared database differs; no conflicting bytes were replaced",
                ));
            }
        } else {
            missing = missing
                .checked_add(bytes.len() as u64)
                .ok_or_else(capacity_overflow)?;
        }
    }
    // Directory blocks, atomic file temporaries and completion intents have a
    // separate reservation. Peers never subsidize one another's free space.
    let overhead = (prepared.entries.len() as u64)
        .checked_mul(8192)
        .ok_or_else(capacity_overflow)?;
    let needed = missing
        .checked_add(overhead)
        .and_then(|bytes| bytes.checked_add(RESERVE))
        .ok_or_else(capacity_overflow)?;
    Ok(needed)
}

pub(super) fn require_capacity(
    root: &DiskRoot,
    prepared: &PreparedFiles<'_>,
    stop: &AtomicBool,
) -> Result<()> {
    if root.available()? < required_capacity(root, prepared, stop)? {
        return Err(Error::new(
            ErrorCode::NoSpace,
            "one selected target cannot retain all prepared databases",
        ));
    }
    Ok(())
}
fn capacity_overflow() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "prepared database reservation overflow",
    )
}

pub(super) fn install(
    root: &DiskRoot,
    prepared: &PreparedFiles<'_>,
    stop: &AtomicBool,
) -> Result<()> {
    require_capacity(root, prepared, stop)?;
    let base = root.path().join(prepared.namespace.name());
    install_preparation_bytes(root, &base.join(MANIFEST), prepared.manifest())?;
    for (path, bytes) in prepared.data_files() {
        preparation_checkpoint(stop)?;
        install_preparation_bytes(root, &base.join(path), bytes)?;
    }
    verify(root, prepared, stop)
}

pub(super) fn verify(
    root: &DiskRoot,
    prepared: &PreparedFiles<'_>,
    stop: &AtomicBool,
) -> Result<()> {
    inventory(root, prepared, true, stop)?;
    let base = root.path().join(prepared.namespace.name());
    let manifest = prepared.manifest();
    if root.read_bytes(&base.join(MANIFEST), manifest.len())? != manifest {
        return Err(invalid(
            "database manifest does not equal the admitted complete State projection",
        ));
    }
    for (path, bytes) in prepared.data_files() {
        preparation_checkpoint(stop)?;
        if root.read_bytes(&base.join(path), bytes.len())? != bytes {
            return Err(invalid(
                "database bytes differ from their admitted source projection",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../../test/engine/unit/backup_material_record_tests.rs"]
mod tests;

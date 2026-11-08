//! Immutable preparation names. No active SQLite/namespace is replaced and no
//! stored database opens by pathname through the SQLite library.
use super::*;
use crate::history_package::{
    install_preparation_bytes, preparation_checkpoint, preparation_path_exists,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub(super) enum Namespace {
    Objects,
    Scopes,
    BackupGeneration,
    GenerationControl,
    GenerationAuthority,
}
impl Namespace {
    fn name(self) -> &'static str {
        match self {
            Self::Objects => "object-databases",
            Self::Scopes => "scoped-databases",
            Self::BackupGeneration => "backup-generation-inputs",
            Self::GenerationControl => "backup-generation-control",
            Self::GenerationAuthority => "backup-generation-authority",
        }
    }
}

pub(super) struct PreparedFiles<'a> {
    namespace: Namespace,
    files: BTreeMap<PathBuf, &'a [u8]>,
    manifest: Vec<u8>,
}
impl<'a> PreparedFiles<'a> {
    pub(super) fn new(
        namespace: Namespace,
        manifest: Vec<u8>,
        files: impl IntoIterator<Item = (PathBuf, &'a [u8])>,
    ) -> Result<Self> {
        let mut exact = BTreeMap::new();
        for (path, bytes) in files {
            if path.as_os_str().is_empty()
                || path
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
                || path == PathBuf::from(MANIFEST)
                || exact.insert(path, bytes).is_some()
            {
                return Err(invalid(
                    "prepared database file names overlap or escape their scope",
                ));
            }
        }
        Ok(Self {
            namespace,
            files: exact,
            manifest,
        })
    }
    fn manifest(&self) -> &[u8] {
        &self.manifest
    }
}
const MANIFEST: &str = "manifest.json";

fn entries(prepared: &PreparedFiles<'_>) -> Result<BTreeMap<PathBuf, bool>> {
    // false = directory, true = immutable file. Generated paths have typed,
    // admitted IDs only. Deserialized descriptors never drive filesystem I/O.
    let mut expected = BTreeMap::from([(PathBuf::from(MANIFEST), true)]);
    for path in prepared.files.keys() {
        let path = path.clone();
        if expected.insert(path.clone(), true).is_some() {
            return Err(invalid("prepared database plan repeats a physical name"));
        }
        for parent in path
            .ancestors()
            .skip(1)
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            if expected.insert(parent.to_owned(), false) == Some(true) {
                return Err(invalid("prepared database and directory names overlap"));
            }
        }
    }
    Ok(expected)
}

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
    let expected = entries(prepared)?;
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
                Namespace::GenerationControl | Namespace::GenerationAuthority
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
            let child = relative.join(name);
            let file = *expected.get(&child).ok_or_else(|| {
                Error::new(
                    ErrorCode::ObjectCollision,
                    "unknown prepared database material is preserved, not adopted or removed",
                )
            })?;
            if !seen.insert(child.clone()) {
                return Err(invalid("repeated prepared database directory entry"));
            }
            if file {
                held.open_regular(&held.path().join(&child))?;
            } else {
                held.private_child(&held.path().join(&child))?;
                pending.push(child);
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

pub(super) fn require_capacity(
    root: &DiskRoot,
    prepared: &PreparedFiles<'_>,
    stop: &AtomicBool,
) -> Result<()> {
    inventory(root, prepared, false, stop)?;
    let base = root.path().join(prepared.namespace.name());
    let manifest = prepared.manifest();
    let mut missing = 0u64;
    for (relative, bytes) in std::iter::once((PathBuf::from(MANIFEST), manifest)).chain(
        prepared
            .files
            .iter()
            .map(|(path, bytes)| (path.clone(), *bytes)),
    ) {
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
    let overhead = (entries(prepared)?.len() as u64)
        .checked_mul(8192)
        .ok_or_else(capacity_overflow)?;
    let needed = missing
        .checked_add(overhead)
        .and_then(|bytes| bytes.checked_add(RESERVE))
        .ok_or_else(capacity_overflow)?;
    if root.available()? < needed {
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
    for (path, bytes) in &prepared.files {
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
    for (path, bytes) in &prepared.files {
        preparation_checkpoint(stop)?;
        if root.read_bytes(&base.join(path), bytes.len())? != *bytes {
            return Err(invalid(
                "database bytes differ from their admitted source projection",
            ));
        }
    }
    Ok(())
}

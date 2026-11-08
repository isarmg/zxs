//! Immutable preparation names. No active SQLite/namespace is replaced and no
//! stored database opens by pathname through the SQLite library.
use super::*;
use crate::history_package::{
    install_preparation_bytes, preparation_checkpoint, preparation_path_exists,
};
use std::collections::{BTreeMap, BTreeSet};

const ROOT: &str = "object-databases";
const MANIFEST: &str = "manifest.json";

fn entries(prepared: &PreparedDatabases) -> Result<BTreeMap<PathBuf, bool>> {
    // false = directory, true = immutable file. Generated paths have typed,
    // admitted IDs only. Deserialized descriptors never drive filesystem I/O.
    let mut expected = BTreeMap::from([(PathBuf::from(MANIFEST), true)]);
    for database in prepared.databases() {
        let path = database.path();
        if expected.insert(path.clone(), true).is_some() {
            return Err(invalid("object database plan repeats a physical name"));
        }
        for parent in path
            .ancestors()
            .skip(1)
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            if expected.insert(parent.to_owned(), false) == Some(true) {
                return Err(invalid("object database and directory names overlap"));
            }
        }
    }
    Ok(expected)
}

fn inventory(
    root: &DiskRoot,
    prepared: &PreparedDatabases,
    complete: bool,
    stop: &AtomicBool,
) -> Result<()> {
    let path = root.path().join(ROOT);
    if !preparation_path_exists(root, &path)? {
        return if complete {
            Err(invalid("object database preparation directory is missing"))
        } else {
            Ok(())
        };
    }
    let held = root.private_child(&path)?;
    let expected = entries(prepared)?;
    let mut seen = BTreeSet::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        preparation_checkpoint(stop)?;
        for entry in held.entries(&held.path().join(&relative))? {
            preparation_checkpoint(stop)?;
            let name = entry.map_err(fs_error)?;
            let child = relative.join(name);
            let file = *expected.get(&child).ok_or_else(|| {
                Error::new(
                    ErrorCode::ObjectCollision,
                    "unknown object database material is preserved, not adopted or removed",
                )
            })?;
            if !seen.insert(child.clone()) {
                return Err(invalid("repeated object database directory entry"));
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
            "object database material is missing part of the complete unit",
        ));
    }
    Ok(())
}

pub(super) fn require_capacity(
    root: &DiskRoot,
    prepared: &PreparedDatabases,
    stop: &AtomicBool,
) -> Result<()> {
    inventory(root, prepared, false, stop)?;
    let base = root.path().join(ROOT);
    let manifest = prepared.manifest()?;
    let mut missing = 0u64;
    for (relative, bytes) in std::iter::once((PathBuf::from(MANIFEST), manifest.as_slice())).chain(
        prepared
            .databases()
            .iter()
            .map(|db| (db.path(), db.bytes())),
    ) {
        preparation_checkpoint(stop)?;
        let path = base.join(relative);
        if preparation_path_exists(root, &path)? {
            if root.read_bytes(&path, bytes.len())? != bytes {
                return Err(Error::new(
                    ErrorCode::ObjectCollision,
                    "retained object database differs; no conflicting bytes were replaced",
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
            "one selected target cannot retain all object databases",
        ));
    }
    Ok(())
}
fn capacity_overflow() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "object database reservation overflow",
    )
}

pub(super) fn install(
    root: &DiskRoot,
    prepared: &PreparedDatabases,
    stop: &AtomicBool,
) -> Result<()> {
    require_capacity(root, prepared, stop)?;
    let base = root.path().join(ROOT);
    install_preparation_bytes(root, &base.join(MANIFEST), &prepared.manifest()?)?;
    for database in prepared.databases() {
        preparation_checkpoint(stop)?;
        install_preparation_bytes(root, &base.join(database.path()), database.bytes())?;
    }
    verify(root, prepared, stop)
}

pub(super) fn verify(
    root: &DiskRoot,
    prepared: &PreparedDatabases,
    stop: &AtomicBool,
) -> Result<()> {
    inventory(root, prepared, true, stop)?;
    let base = root.path().join(ROOT);
    let manifest = prepared.manifest()?;
    if root.read_bytes(&base.join(MANIFEST), manifest.len())? != manifest {
        return Err(invalid(
            "database manifest does not equal the admitted complete State projection",
        ));
    }
    for database in prepared.databases() {
        preparation_checkpoint(stop)?;
        database.verify(&root.read_bytes(&base.join(database.path()), database.bytes().len())?)?;
    }
    Ok(())
}

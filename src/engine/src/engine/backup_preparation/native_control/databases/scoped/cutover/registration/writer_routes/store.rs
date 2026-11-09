//! A closed, no-clobber namespace below each held isolated target. Missing
//! suffixes can be continued by the original task; different bytes stay put.
use super::*;

const DIRECTORY: &str = "backup-writer-routes";
const ROUTES: &str = "routes.json";
const COMPLETE: &str = "prepared.json";

fn inventory(root: &DiskRoot) -> Result<()> {
    let path = root.path().join(DIRECTORY);
    if !preparation_path_exists(root, &path)? {
        return Ok(());
    }
    let held = root.private_child(&path)?;
    for entry in held.entries(held.path())? {
        let name = entry.map_err(fs_error)?;
        if !matches!(name.as_path().to_str(), Some(ROUTES | COMPLETE)) {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "unknown writer-route material retained; no cleanup or adoption",
            ));
        }
        held.open_regular(&held.path().join(name.as_path()))?;
    }
    Ok(())
}

fn capacity(root: &DiskRoot, routes: &Routes, stop: &AtomicBool) -> Result<()> {
    inventory(root)?;
    // Both files, their atomic temporaries and directory metadata are reserved
    // separately on each target. No other disk's capacity fills this obligation.
    let mut needed = RESERVE + 3 * 8192;
    for (name, bytes) in [(ROUTES, &routes.bytes), (COMPLETE, &routes.completion)] {
        preparation_checkpoint(stop)?;
        let path = root.path().join(DIRECTORY).join(name);
        if preparation_path_exists(root, &path)? {
            if root.read_bytes(&path, bytes.len())? != *bytes {
                return Err(Error::new(
                    ErrorCode::ObjectCollision,
                    "writer-route task or bytes changed; original material preserved",
                ));
            }
        } else {
            needed = needed
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| invalid("writer-route reservation overflow"))?;
        }
    }
    if root.available()? < needed {
        return Err(Error::new(
            ErrorCode::NoSpace,
            "a selected target cannot retain its complete writer-route material",
        ));
    }
    Ok(())
}

fn read_back(
    prepared: &PreparedBackupRegistry,
    name: &str,
    expected: &[u8],
    stop: &AtomicBool,
) -> Result<()> {
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        inventory(&target.root)?;
        if target.root.read_bytes(
            &target.root.path().join(DIRECTORY).join(name),
            expected.len(),
        )? != expected
        {
            return Err(invalid(
                "writer-route material differs from the complete selected images",
            ));
        }
    }
    prepared.recheck_names()
}

fn install(
    prepared: &PreparedBackupRegistry,
    name: &str,
    bytes: &[u8],
    stop: &AtomicBool,
) -> Result<()> {
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        recovery::require_isolated(&prepared.targets)?;
        prepared.recheck_names()?;
        inventory(&target.root)?;
        install_preparation_bytes(
            &target.root,
            &target.root.path().join(DIRECTORY).join(name),
            bytes,
        )?;
    }
    read_back(prepared, name, bytes, stop)
}

pub(super) fn prepare(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    routes: &Routes,
    stop: &AtomicBool,
) -> Result<()> {
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        capacity(&target.root, routes, stop)?;
    }
    install(prepared, ROUTES, &routes.bytes, stop)?;
    // A crash may leave only routes, or completion on a subset. Neither case
    // grants authority. Every retry re-admits all image/decision inputs before
    // it can fill the missing copies of this same deterministic material.
    decision.verify_targets(prepared, stop)?;
    recovery::require_isolated(&prepared.targets)?;
    read_back(prepared, ROUTES, &routes.bytes, stop)?;
    install(prepared, COMPLETE, &routes.completion, stop)?;
    verify(prepared, routes, stop)?;
    recovery::require_isolated(&prepared.targets)
}

pub(super) fn verify(
    prepared: &PreparedBackupRegistry,
    routes: &Routes,
    stop: &AtomicBool,
) -> Result<()> {
    read_back(prepared, ROUTES, &routes.bytes, stop)?;
    read_back(prepared, COMPLETE, &routes.completion, stop)
}

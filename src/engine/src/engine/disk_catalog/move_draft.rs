//! Only the private native move draft can reach this unaccepted disk encoder.
//! Bytes stay under the move's immutable staging manifest, never the root alias.
use super::*;
use crate::disk::DiskRoot;

pub(in crate::engine) fn prepare_move_current(
    native: &crate::business::NativeMoveDraft,
    identity: DiskRegistrationIdentity,
) -> Result<Vec<u8>> {
    let proposed = snapshot(native.proposed_state(), identity)?;
    let hash = storage_contract::object_hash("disk-current-catalog", &proposed)?;
    let bytes = database::encode(&proposed, &hash)?;
    if database::read(&bytes, &proposed.identity)? != proposed {
        return Err(invalid(
            "unaccepted move disk database changed its exact proposed scope",
        ));
    }
    Ok(bytes)
}

/// Check the candidate outcome against the same scoped current image. Only
/// SYSTEM receipt status changes; no disk inventory can silently drift.
pub(in crate::engine) fn require_same_move_current(
    native: &crate::business::NativeMoveDraft,
    candidate: &crate::business::NativeMoveCandidate,
    identity: DiskRegistrationIdentity,
) -> Result<()> {
    if snapshot(native.proposed_state(), identity.clone())?
        != snapshot(candidate.state(), identity)?
    {
        return Err(invalid(
            "native move candidate changes its original scoped disk inventory",
        ));
    }
    Ok(())
}

/// Prepare the common preserving installer from the private move candidate.
/// This freezes only current inventory: a disk-history row still requires the
/// real joint Publication and cannot be invented from a prospective result.
pub(in crate::engine) fn prepare_move_current_install(
    root: &DiskRoot,
    candidate: &crate::business::NativeMoveCandidate,
    identity: &DiskRegistrationIdentity,
    published_sequence: u64,
) -> Result<PreparedInstall> {
    root_install::require_unselected(root)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        if root
            .optional_metadata(&root.path().join(format!("{DISK_DATABASE_NAME}{suffix}")))?
            .is_some()
        {
            return Err(invalid(
                "move current database has an unknown SQLite sidecar; retain all files",
            ));
        }
    }
    let proposed = snapshot(candidate.state(), identity.clone())?;
    let hash = storage_contract::object_hash("disk-current-catalog", &proposed)?;
    let bytes = database::encode(&proposed, &hash)?;
    if database::read(&bytes, identity)? != proposed {
        return Err(invalid(
            "move current installation differs from its exact candidate inventory",
        ));
    }
    let inode = root.directory_identity(root.path())?;
    PreparedInstall::capture(
        root,
        root.path(),
        (root.path(), inode),
        (
            Scope::disk(identity.clone()),
            InstallAdmission::NewOperation,
        ),
        bytes,
        MAX_DATABASE_BYTES,
        |old| {
            if database::read(old, identity)?.source_sequence > published_sequence {
                return Err(invalid(
                    "unpublished move cannot adopt a future current database",
                ));
            }
            Ok(())
        },
    )
}

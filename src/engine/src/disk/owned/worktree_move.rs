//! Physical MOVE for an externally decided sparse-file publication.
//! Both parents remain held and revocable. No copies, overwrite, unlink, rollback
//! or content-based adoption are permitted; recovery uses the original inode.
use super::{DiskRoot, RootOperation};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{EntryName, linux::FileIdentity};
use platform_io::payload::{Expectation, Receipt};
use std::{fs::File, io};
use storage_contract::ContentHash;
use storage_contract::working_tree::LocalEntityIdentity;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct FrozenFileMove {
    #[serialization(with = "storage_contract::u64_string")]
    pub(crate) bytes: u64,
    pub(crate) content_hash: ContentHash,
    pub(crate) entity: LocalEntityIdentity,
}

#[derive(Debug, error_definition::Error)]
pub(crate) enum FileMoveFault {
    #[error("file move disk admission failed")]
    Gate(#[source] crate::Error),
    #[error("file move payload verification failed")]
    Payload(#[source] platform_io::payload::Error),
    #[error("file move inode observation failed")]
    Io(#[source] io::Error),
    #[error("file move payload synchronization failed")]
    PayloadSync(#[source] io::Error),
    #[error("file move crosses device or disk-admission domains")]
    Domain,
    #[error("file move names do not identify the independently frozen inode")]
    Identity,
    #[error("file move source and destination are both occupied")]
    Occupied,
    #[error("file move source and destination are both absent")]
    Missing,
    #[error("fresh file move cannot adopt an already occupied destination")]
    ExistingDestination,
    #[error("attempted file move did not leave the frozen inode at the destination")]
    UnexpectedLocation,
    #[error("file move rename or durability tail failed")]
    Rename {
        #[source]
        source: io::Error,
        destination_sync: Option<io::Error>,
        source_sync: Option<io::Error>,
    },
    #[error("file move durability barriers failed")]
    Sync {
        #[source]
        first: io::Error,
        second: Option<io::Error>,
    },
}
#[derive(Debug, error_definition::Error)]
pub(crate) enum FileMoveError {
    #[error("file move was rejected before attempting a rename")]
    Rejected(#[source] FileMoveFault),
    #[error("file move was attempted; preserve both locations and the original decision")]
    Attempted(#[source] FileMoveFault),
    #[error("decided file move recovery remains unresolved")]
    Recovery(#[source] FileMoveFault),
}

struct HeldPayload {
    file: File,
    receipt: Receipt,
}
enum Location {
    Source(HeldPayload),
    Destination(HeldPayload),
}

impl DiskRoot {
    /// The owner has durably decided both parent bindings, names and expectation,
    /// and holds the complete source/destination namespace and file-writer fence.
    /// An already present destination is never adopted, even if it is exact.
    pub(crate) fn move_new_entry_verified_synced(
        &self,
        from: &EntryName,
        target: &DiskRoot,
        to: &EntryName,
        expected: FrozenFileMove,
    ) -> Result<Receipt, FileMoveError> {
        let (source, destination, location) = admit_move(self, from, target, to, expected)
            .map_err(FileMoveError::Rejected)?;
        match location {
            Location::Source(held) => move_held(&source, from, &destination, to, expected, held),
            Location::Destination(_) => {
                Err(FileMoveError::Rejected(FileMoveFault::ExistingDestination))
            }
        }
    }

    /// Call only with the exact externally admitted permanent move decision.
    /// Matching bytes with a different inode are not recovery evidence. A missing
    /// source alone is not success, absence of a container, or permission to copy.
    pub(crate) fn resume_decided_file_move(
        &self,
        from: &EntryName,
        target: &DiskRoot,
        to: &EntryName,
        expected: FrozenFileMove,
    ) -> Result<Receipt, FileMoveError> {
        let (source, destination, location) = admit_move(self, from, target, to, expected)
            .map_err(FileMoveError::Recovery)?;
        match location {
            Location::Source(held) => move_held(&source, from, &destination, to, expected, held),
            Location::Destination(held) => {
                held.file.sync_all().map_err(FileMoveFault::PayloadSync)
                    .map_err(FileMoveError::Recovery)?;
                sync_both(&source, &destination, Ok(())).map_err(FileMoveError::Recovery)?;
                verify_destination(&source, from, &destination, to, expected)
                    .map_err(FileMoveError::Recovery)?;
                Ok(held.receipt)
            }
        }
    }
}

fn admit_move<'a>(
    source_root: &'a DiskRoot,
    from: &EntryName,
    destination_root: &'a DiskRoot,
    to: &EntryName,
    expected: FrozenFileMove,
) -> Result<(RootOperation<'a>, RootOperation<'a>, Location), FileMoveFault> {
    let source = source_root.operation().map_err(FileMoveFault::Gate)?;
    let destination = destination_root.operation().map_err(FileMoveFault::Gate)?;
    same_domain(source_root, destination_root, &source, &destination)?;
    let location = locate(&source, from, &destination, to, expected)?;
    Ok((source, destination, location))
}

fn same_domain(
    source_root: &DiskRoot,
    destination_root: &DiskRoot,
    source: &RootOperation<'_>,
    destination: &RootOperation<'_>,
) -> Result<(), FileMoveFault> {
    let bound = match (source_root.binding(), destination_root.binding()) {
        (Some(left), Some(right)) => left.same_domain(right),
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
    };
    if !bound || source.held().identity().device != destination.held().identity().device {
        return Err(FileMoveFault::Domain);
    }
    Ok(())
}
fn inspect(
    operation: &RootOperation<'_>,
    name: &EntryName,
    expected: FrozenFileMove,
) -> Result<Option<HeldPayload>, FileMoveFault> {
    let Some(mut file) = operation.open_optional_relative_regular(&name.as_relative())
        .map_err(FileMoveFault::Gate)? else { return Ok(None) };
    let identity = FileIdentity::from_fd(&file).map_err(FileMoveFault::Io)?;
    if identity.device != expected.entity.device() || identity.inode != expected.entity.inode() {
        return Err(FileMoveFault::Identity);
    }
    let receipt = platform_io::payload::verify_full_observed(
        &mut file,
        Expectation { bytes: expected.bytes, blake3: Some(expected.content_hash.digest()) },
        &mut || operation.check_running().map_err(|source| {
            io::Error::new(io::ErrorKind::PermissionDenied, source)
        }),
    ).map_err(FileMoveFault::Payload)?;
    Ok(Some(HeldPayload { file, receipt }))
}
fn locate(
    source: &RootOperation<'_>,
    from: &EntryName,
    destination: &RootOperation<'_>,
    to: &EntryName,
    expected: FrozenFileMove,
) -> Result<Location, FileMoveFault> {
    let at_source = inspect(source, from, expected)?;
    let at_destination = inspect(destination, to, expected)?;
    match (at_source, at_destination) {
        (Some(held), None) => Ok(Location::Source(held)),
        (None, Some(held)) => Ok(Location::Destination(held)),
        (Some(_), Some(_)) => Err(FileMoveFault::Occupied),
        (None, None) => Err(FileMoveFault::Missing),
    }
}
fn move_held(
    source: &RootOperation<'_>,
    from: &EntryName,
    destination: &RootOperation<'_>,
    to: &EntryName,
    expected: FrozenFileMove,
    held: HeldPayload,
) -> Result<Receipt, FileMoveError> {
    held.file.sync_all().map_err(FileMoveFault::PayloadSync).map_err(FileMoveError::Rejected)?;
    source.check_running().map_err(FileMoveFault::Gate).map_err(FileMoveError::Rejected)?;
    destination.check_running().map_err(FileMoveFault::Gate).map_err(FileMoveError::Rejected)?;
    let renamed = rustix::fs::renameat_with(
        source.held().directory(), from.as_os_str(),
        destination.held().directory(), to.as_os_str(), rustix::fs::RenameFlags::NOREPLACE,
    ).map_err(io::Error::from);
    sync_both(source, destination, renamed).map_err(FileMoveError::Attempted)?;
    verify_destination(source, from, destination, to, expected).map_err(FileMoveError::Attempted)?;
    Ok(held.receipt)
}
fn verify_destination(
    source: &RootOperation<'_>,
    from: &EntryName,
    destination: &RootOperation<'_>,
    to: &EntryName,
    expected: FrozenFileMove,
) -> Result<(), FileMoveFault> {
    match locate(source, from, destination, to, expected)? {
        Location::Destination(_) => {
            source.check_running().map_err(FileMoveFault::Gate)?;
            destination.check_running().map_err(FileMoveFault::Gate)
        }
        Location::Source(_) => Err(FileMoveFault::UnexpectedLocation),
    }
}
fn sync_both(
    source: &RootOperation<'_>,
    destination: &RootOperation<'_>,
    renamed: io::Result<()>,
) -> Result<(), FileMoveFault> {
    // Evaluate both tails even when rename or the first barrier failed. Unknown
    // outcomes keep every entry and all evidence; this function never cleans up.
    let destination_sync = destination.held().directory().sync_all();
    let source_sync = source.held().directory().sync_all();
    match (renamed, destination_sync, source_sync) {
        (Err(source), destination, origin) => Err(FileMoveFault::Rename {
            source, destination_sync: destination.err(), source_sync: origin.err(),
        }),
        (Ok(()), Err(first), second) => Err(FileMoveFault::Sync { first, second: second.err() }),
        (Ok(()), Ok(()), Err(first)) => Err(FileMoveFault::Sync { first, second: None }),
        (Ok(()), Ok(()), Ok(())) => Ok(()),
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/disk/worktree_move.rs"]
mod tests;

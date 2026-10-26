//! Read-only replica selection for hash-bound publication evidence.
//!
//! Callers supply trusted metadata members and authoritative journal/candidate
//! hashes. A replica is never selected by voting, and reads never repair data.
use crate::{disk::*, journal::*, *};

// The journal supplies the authoritative digest. A replica is usable only if
// its decoded object matches that exact digest, never by majority agreement.
pub(crate) fn verified_object<
    T: data_serialization::Serialize + data_serialization::de::DeserializeOwned,
>(
    members: &[DiskConfig],
    kind: &str,
    hash: &str,
) -> Result<T> {
    let mut first_error = None;
    for member in members {
        match load_object(member, kind, hash) {
            Ok(value) => return Ok(value),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::RecoveryRequired,
            "no trusted object replica",
        )
    }))
}

pub(crate) fn verified_graph_object(
    members: &[DiskConfig],
    kind: &'static str,
    hash: &str,
) -> Result<AuthenticatedMetadataObject> {
    let mut first_error = None;
    for member in members {
        match load_authenticated_object(member, kind, hash) {
            Ok(object) => return Ok(object),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::RecoveryRequired,
            "no trusted object replica",
        )
    }))
}

// A candidate binds one exact root and every referenced page digest. Resolve
// each object independently across trusted members: no member needs to retain
// the whole closure, but every dependency must authenticate before returning.
// This read path never repairs or rewrites any replica.
pub(crate) fn verified_snapshot(
    members: &[DiskConfig],
    hash: &str,
) -> Result<crate::history_package::AdmittedState> {
    let snapshot: Snapshot = verified_object(members, "snapshot", hash)?;
    resolve_admitted_snapshot(snapshot, |hash| {
        verified_object(members, "state-page", hash)
    })
}
/// A seal has no independently supplied digest: validate its complete receipt
/// barrier against the exact candidate and publication before accepting a copy.
pub(crate) fn verified_publication_seal(
    members: &[DiskConfig],
    candidate: &Candidate,
    publication: &Publication,
) -> Result<PublicationSeal> {
    let hash = object_hash("candidate", candidate)?;
    let mut first_error = None;
    for member in members {
        let result = read_json(&publication_seal_path(member, &hash)).and_then(|seal| {
            verify_publication_seal(candidate, publication, &seal)?;
            Ok(seal)
        });
        match result {
            Ok(seal) => return Ok(seal),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::RecoveryRequired,
            "no trusted publication seal replica",
        )
    }))
}

pub(crate) fn repair_object_copies<T: data_serialization::Serialize>(
    members: &[DiskConfig],
    kind: &str,
    hash: &str,
    value: &T,
) -> Result<()> {
    if object_hash(kind, value)? != hash {
        return Err(Error::new(
            storage_contract::ErrorCode::ObjectCorrupt,
            "repair object digest differs",
        ));
    }
    let bytes = encode_metadata(value, MAX_METADATA)?;
    repair_object_bytes(members, kind, hash, &bytes)
}

pub(crate) fn repair_authenticated_object(
    members: &[DiskConfig],
    object: &AuthenticatedMetadataObject,
) -> Result<()> {
    repair_object_bytes(members, object.kind(), object.hash(), object.bytes())
}

fn repair_object_bytes(members: &[DiskConfig], kind: &str, hash: &str, bytes: &[u8]) -> Result<()> {
    for member in members {
        let mut paths = existing_object_paths(member, kind, hash)?;
        if paths.is_empty() {
            paths.push(object_path(member, kind, hash));
        }
        for path in paths {
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    // Never replace an unsafe entry, even if a healthy replica exists.
                    let replace = match open_optional_regular(&path)? {
                        None => false,
                        Some(file) => match read_file_bytes(file, MAX_METADATA) {
                            Ok(existing) => existing != bytes,
                            Err(error)
                                if error.code() == storage_contract::ErrorCode::ResourceLimit =>
                            {
                                true
                            }
                            Err(error) => return Err(error),
                        },
                    };
                    // Matching bytes may be the visible result of a previous
                    // repair whose directory sync failed. Complete its barrier.
                    write_atomic(&path, bytes, replace)?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    write_atomic(&path, bytes, false)?;
                }
                Err(e) => return Err(e.into()),
                Ok(_) => {
                    return Err(Error::new(
                        storage_contract::ErrorCode::UnsafePath,
                        "object repair target is not a regular file",
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../test/engine/unit/publication_evidence_tests.rs"]
mod tests;

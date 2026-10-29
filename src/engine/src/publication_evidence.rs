//! Read-only replica selection for hash-bound publication evidence.
//!
//! Callers supply trusted metadata members and authoritative journal/candidate
//! hashes. A replica is never selected by voting, and reads never repair data.
use crate::{disk::*, journal::*, *};

// The journal supplies the authoritative digest. A replica is usable only if
// its decoded object matches that exact digest, never by majority agreement.
#[cfg(test)]
pub(crate) fn verified_object_observed<
    T: data_serialization::Serialize + data_serialization::de::DeserializeOwned,
>(
    members: &[DiskConfig],
    kind: &str,
    hash: &str,
    fault: &mut impl FnMut(&Error),
) -> Result<T> {
    select_replica(
        members,
        |member| load_object(member, kind, hash),
        fault,
        "no trusted object replica",
    )
}

pub(crate) fn verified_object_at<
    T: data_serialization::Serialize + data_serialization::de::DeserializeOwned,
>(
    resources: &DiskResources,
    members: &[DiskConfig],
    kind: &str,
    hash: &str,
) -> Result<T> {
    verified_object_observed_at(resources, members, kind, hash, &mut |_| {})
}

pub(crate) fn verified_object_observed_at<
    T: data_serialization::Serialize + data_serialization::de::DeserializeOwned,
>(
    resources: &DiskResources,
    members: &[DiskConfig],
    kind: &str,
    hash: &str,
    fault: &mut impl FnMut(&Error),
) -> Result<T> {
    select_replica(
        members,
        |member| load_object_at(&resources.get(member)?.root, member, kind, hash),
        fault,
        "no trusted object replica",
    )
}

#[cfg(test)]
pub(crate) fn verified_graph_object(
    members: &[DiskConfig],
    kind: &'static str,
    hash: &str,
) -> Result<AuthenticatedMetadataObject> {
    verified_graph_object_observed(members, kind, hash, &mut |_| {})
}

#[cfg(test)]
pub(crate) fn verified_graph_object_observed(
    members: &[DiskConfig],
    kind: &'static str,
    hash: &str,
    fault: &mut impl FnMut(&Error),
) -> Result<AuthenticatedMetadataObject> {
    select_replica(
        members,
        |member| load_authenticated_object(member, kind, hash),
        fault,
        "no trusted object replica",
    )
}

pub(crate) fn verified_graph_object_at(
    resources: &DiskResources,
    members: &[DiskConfig],
    kind: &'static str,
    hash: &str,
) -> Result<AuthenticatedMetadataObject> {
    verified_graph_object_observed_at(resources, members, kind, hash, &mut |_| {})
}

pub(crate) fn verified_graph_object_observed_at(
    resources: &DiskResources,
    members: &[DiskConfig],
    kind: &'static str,
    hash: &str,
    fault: &mut impl FnMut(&Error),
) -> Result<AuthenticatedMetadataObject> {
    select_replica(
        members,
        |member| load_authenticated_object_at(&resources.get(member)?.root, member, kind, hash),
        fault,
        "no trusted object replica",
    )
}

// A candidate binds one exact root and every referenced page digest. Resolve
// each object independently across trusted members: no member needs to retain
// the whole closure, but every dependency must authenticate before returning.
// This read path never repairs or rewrites any replica.
#[cfg(test)]
pub(crate) fn verified_snapshot(
    members: &[DiskConfig],
    hash: &str,
) -> Result<crate::history_package::AdmittedState> {
    verified_snapshot_observed(members, hash, &mut |_| {})
}

#[cfg(test)]
pub(crate) fn verified_snapshot_observed(
    members: &[DiskConfig],
    hash: &str,
    fault: &mut impl FnMut(&Error),
) -> Result<crate::history_package::AdmittedState> {
    let snapshot: Snapshot = verified_object_observed(members, "snapshot", hash, fault)?;
    resolve_verified_snapshot(snapshot, fault, |kind, hash, fault| {
        verified_graph_object_observed(members, kind, hash, fault)
    })
}

pub(crate) fn verified_snapshot_at(
    resources: &DiskResources,
    members: &[DiskConfig],
    hash: &str,
) -> Result<crate::history_package::AdmittedState> {
    verified_snapshot_observed_at(resources, members, hash, &mut |_| {})
}

pub(crate) fn verified_snapshot_observed_at(
    resources: &DiskResources,
    members: &[DiskConfig],
    hash: &str,
    fault: &mut impl FnMut(&Error),
) -> Result<crate::history_package::AdmittedState> {
    let snapshot: Snapshot =
        verified_object_observed_at(resources, members, "snapshot", hash, fault)?;
    resolve_verified_snapshot(snapshot, fault, |kind, hash, fault| {
        verified_graph_object_observed_at(resources, members, kind, hash, fault)
    })
}

fn resolve_verified_snapshot<F: FnMut(&Error)>(
    snapshot: Snapshot,
    fault: &mut F,
    mut load: impl FnMut(&'static str, &str, &mut F) -> Result<AuthenticatedMetadataObject>,
) -> Result<crate::history_package::AdmittedState> {
    let result = resolve_admitted_snapshot(snapshot, |kind, hash| load(kind, hash, fault));
    if let Err(error) = &result {
        fault(error);
    }
    result
}
/// A seal has no independently supplied digest: validate its complete receipt
/// barrier against the exact candidate and publication before accepting a copy.
pub(crate) fn verified_publication_seal_at(
    resources: &DiskResources,
    members: &[DiskConfig],
    candidate: &Candidate,
    publication: &Publication,
) -> Result<PublicationSeal> {
    verified_publication_seal_observed_at(resources, members, candidate, publication, &mut |_| {})
}

pub(crate) fn verified_publication_seal_observed_at(
    resources: &DiskResources,
    members: &[DiskConfig],
    candidate: &Candidate,
    publication: &Publication,
    fault: &mut impl FnMut(&Error),
) -> Result<PublicationSeal> {
    let hash = object_hash("candidate", candidate)?;
    select_replica(
        members,
        |member| {
            read_publication_seal(
                &resources.get(member)?.root,
                member,
                &hash,
                candidate,
                publication,
            )
        },
        fault,
        "no trusted publication seal replica",
    )
}

fn read_publication_seal(
    root: &DiskRoot,
    member: &DiskConfig,
    hash: &str,
    candidate: &Candidate,
    publication: &Publication,
) -> Result<PublicationSeal> {
    let seal =
        decode_strict(&root.read_bytes(&publication_seal_path(member, hash), MAX_METADATA)?)?;
    verify_publication_seal(candidate, publication, &seal)?;
    Ok(seal)
}

fn select_replica<T>(
    members: &[DiskConfig],
    mut read: impl FnMut(&DiskConfig) -> Result<T>,
    fault: &mut impl FnMut(&Error),
    missing: &'static str,
) -> Result<T> {
    let mut first_error = None;
    for member in members {
        match read(member) {
            Ok(seal) => return Ok(seal),
            Err(error) => {
                fault(&error);
                first_error.get_or_insert(error);
            }
        }
    }
    Err(first_error
        .unwrap_or_else(|| Error::new(storage_contract::ErrorCode::RecoveryRequired, missing)))
}

#[cfg(test)]
pub(crate) fn repair_object_copies<T: data_serialization::Serialize>(
    members: &[DiskConfig],
    kind: &str,
    hash: &str,
    value: &T,
) -> Result<()> {
    let bytes = encode_repair(kind, hash, value)?;
    repair_object_bytes(members, kind, hash, &bytes)
}

pub(crate) fn repair_object_copies_at<T: data_serialization::Serialize>(
    resources: &DiskResources,
    members: &[DiskConfig],
    kind: &str,
    hash: &str,
    value: &T,
) -> Result<()> {
    let bytes = encode_repair(kind, hash, value)?;
    repair_object_bytes_at(resources, members, kind, hash, &bytes)
}

fn encode_repair<T: data_serialization::Serialize>(
    kind: &str,
    hash: &str,
    value: &T,
) -> Result<Vec<u8>> {
    if object_hash(kind, value)? != hash {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "repair object digest differs",
        ));
    }
    encode_metadata(value, MAX_METADATA)
}

#[cfg(test)]
pub(crate) fn repair_authenticated_object_at(
    resources: &DiskResources,
    members: &[DiskConfig],
    object: &AuthenticatedMetadataObject,
) -> Result<()> {
    repair_object_bytes_at(
        resources,
        members,
        object.kind(),
        object.hash(),
        object.bytes(),
    )
}

#[cfg(test)]
fn repair_object_bytes(members: &[DiskConfig], kind: &str, hash: &str, bytes: &[u8]) -> Result<()> {
    for member in members {
        repair_member(&DiskRoot::open(&member.root)?, member, kind, hash, bytes)?;
    }
    Ok(())
}

fn repair_object_bytes_at(
    resources: &DiskResources,
    members: &[DiskConfig],
    kind: &str,
    hash: &str,
    bytes: &[u8],
) -> Result<()> {
    for member in members {
        repair_member(&resources.get(member)?.root, member, kind, hash, bytes)?;
    }
    Ok(())
}

pub(crate) fn repair_member(
    root: &DiskRoot,
    member: &DiskConfig,
    kind: &str,
    hash: &str,
    bytes: &[u8],
) -> Result<()> {
    let address = ObjectAddress::parse(kind, hash)?;
    let mut batch = root.object_install_batch();
    repair_member_in_batch(&mut batch, member, &address, bytes)?;
    batch.finish()
}

fn repair_member_in_batch(
    batch: &mut ObjectInstallBatch<'_>,
    member: &DiskConfig,
    address: &ObjectAddress<'_>,
    bytes: &[u8],
) -> Result<()> {
    let paths = address.existing_paths(batch.root(), member)?;
    if paths.is_empty() {
        return batch.write(address, bytes, false);
    }
    for path in paths {
        match batch.root().optional_metadata(&path)? {
            Some(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                // Never replace an unsafe entry, even if a healthy replica exists.
                let replace = match batch.root().open_optional_regular(&path)? {
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
                batch.write(address, bytes, replace)?;
            }
            None => {
                batch.write(address, bytes, false)?;
            }
            Some(_) => {
                return Err(Error::new(
                    storage_contract::ErrorCode::UnsafePath,
                    "object repair target is not a regular file",
                ));
            }
        }
    }
    Ok(())
}

// Recovery holds this many directory obligations across all members, not per member.
const MAX_RECOVERY_OBJECT_DIRECTORIES: usize = ObjectInstallBatch::MAX_PENDING_DIRECTORIES;

#[must_use = "finish all recovery object barriers before publishing or caching success"]
pub(crate) struct RecoveryObjectRepairBatch<'a> {
    members: Vec<(&'a DiskConfig, ObjectInstallBatch<'a>)>,
    pending_directories: usize,
}

impl<'a> RecoveryObjectRepairBatch<'a> {
    pub(crate) fn new(resources: &'a DiskResources, members: &'a [DiskConfig]) -> Result<Self> {
        let members = members
            .iter()
            .map(|member| Ok((member, resources.get(member)?.root.object_install_batch())))
            .collect::<Result<_>>()?;
        Ok(Self {
            members,
            pending_directories: 0,
        })
    }

    pub(crate) fn repair(&mut self, object: &AuthenticatedMetadataObject) -> Result<()> {
        let address = ObjectAddress::parse(object.kind(), object.hash())?;
        let directories = address.directory_count();
        assert!(
            directories <= MAX_RECOVERY_OBJECT_DIRECTORIES,
            "invariant: one object directory chain fits the recovery batch"
        );
        for index in 0..self.members.len() {
            if self.pending_directories + directories > MAX_RECOVERY_OBJECT_DIRECTORIES {
                self.flush()?;
            }
            let (member, batch) = &mut self.members[index];
            let before = batch.pending_directories();
            repair_member_in_batch(batch, member, &address, object.bytes())?;
            self.pending_directories += batch.pending_directories() - before;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        for (_, batch) in &mut self.members {
            let root = batch.root();
            let pending = std::mem::replace(batch, root.object_install_batch());
            pending.finish()?;
        }
        self.pending_directories = 0;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<()> {
        self.flush()
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/publication_evidence_tests.rs"]
mod tests;

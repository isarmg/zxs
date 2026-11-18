//! Authenticate the selected Snapshot and its inclusion path. Neither chooses
//! the original decision; the native prefix owner supplies it independently.
use super::{
    ScopeClosureError as Failure,
    genesis::{MAX_DISKS, PAGE_LIMIT, registration_key},
    objects::{self, Domain},
    schema::{Closure, DiskManifest, Membership, Origin, Scope},
};
use crate::{
    DiskConfig, Error, ErrorCode, Result, ValidatedConfig, disk::DiskRoot, journal, metadata_map,
};
use storage_contract::working_tree::{DiskRegistrationIdentity, MaterialCommitment};
use storage_contract::{DiskId, OperationId, Sequence};

/// All original registrations must appear even though this caller opens one
/// local manifest. With at most 32 bounded tuples the map is one canonical leaf.
pub(super) fn manifest_reference(
    config: &ValidatedConfig,
    disk: &DiskConfig,
    root: &DiskRoot,
    reference: &metadata_map::Reference,
    identity: &DiskRegistrationIdentity,
) -> Result<MaterialCommitment> {
    let n = config.storage_roots.len();
    if n == 0 || n > MAX_DISKS || reference.kind() != metadata_map::ReferenceKind::Page {
        return Err(Failure::PublicationMismatch.into());
    }
    let mut rows = metadata_map::read_map(reference, |kind, hash| {
        journal::load_scope_page_at(root, disk, kind, hash, PAGE_LIMIT)
    })?;
    if rows.len() != n {
        return Err(Failure::PublicationMismatch.into());
    }
    let mut selected = None;
    for binding in config.storage_roots.values() {
        let key = registration_key(&binding.registration)?;
        let value = rows.remove(&key).ok_or(Failure::PublicationMismatch)?;
        let hash: MaterialCommitment = json_codec::from_value(value)?;
        if binding.registration == *identity {
            selected = Some(hash);
        }
    }
    selected.ok_or_else(|| Failure::PublicationMismatch.into())
}

pub(in crate::engine) struct VerifiedScopeSnapshot {
    root: MaterialCommitment,
    sequence: Sequence,
}
impl VerifiedScopeSnapshot {
    pub(in crate::engine) fn read(
        object: journal::AuthenticatedMetadataObject,
        original_hash: MaterialCommitment,
        sequence: Sequence,
    ) -> Result<Self> {
        if object.kind() != "snapshot"
            || object.hash() != original_hash.hash().to_string()
            || sequence.get() == 0
        {
            return Err(Failure::PublicationMismatch.into());
        }
        let json_codec::Value::Object(mut fields) = object.into_value() else {
            return Err(Failure::PublicationMismatch.into());
        };
        let scope = fields.remove("scope_closure").ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "original Snapshot has no scope closure; preserve its material",
            )
        })?;
        let root = json_codec::from_value(scope)?;
        // These are the state fields of the exact eight-field object. This is
        // not a fallback that elects or hashes the seven-field state projection.
        let descriptor: journal::Snapshot =
            json_codec::from_value(json_codec::Value::Object(fields))?;
        journal::validate_snapshot_schema(&descriptor)?;
        if descriptor.sequence != sequence.get() {
            return Err(Failure::PublicationMismatch.into());
        }
        Ok(Self { root, sequence })
    }

    /// Fixed-membership adapter. A successor needs its original complete
    /// registration map; the present configuration cannot substitute for it.
    pub(in crate::engine) fn history(
        &self,
        config: &ValidatedConfig,
        disk: &DiskConfig,
        root: &DiskRoot,
        identity: &DiskRegistrationIdentity,
        operation_id: OperationId,
        request_digest: MaterialCommitment,
        predecessor: crate::engine::disk_catalog::Predecessor,
    ) -> Result<HistoryReference> {
        if config.generation().is_some() {
            return Err(Failure::PublicationMismatch.into());
        }
        let config_hash = MaterialCommitment::from_digest(
            storage_contract::canonical_object_digest("pool-config", &**config)?,
        );
        let origin = Origin::Publication {
            operation_id,
            request_digest,
            predecessor,
        };
        let closure: Closure = objects::load(root, Domain::Closure, self.root)?;
        let n = config.storage_roots.len() as u64;
        if closure.pool_id.to_string() != identity.pool_id
            || closure.origin != origin
            || closure.sequence != self.sequence
            || closure.membership != (Membership::Genesis { config_hash })
            || closure.counts.disks != n
            || n == 0
            || n > MAX_DISKS as u64
            || closure.counts.materials < 2 * n + 1
        {
            return Err(Failure::PublicationMismatch.into());
        }
        let saved = config
            .storage_roots
            .get(&identity.disk_id.parse::<DiskId>()?)
            .ok_or(Failure::PublicationMismatch)?;
        if saved.registration != *identity {
            return Err(Failure::PublicationMismatch.into());
        }
        let hash = manifest_reference(config, disk, root, &closure.disks, identity)?;
        let manifest: DiskManifest = objects::load(root, Domain::Disk, hash)?;
        let history = manifest.disk_history;
        if manifest.origin != origin
            || manifest.sequence != self.sequence
            || manifest.root_binding != *saved
            || manifest.registration != *identity
            || history.scope != Scope::DiskHistory
            || history.identity != *identity
            || manifest.disk_current.scope != Scope::DiskCurrent
            || manifest.disk_current.identity != *identity
            || manifest.counts.materials < 2
            || manifest.counts.canonical_metadata_bytes > closure.counts.canonical_metadata_bytes
        {
            return Err(Failure::PublicationMismatch.into());
        }
        // DISK SQLite has no separate payload bodies. Read the empty page;
        // neither FILE/worktree inventory nor other disks are claimed here.
        if history.body_inventory.kind() != metadata_map::ReferenceKind::Page {
            return Err(Failure::PublicationMismatch.into());
        }
        let empty = super::OrdinalBuilder::new(0).finish()?;
        empty.verify(disk, root, &history.body_inventory)?;
        Ok(HistoryReference {
            current_root: manifest.disk_current.logical_root,
            current_representation: manifest.disk_current.representation,
            both_have_no_bodies: manifest.disk_current.body_inventory == history.body_inventory,
            logical_root: history.logical_root,
            representation: history.representation,
            changes: manifest.local_changes,
        })
    }
}

pub(in crate::engine) struct HistoryReference {
    pub(in crate::engine) current_root: MaterialCommitment,
    pub(in crate::engine) current_representation: super::Representation,
    // The physical, canonical empty history-body page was already read on
    // this holder. Current has no payload bodies of its own either.
    pub(in crate::engine) both_have_no_bodies: bool,
    pub(in crate::engine) logical_root: MaterialCommitment,
    pub(in crate::engine) representation: super::Representation,
    pub(in crate::engine) changes: metadata_map::Reference,
}

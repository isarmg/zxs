//! Join an independently decided frontier to one holder's physical history.
//! Neither a SQLite nor a prospective package can construct Origin.
use super::{
    Binding, DiskHistoryHeader, DiskHistorySnapshot, StoredHistory, Through, change_pages,
    database, invalid, replay, stored,
};
use crate::{
    DiskRegistrationIdentity, Engine, Error, ErrorCode, OperationId, Result, ValidatedConfig,
    disk::{DiskRoot, HeldRegular, MAX_METADATA, decode_seal, identity_at, validate_stored_seal},
    engine::{
        disk_catalog::{DiskCatalogSnapshot, MAX_DATABASE_BYTES},
        fresh_layout,
        scope_closure::{Anchor, HistoryReference, Representation, Schema, VerifiedScopeSnapshot},
    },
    persist::layout,
};
use storage_contract::{ContentHash, DiskId, Sequence, working_tree::MaterialCommitment};

pub(in crate::engine) struct Original {
    configuration: ValidatedConfig,
    snapshot: VerifiedScopeSnapshot,
    expected: StoredHistory,
    current: DiskCatalogSnapshot,
    header: DiskHistoryHeader,
    binding: Binding,
    candidate_hash: MaterialCommitment,
    snapshot_hash: MaterialCommitment,
}

impl Engine {
    /// Select a requested already-decided, fixed-membership frontier. The
    /// target's latest local file cannot select it. A later membership needs
    /// its own original registration and cannot be served from today's disk set.
    pub(in crate::engine) fn original_disk_history(
        &self,
        id: DiskId,
        sequence: Sequence,
        operation: OperationId,
    ) -> Result<Original> {
        if self.config.generation().is_some() || sequence.get() == 0 {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "exact DISK history requires its original fixed membership and a positive frontier",
            ));
        }
        let configuration = Anchor::decode(self.config.fixed_genesis()?.bytes())?;
        let selected = configuration
            .storage_roots
            .get(&id)
            .ok_or_else(|| invalid("DISK scope is outside the original registration set"))?;
        let (captured, current) = replay::capture_pair(self, selected.registration.clone(), sequence.get())?;
        if !matches!(
            captured.binding.through,
            Through::Publication { sequence: n, operation_id, .. }
                if n == sequence.get() && operation_id == operation
        ) {
            return Err(invalid("DISK scope does not belong to the requested native operation"));
        }
        let candidate = captured
            .binding
            .candidate
            .as_deref()
            .ok_or_else(|| invalid("missing original DISK candidate"))?;
        let snapshot_hash = captured
            .view
            .header
            .through_snapshot
            .as_deref()
            .ok_or_else(|| invalid("missing original DISK snapshot"))?;
        let native = crate::publication_evidence::verified_canonical_snapshot_at(
            &self.disk_resources,
            &self.members(),
            snapshot_hash,
            &mut |error| self.metadata_integrity.fault(error),
        )?;
        let candidate_hash = stored::commitment(candidate)?;
        let snapshot_hash = stored::commitment(snapshot_hash)?;
        let snapshot = VerifiedScopeSnapshot::read(native, snapshot_hash, sequence)?;
        let expected = StoredHistory::from_replayed(&captured.view, captured.binding.through)?;
        Ok(Original {
            configuration,
            snapshot,
            expected,
            current,
            header: captured.view.header,
            binding: captured.binding,
            candidate_hash,
            snapshot_hash,
        })
    }
}

/// Nonserializable evidence for one DISK history, not a whole-disk receipt,
/// FILE inventory, decision, restore capability or authorization to write.
pub(in crate::engine) struct VerifiedHistory {
    pub(in crate::engine) registration: DiskRegistrationIdentity,
    pub(in crate::engine) sequence: Sequence,
    pub(in crate::engine) operation_id: OperationId,
    pub(in crate::engine) candidate_hash: MaterialCommitment,
    pub(in crate::engine) snapshot_hash: MaterialCommitment,
    pub(in crate::engine) logical_root: MaterialCommitment,
    pub(in crate::engine) image_hash: ContentHash,
    pub(in crate::engine) image_bytes: u64,
    business: DiskHistorySnapshot,
}
impl VerifiedHistory {
    pub(in crate::engine) fn into_business(self) -> DiskHistorySnapshot {
        self.business
    }
}

/// Both scoped databases are linked through the same manifest, operation and
/// registered physical owner. No FILE/inventory coverage is implied.
pub(in crate::engine) struct VerifiedCatalogs {
    pub(in crate::engine) history: VerifiedHistory,
    pub(in crate::engine) current: VerifiedCurrent,
}

pub(in crate::engine) struct VerifiedCurrent {
    pub(in crate::engine) logical_root: MaterialCommitment,
    pub(in crate::engine) image_hash: ContentHash,
    pub(in crate::engine) image_bytes: u64,
    pub(in crate::engine) catalog: DiskCatalogSnapshot,
}

impl Original {
    /// A deliberately selected history-only read need not open the current
    /// image. It never claims that current is present or proven.
    pub(in crate::engine) fn verify_local(self, root: &DiskRoot) -> Result<VerifiedHistory> {
        self.with_holder(root, |_, _| Ok(())).map(|(history, ())| history)
    }

    pub(in crate::engine) fn verify_catalogs(self, root: &DiskRoot) -> Result<VerifiedCatalogs> {
        let (history, current) = self.with_holder(root, |original, selected| {
            if !selected.both_have_no_bodies
                || super::offline::fold_stored(&original.expected, Some(&original.current))?
                    != original.current
            {
                return Err(invalid("original DISK current and complete history do not agree"));
            }
            verify_current_image(root, &original.current, selected.current_root, selected.current_representation)
        })?;
        Ok(VerifiedCatalogs { history, current })
    }

    /// Only this caller-selected, held target is addressed after capture. No
    /// peer reads, adopting a mutable catalog alias, or SQLite re-encoding.
    fn with_holder<T>(
        self,
        root: &DiskRoot,
        extra: impl FnOnce(&Self, &HistoryReference) -> Result<T>,
    ) -> Result<(VerifiedHistory, T)> {
        let Through::Publication {
            operation_id,
            request_digest,
            predecessor,
            sequence,
        } = self.expected.header.through else {
            return Err(invalid("not a published DISK history"));
        };
        let identity = &self.expected.header.identity;
        let mut disk = self.configuration.disks.iter()
            .find(|disk| disk.disk_id == identity.disk_id)
            .ok_or_else(|| invalid("DISK is outside the original configuration"))?
            .clone();
        disk.root = root.path().to_owned();
        if fresh_layout::registration(&self.configuration, &disk) != *identity {
            return Err(invalid("local DISK root differs from its original registration"));
        }
        // An independently supplied read-only root can be unbound. If this
        // is a managed root, retain its real I/O fence for the complete read.
        let lease = root.retain_io_activity()?;
        let sealed = HeldRegular::capture_entry(root, &layout::DISK_IDENTITY, MAX_METADATA)?
            .ok_or_else(|| invalid("missing original DISK seal; preserve material"))?;
        validate_stored_seal(
            &decode_seal(sealed.bytes())?,
            &identity_at(&self.configuration, &disk, &root.directory(root.path())?)?,
        )?;
        fresh_layout::verify_root(&self.configuration, &disk, root)?;
        let original_anchor = self.configuration.fixed_genesis()?.hash;
        if Anchor::read(root)?.fixed_genesis()?.hash != original_anchor {
            return Err(invalid("local DISK has a different fixed genesis"));
        }
        let material = self.snapshot.history(
            &self.configuration,
            &disk,
            root,
            identity,
            operation_id,
            request_digest,
            predecessor,
        )?;
        let logical_root = self.expected.logical_root()?;
        if material.logical_root != logical_root {
            return Err(invalid("DISK-history logical root differs from its native prefix"));
        }
        let changes = change_pages::ChangePages::prepare(&self.expected)?;
        changes.verify(&disk, root, &material.changes)?;
        let (local, additional) = material.representation.verify(
            root,
            Schema::DiskHistory,
            MAX_DATABASE_BYTES,
            |bytes| {
                let actual = database::read(bytes, identity)?;
                if actual != self.expected {
                    return Err(invalid("complete DISK SQLite differs from its native history"));
                }
                // In pair mode both image observations overlap: the original
                // history FD stays open and its real name/stamp is rechecked
                // after reading the complete current image.
                Ok((actual, extra(&self, &material)?))
            },
        )?;
        fresh_layout::verify_root(&self.configuration, &disk, root)?;
        let final_seal = HeldRegular::capture_entry(root, &layout::DISK_IDENTITY, MAX_METADATA)?
            .ok_or_else(|| invalid("original DISK seal disappeared during read"))?;
        if final_seal.stamp() != sealed.stamp()
            || Anchor::read(root)?.fixed_genesis()?.hash != original_anchor
        {
            return Err(invalid("original DISK evidence changed during read"));
        }
        sealed.check_running()?;
        if let Some(lease) = lease {
            lease.check_running()?;
        }
        let Representation::Sqlite { image_hash, image_bytes, .. } = material.representation else {
            return Err(invalid("native material cannot be a DISK history SQLite image"));
        };
        Ok((VerifiedHistory {
            registration: identity.clone(),
            sequence: Sequence::new(sequence),
            operation_id,
            candidate_hash: self.candidate_hash,
            snapshot_hash: self.snapshot_hash,
            logical_root,
            image_hash,
            image_bytes,
            business: local.resolved(&self.header, &self.binding)?,
        }, additional))
    }
}

pub(super) fn verify_current_image(
    root: &DiskRoot,
    expected: &DiskCatalogSnapshot,
    logical_root: MaterialCommitment,
    representation: Representation,
) -> Result<VerifiedCurrent> {
    let original = MaterialCommitment::from_digest(storage_contract::canonical_object_digest(
        "disk-current-catalog", expected,
    )?);
    if logical_root != original {
        return Err(invalid("DISK current logical root differs from the original terminal State"));
    }
    let catalog = representation.verify(root, Schema::DiskCurrent, MAX_DATABASE_BYTES, |bytes| {
        let actual = super::super::database::read(bytes, &expected.identity)?;
        if actual != *expected {
            return Err(invalid("DISK current SQLite does not contain the exact selected State"));
        }
        Ok(actual)
    })?;
    let Representation::Sqlite { image_hash, image_bytes, .. } = representation else {
        return Err(invalid("native material cannot be a DISK current SQLite image"));
    };
    Ok(VerifiedCurrent { logical_root, image_hash, image_bytes, catalog })
}

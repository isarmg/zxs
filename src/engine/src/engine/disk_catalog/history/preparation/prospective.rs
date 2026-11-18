//! Freeze the complete prospective pair from the admitted *previous* states
//! and the original request. There is intentionally no parameter for the new
//! Candidate/Snapshot hash. Replaying earlier material never changes it in place.
use super::*;
use super::super::stored::{Predecessor, RowSource, StoredHeader, StoredRow, commitment};
use crate::engine::scope_closure::{PreparedImage, Representation, Schema, ScopeClosureError};
use crate::disk::DiskRoot;
use storage_contract::working_tree::MaterialCommitment;

/// Original, bounded, pre-decision bytes and logical commitments. Not a
/// published binding: only a verified native decision may resolve current rows.
pub(in crate::engine) struct FrozenDiskPair {
    pub(in crate::engine) registration: DiskRegistrationIdentity,
    through: Through,
    current_root: MaterialCommitment,
    history_root: MaterialCommitment,
    current: PreparedImage,
    history: PreparedImage,
    changes: super::super::change_pages::ChangePages,
}
impl FrozenDiskPair {
    fn encode(current: &DiskCatalogSnapshot, history: &StoredHistory) -> Result<Self> {
        let (a, b) = PreparedDiskScopes::encode_pair(current, history)?;
        if super::super::super::database::read(&a, &current.identity)? != *current
            || database::read(&b, &current.identity)? != *history
        { return Err(ScopeClosureError::ImageMismatch.into()); }
        Ok(Self {
            registration: current.identity.clone(),
            through: history.header.through,
            current_root: MaterialCommitment::from_digest(storage_contract::canonical_object_digest("disk-current-catalog", current)?),
            history_root: history.logical_root()?,
            changes: super::super::change_pages::ChangePages::prepare(history)?,
            current: PreparedImage::new(Schema::DiskCurrent, a),
            history: PreparedImage::new(Schema::DiskHistory, b),
        })
    }

    pub(in crate::engine) fn current(&self) -> (MaterialCommitment, Representation) {
        (self.current_root, self.current.representation())
    }
    pub(in crate::engine) fn history(&self) -> (MaterialCommitment, Representation) {
        (self.history_root, self.history.representation())
    }
    pub(in crate::engine) fn local_changes(&self) -> &crate::metadata_map::Reference { self.changes.root() }
    pub(in crate::engine) fn bytes(&self) -> usize {
        self.current.bytes() + self.history.bytes() + self.changes.bytes()
    }

    /// Preserve immutable raw bytes at their actual registration's root prior
    /// to PREPARE. Existing bytes must match; never replace a damaged holder.
    pub(in crate::engine) fn install_at(&self, config: &ValidatedConfig, disk: &DiskConfig, root: &DiskRoot) -> Result<()> {
        if crate::engine::fresh_layout::registration(config, disk) != self.registration {
            return Err(ScopeClosureError::ImageMismatch.into());
        }
        crate::engine::fresh_layout::verify_root(config, disk, root)?;
        self.current.install_verified(root, |bytes| {
            let current = super::super::super::database::read(bytes, &self.registration)?;
            if current.source_sequence != self.through.sequence() ||
                MaterialCommitment::from_digest(storage_contract::canonical_object_digest("disk-current-catalog", &current)?) != self.current_root {
                return Err(ScopeClosureError::ImageMismatch.into());
            }
            Ok(())
        })?;
        self.history.install_verified(root, |bytes| {
            let history = database::read(bytes, &self.registration)?;
            if history.header.through != self.through || history.logical_root()? != self.history_root {
                return Err(ScopeClosureError::ImageMismatch.into());
            }
            Ok(())
        })?;
        self.changes.install(disk, root)?;
        crate::engine::fresh_layout::verify_root(config, disk, root)
    }
    pub(in crate::engine) fn into_bytes(self) -> (DiskRegistrationIdentity, Vec<u8>, Vec<u8>) {
        (self.registration, self.current.into_bytes(), self.history.into_bytes())
    }
}
impl PreparedDiskScopes {
    /// All earlier rows are already resolved by the native prefix owner.
    /// A fresh binding has genuinely empty prior rows, not copied history.
    pub(in crate::engine) fn freeze_next(
        self, before: &State, next: &State, previous_snapshot: Option<&str>,
        result: &OperationResult, stop: &AtomicBool,
    ) -> Result<Vec<FrozenDiskPair>> {
        if before.sequence >= next.sequence || next.sequence != result.sequence
            || next.operations.get(&result.operation_id) != Some(result)
            || before.operations.contains_key(&result.operation_id)
        { return Err(invalid("prospective disk pair does not extend the original operation")); }
        let through = Through::planned(&self.config, previous_snapshot, result.sequence,
            result.operation_id, &result.request_digest)?;
        let mut pairs = Vec::with_capacity(self.disks.len());
        let mut bytes = 0usize;
        for disk in self.disks {
            preparation_checkpoint(stop)?;
            let identity = disk.header.identity.clone();
            if disk.header.through_sequence != before.sequence || disk.header.through_snapshot.as_deref() != previous_snapshot
                || disk.header.publications as usize >= MAX_PUBLICATIONS
                || !disk.previous.same_rows(&snapshot(before, identity.clone())?)
            { return Err(invalid("prospective disk prefix does not match the selected native predecessor")); }
            let header = StoredHeader { identity: identity.clone(), through,
                publications: disk.header.publications + 1 };
            header.require_shape()?;
            // The first (possibly non-contiguous) chosen slot extends the real
            // fixed genesis; each later slot extends its original Snapshot.
            if let Through::Publication { predecessor: Predecessor::Genesis { .. }, .. } = through
                && before.sequence != 0 {
                return Err(invalid("nonempty DISK prefix cannot extend genesis"));
            }
            let current = snapshot(next, identity)?;
            let mut changes = Vec::with_capacity(disk.changes.rows.len());
            let mut record_bytes = 0usize;
            let mut push = |row: StoredRow| -> Result<()> {
                header.require_source(&row)?;
                let n = crate::disk::encode_metadata(&row, MAX_RECORD_BYTES)?.len();
                if changes.len() == MAX_CHANGES || n > MAX_RECORD_BYTES - record_bytes { return Err(budget()); }
                record_bytes += n;
                changes.push(row);
                Ok(())
            };
            for row in disk.changes.rows {
                push(StoredRow { sequence: row.sequence, operation_id: row.operation_id,
                    source: RowSource::Prior { candidate_hash: commitment(&row.candidate_hash)?,
                        snapshot_hash: commitment(&row.snapshot_hash)? }, change: row.change })?;
            }
            visit_changes(&disk.previous, &current, |change| push(StoredRow {
                sequence: result.sequence, operation_id: result.operation_id, source: RowSource::ThisPublication {}, change,
            }))?;
            let history = StoredHistory { header, changes };
            if super::super::offline::fold_stored(&history, Some(&current))? != current {
                return Err(invalid("prospective local changes do not close over the original current rows"));
            }
            let pair = FrozenDiskPair::encode(&current, &history)?;
            bytes = bytes.checked_add(pair.bytes()).filter(|n| *n <= MAX_SET_BYTES).ok_or_else(budget)?;
            preparation_checkpoint(stop)?;
            pairs.push(pair);
        }
        Ok(pairs)
    }
}

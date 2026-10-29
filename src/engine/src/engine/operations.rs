//! Frozen operation receipts and request-scoped, leased health observations.
//! Neither a receipt nor a read plan is an authorization grant. Network callers
//! must authorize the receipt before capturing health and recheck after slow I/O.
use super::{ContentReadPlan, Engine};
use crate::journal::RecordType;
use crate::{disk::DiskResources, journal::*, publication_evidence::verified_object_at, *};
use std::collections::BTreeSet;

/// Only the selected operation and its owner, never a clone of the whole State.
/// The publication facts remain unchanged when current content health changes.
pub struct OperationRecord {
    owner: Option<String>,
    result: OperationResult,
}

impl OperationRecord {
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    pub fn result(&self) -> &OperationResult {
        &self.result
    }

    pub fn into_result(self) -> OperationResult {
        self.result
    }
}

/// Owns one immutable descriptor and read lease per distinct committed version.
/// Configuration is shared across versions; no payload or proof is read during
/// capture. Drop cancels the observation and releases all leases.
pub struct OperationReadPlan {
    record: OperationRecord,
    versions: Vec<(ContentReadPlan, u32)>,
    unavailable: bool,
}

impl OperationReadPlan {
    /// Current named-disk identity, publication proof and complete payload
    /// verification happen after the caller releases its Engine mutex.
    pub fn check(mut self) -> OperationRecord {
        let mut degraded = false;
        for (plan, target_copies) in &self.versions {
            match plan.health(*target_copies) {
                storage_contract::CurrentHealth::Unavailable => self.unavailable = true,
                storage_contract::CurrentHealth::Degraded => degraded = true,
                storage_contract::CurrentHealth::Healthy => (),
                storage_contract::CurrentHealth::Unknown
                | storage_contract::CurrentHealth::Recovering => {
                    unreachable!("invariant: a complete replica scan establishes current health")
                }
            }
        }
        if self.unavailable {
            self.record.result.current_health = storage_contract::CurrentHealth::Unavailable;
        } else if degraded {
            self.record.result.current_health = storage_contract::CurrentHealth::Degraded;
        }
        self.record
    }
}

impl Engine {
    pub(crate) fn segment_reader<'a>(
        &'a self,
        members: &'a [DiskConfig],
    ) -> crate::segment_store::SegmentReader<'a> {
        crate::segment_store::SegmentReader {
            resources: &self.disk_resources,
            members,
            caches: &self.segment_caches,
        }
    }

    /// One permanent receipt by id over the current published State; see
    /// `segment_store::ReceiptLookup` for the exact lookup order.
    pub(crate) fn find_receipt(
        &self,
        id: OperationId,
    ) -> Result<Option<storage_contract::OperationRecordEntry>> {
        let members = self.members();
        crate::segment_store::ReceiptLookup {
            state: &self.state,
            reader: self.segment_reader(&members),
            locator_root: self.operation_locator_root.as_ref(),
        }
        .find(id)
    }

    /// A candidate's own receipt in its prepared or fully admitted snapshot.
    /// Preparation may have just sealed it; cold admission supplies the full
    /// locator index when the body is no longer resident. Never borrow the
    /// current published state's locator root for another snapshot.
    pub(crate) fn candidate_receipt(
        &self,
        state: &State,
        members: &[DiskConfig],
        id: OperationId,
    ) -> Result<Option<storage_contract::OperationRecordEntry>> {
        crate::segment_store::ReceiptLookup {
            state,
            reader: self.segment_reader(members),
            locator_root: None,
        }
        .find(id)
    }

    /// One published permanent receipt by id, without health recomputation.
    /// This is a read of the frozen publication facts for projections; it is
    /// not authorization and not the pending-decision path.
    pub fn published_receipt(&self, id: OperationId) -> Result<Option<OperationResult>> {
        Ok(self.find_receipt(id)?.map(|entry| entry.result))
    }

    /// The permanent receipt published at exactly this decision slot, if any.
    /// Locates the segment through the resident directory; one bounded read.
    pub fn receipt_by_sequence(
        &self,
        sequence: u64,
    ) -> Result<Option<storage_contract::OperationRecordEntry>> {
        let log = &self.state.operations;
        let tail = log.tail();
        if let Some(first) = tail.first() {
            if sequence >= first.result.sequence {
                let rank = tail.partition_point(|entry| entry.result.sequence < sequence);
                return Ok(tail
                    .get(rank)
                    .filter(|entry| entry.result.sequence == sequence)
                    .cloned());
            }
        }
        let sealed = log.sealed();
        let Some(index) = sealed
            .partition_point(|segment| segment.first_sequence <= sequence)
            .checked_sub(1)
        else {
            return Ok(None);
        };
        let members = self.members();
        let segment = self
            .segment_reader(&members)
            .operation_segment(&sealed[index])?;
        let rank = segment.partition_point(|entry| entry.result.sequence < sequence);
        Ok(segment
            .get(rank)
            .filter(|entry| entry.result.sequence == sequence)
            .cloned())
    }

    /// Pull permanent receipts newest-first. Each sealed segment is read on
    /// demand through the bounded cache; a caller that stops early reads only
    /// the segments it visited.
    pub fn receipts_descending(&self) -> ReceiptsDescending<'_> {
        ReceiptsDescending {
            engine: self,
            members: self.members(),
            tail_remaining: self.state.operations.tail().len(),
            segment: None,
            next_sealed: self.state.operations.sealed().len(),
        }
    }

    pub fn operation_owner(&self, id: OperationId) -> Result<Option<String>> {
        if let Some(entry) = self.find_receipt(id)? {
            return Ok(Some(entry.principal));
        }
        let (members, snapshot, _) = match self.pending_operation_snapshot(id) {
            Ok(pending) => pending,
            Err(error) if error.code() == storage_contract::ErrorCode::OperationNotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        Ok(pending_snapshot_receipt(self, &members, &snapshot, id)?.map(|entry| entry.principal))
    }

    /// Published records are metadata-only. An uncertain decision reads only
    /// the hash-bound locator path and one receipt segment, not the
    /// candidate's entire State.
    pub fn operation_record(&self, id: OperationId) -> Result<OperationRecord> {
        if let Some(entry) = self.find_receipt(id)? {
            return Ok(OperationRecord {
                owner: Some(entry.principal),
                result: entry.result,
            });
        }
        let (members, snapshot, candidate) = self.pending_operation_snapshot(id)?;
        let entry = pending_snapshot_receipt(self, &members, &snapshot, id)?
            .ok_or_else(operation_not_found)?;
        let (owner, mut result) = (Some(entry.principal), entry.result);
        if result.operation_id != id
            || result.request_digest != candidate.request_digest
            || result.sequence != candidate.sequence
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ObjectCorrupt,
                "operation differs from accepted candidate",
            ));
        }
        result.commit_state = storage_contract::CommitState::InDoubt;
        result.protection_at_publish = storage_contract::ProtectionAtPublish::Unconfirmed;
        result.current_health = storage_contract::CurrentHealth::Recovering;
        Ok(OperationRecord { owner, result })
    }

    /// Capture this immediately after authorizing `record`, in the same Engine
    /// critical section. The record is response data, not reusable admission.
    pub fn prepare_operation_health(&self, record: OperationRecord) -> OperationReadPlan {
        let mut plan = OperationReadPlan {
            record,
            versions: Vec::new(),
            unavailable: false,
        };
        // A pending candidate has no publication to inspect, even if its staged
        // commit happens to resemble an existing public commit.
        if plan.record.result.commit_state == storage_contract::CommitState::InDoubt {
            return plan;
        }
        let Some(commit) = plan
            .record
            .result
            .commit_id
            .as_ref()
            .and_then(|id| self.state.commits.get(id))
        else {
            return plan;
        };
        // Health covers the publication's complete commit: stream its file
        // versions through the paged namespace, deduplicated, without holding
        // the materialized tree.
        let mut versions = Vec::new();
        let streamed = self.with_namespace_source(|source| {
            let mut selected = BTreeSet::new();
            crate::namespace::NamespaceView::new(&commit.namespace, source).walk(|_, entry| {
                if let storage_contract::NamespacePageEntry::File { version_id, .. } = entry
                    && selected.insert(version_id.clone())
                {
                    versions.push(version_id.clone());
                }
                Ok(true)
            })
        });
        if streamed.is_err() {
            // An unreadable namespace page cannot prove any content healthy.
            plan.unavailable = true;
            return plan;
        }
        for version_id in versions {
            let prepared = self.prepare_content(&version_id);
            match prepared {
                Ok(content) => {
                    let target = self.state.placements[&version_id].target_copies;
                    plan.versions.push((content, target));
                }
                // Purged/migrated, missing or inconsistent descriptors all fail
                // closed, without rewriting the original publication receipt.
                Err(_) => plan.unavailable = true,
            }
        }
        plan
    }

    /// Synchronous convenience for trusted local callers. Shared backends must
    /// capture and execute the read plan separately to release their mutex.
    pub fn operation(&self, id: OperationId) -> Result<OperationResult> {
        Ok(self
            .prepare_operation_health(self.operation_record(id)?)
            .check()
            .into_result())
    }

    /// Only accepted, hash-bound decisions can supply an unpublished owner.
    /// Current format omits owner/history from Candidate, so this exceptional
    /// path must read the journal, candidate and selected snapshot pages. It
    /// never traverses version/commit/authentication pages or payload objects.
    fn pending_operation_snapshot(
        &self,
        id: OperationId,
    ) -> Result<(Vec<DiskConfig>, Snapshot, Candidate)> {
        if !self.blocked {
            return Err(operation_not_found());
        }
        let members = self.trusted_evidence_members();
        let mut first_error = None;
        for disk in &members {
            let scanned = match scan_at(&self.disk_resources.get(disk)?.root, disk) {
                Ok(scanned) => scanned,
                Err(error) => {
                    first_error.get_or_insert(error);
                    continue;
                }
            };
            for (record, _) in scanned.records.iter().rev().filter(|(record, _)| {
                matches!(
                    record.record_type,
                    RecordType::Accept | RecordType::Publication
                )
            }) {
                let candidate = match verified_object_at::<Candidate>(
                    &self.disk_resources,
                    &members,
                    "candidate",
                    &record.candidate_hash,
                ) {
                    Ok(candidate) => candidate,
                    Err(error) => {
                        first_error.get_or_insert(error);
                        continue;
                    }
                };
                if candidate.operation_id != id
                    || candidate.pool_id != self.config.pool_id
                    || candidate.sequence != record.sequence
                {
                    continue;
                }
                let snapshot: Snapshot = verified_object_at(
                    &self.disk_resources,
                    &members,
                    "snapshot",
                    &candidate.snapshot_hash,
                )?;
                validate_snapshot_schema(&snapshot)?;
                if snapshot.sequence != candidate.sequence {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ObjectCorrupt,
                        "snapshot sequence differs",
                    ));
                }
                return Ok((members, snapshot, candidate));
            }
        }
        Err(first_error.unwrap_or_else(operation_not_found))
    }
}

/// Newest-first pull over the permanent receipt log. Owned items; segment
/// bodies stay in the bounded cache, never pinned by the iterator.
pub struct ReceiptsDescending<'e> {
    engine: &'e Engine,
    members: Vec<DiskConfig>,
    tail_remaining: usize,
    segment: Option<(
        std::sync::Arc<Vec<storage_contract::OperationRecordEntry>>,
        usize,
    )>,
    next_sealed: usize,
}
impl Iterator for ReceiptsDescending<'_> {
    type Item = Result<storage_contract::OperationRecordEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.tail_remaining > 0 {
            self.tail_remaining -= 1;
            return Some(Ok(
                self.engine.state.operations.tail()[self.tail_remaining].clone()
            ));
        }
        loop {
            if let Some((segment, remaining)) = &mut self.segment {
                if *remaining > 0 {
                    *remaining -= 1;
                    return Some(Ok(segment[*remaining].clone()));
                }
                self.segment = None;
            }
            self.next_sealed = self.next_sealed.checked_sub(1)?;
            let reference = &self.engine.state.operations.sealed()[self.next_sealed];
            match self
                .engine
                .segment_reader(&self.members)
                .operation_segment(reference)
            {
                Ok(segment) => {
                    let length = segment.len();
                    self.segment = Some((segment, length));
                }
                Err(error) => return Some(Err(error)),
            }
        }
    }
}

fn operation_not_found() -> Error {
    Error::new(
        storage_contract::ErrorCode::OperationNotFound,
        "operation is not registered",
    )
}

// Verify exact bounded pages one at a time. Do not materialize unrelated maps,
// or trust an unverified index/cache as a substitute for the accepted root.
fn pending_snapshot_entry<T: data_serialization::de::DeserializeOwned>(
    resources: &DiskResources,
    members: &[DiskConfig],
    snapshot: &Snapshot,
    field: &str,
    key: &str,
) -> Result<Option<T>> {
    let reference = snapshot.maps.get(field).ok_or_else(|| {
        Error::new(
            ErrorCode::ObjectCorrupt,
            "operation snapshot field is missing",
        )
    })?;
    let value = crate::metadata_map::lookup(reference, key, |kind, hash| {
        crate::publication_evidence::verified_graph_object_at(resources, members, kind, hash)
    })?;
    value
        .map(json_codec::from_value)
        .transpose()
        .map_err(Into::into)
}

/// Locate one receipt in a foreign accepted snapshot: locator → directory
/// entry (located by its recorded ordinal range) → exact segment body. The
/// located receipt is cross-checked against the id, so recorded counts are
/// placement hints that cannot substitute a wrong record.
fn pending_snapshot_receipt(
    engine: &Engine,
    members: &[DiskConfig],
    snapshot: &Snapshot,
    id: OperationId,
) -> Result<Option<storage_contract::OperationRecordEntry>> {
    let Some(text) = pending_snapshot_entry::<String>(
        &engine.disk_resources,
        members,
        snapshot,
        "operation_locators",
        &id.to_string(),
    )?
    else {
        return Ok(None);
    };
    let ordinal = storage_contract::parse_u64_decimal(&text)?;
    let entry_at = |segment: u64| -> Result<Option<storage_contract::OperationDirectoryEntry>> {
        pending_snapshot_entry(
            &engine.disk_resources,
            members,
            snapshot,
            "operation_log",
            &format!("{segment:020}"),
        )
    };
    let corrupt = || {
        Error::new(
            ErrorCode::ObjectCorrupt,
            "operation locator points outside the receipt log",
        )
    };
    let end = |entry: &storage_contract::OperationDirectoryEntry| {
        entry
            .first_ordinal
            .checked_add(entry.records)
            .ok_or_else(corrupt)
    };
    // Directory keys are contiguous segment ordinals. Gallop to a bracket
    // whose recorded range passes the target, then binary search inside it.
    let mut entry = entry_at(0)?.ok_or_else(corrupt)?;
    if ordinal >= end(&entry)? {
        let mut low = 0u64;
        let mut step = 1u64;
        let mut high = None;
        while high.is_none() {
            let probe = low.checked_add(step).ok_or_else(corrupt)?;
            match entry_at(probe)? {
                Some(candidate) if ordinal >= end(&candidate)? => {
                    low = probe;
                    step = step.checked_mul(2).ok_or_else(corrupt)?;
                }
                Some(candidate) if ordinal >= candidate.first_ordinal => {
                    entry = candidate;
                    high = Some(probe);
                }
                Some(_) | None => high = Some(probe),
            }
        }
        let mut high = high.expect("invariant: the gallop sets its bracket");
        while entry.first_ordinal > ordinal || ordinal >= end(&entry)? {
            if high <= low + 1 {
                return Err(corrupt());
            }
            let middle = low + (high - low) / 2;
            match entry_at(middle)? {
                Some(candidate) if ordinal >= end(&candidate)? => {
                    low = middle;
                }
                Some(candidate) => {
                    entry = candidate;
                    high = middle;
                }
                None => high = middle,
            }
        }
    }
    let segment = engine
        .segment_reader(members)
        .operation_segment(&entry.reference())?;
    let offset = usize::try_from(
        ordinal
            .checked_sub(entry.first_ordinal)
            .ok_or_else(corrupt)?,
    )
    .map_err(|_| corrupt())?;
    let record = segment.get(offset).cloned().ok_or_else(corrupt)?;
    if record.result.operation_id != id {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "operation locator points at a different receipt",
        ));
    }
    Ok(Some(record))
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/operations/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/operations/segmented_receipts_tests.rs"]
mod segmented_receipts_tests;

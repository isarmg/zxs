//! Frozen operation receipts and request-scoped, leased health observations.
//! Neither a receipt nor a read plan is an authorization grant. Network callers
//! must authorize the receipt before capturing health and recheck after slow I/O.
use super::{ContentReadPlan, Engine};
use crate::journal::RecordType;
use crate::{journal::*, publication_evidence::verified_object, *};
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
    /// All disk identity, publication proof and complete payload verification
    /// happens here, after the caller has released its Engine mutex.
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
    pub fn operation_owner(&self, id: OperationId) -> Result<Option<String>> {
        if let Some(owner) = self.state.operation_principals.get(&id) {
            return Ok(Some(owner.clone()));
        }
        let (members, snapshot, _) = match self.pending_operation_snapshot(id) {
            Ok(pending) => pending,
            Err(error) if error.code() == storage_contract::ErrorCode::OperationNotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        snapshot_entry(&members, &snapshot, "operation_principals", id)
    }

    /// Published records are metadata-only. An uncertain decision reads only
    /// the hash-bound owner/operation pages, not the candidate's entire State.
    pub fn operation_record(&self, id: OperationId) -> Result<OperationRecord> {
        if let Some(result) = self.state.operations.get(&id) {
            return Ok(OperationRecord {
                owner: self.state.operation_principals.get(&id).cloned(),
                result: result.clone(),
            });
        }
        let (members, snapshot, candidate) = self.pending_operation_snapshot(id)?;
        let owner = snapshot_entry(&members, &snapshot, "operation_principals", id)?;
        let mut result: OperationResult = snapshot_entry(&members, &snapshot, "operations", id)?
            .ok_or_else(operation_not_found)?;
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
        let mut selected = BTreeSet::new();
        for entry in commit
            .entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
        {
            if !selected.insert(&entry.version_id) {
                continue;
            }
            let prepared = self.prepare_content(&entry.version_id);
            match prepared {
                Ok(content) => {
                    let target = self.state.placements[&entry.version_id].target_copies;
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
            let scanned = match scan(disk) {
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
                let candidate = match verified_object::<Candidate>(
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
                let snapshot: Snapshot =
                    verified_object(&members, "snapshot", &candidate.snapshot_hash)?;
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

fn operation_not_found() -> Error {
    Error::new(
        storage_contract::ErrorCode::OperationNotFound,
        "operation is not registered",
    )
}

// Verify exact bounded pages one at a time. Do not materialize unrelated maps,
// or trust an unverified index/cache as a substitute for the accepted root.
fn snapshot_entry<T: data_serialization::Serialize + data_serialization::de::DeserializeOwned>(
    members: &[DiskConfig],
    snapshot: &Snapshot,
    field: &str,
    id: OperationId,
) -> Result<Option<T>> {
    let reference = snapshot.maps.get(field).ok_or_else(|| {
        Error::new(
            ErrorCode::ObjectCorrupt,
            "operation snapshot field is missing",
        )
    })?;
    let value = crate::metadata_map::lookup(reference, &id.to_string(), |kind, hash| {
        crate::publication_evidence::verified_graph_object(members, kind, hash)
    })?;
    value
        .map(json_codec::from_value)
        .transpose()
        .map_err(Into::into)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/operations/tests.rs"]
mod tests;

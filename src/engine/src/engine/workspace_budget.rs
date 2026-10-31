//! Admission for durable workspace evidence, not just currently open leases.
use super::Engine;
use crate::*;
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub(super) struct WorkspaceBudget {
    records: BTreeMap<String, WorkspaceCharge>,
    principals: BTreeMap<String, Usage>,
    resident: usize,
    bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkspaceCharge {
    principal: String,
    bytes: u64,
    retention: WorkspaceRetention,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkspaceRetention {
    Resident,
    Cold {
        fingerprint: blake3::Hash,
        mirrors: WorkspaceMirrors,
        kind: WorkspaceColdKind,
    },
}

/// In-memory admission proof, never a new manifest field or disk format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkspaceColdKind {
    EmptyAbort,
    Published,
    HistoricalPublished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkspaceMirrors {
    System,
    Primary,
    Both,
}

impl WorkspaceRetention {
    pub(super) fn is_cold(self) -> bool {
        matches!(self, Self::Cold { .. })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Usage {
    resident: usize,
    cold: usize,
    bytes: u64,
}

pub(super) struct WorkspaceAdmission {
    id: String,
    charge: WorkspaceCharge,
}

/// A reservation cannot hide an impossible post-publication receipt. This
/// limit is the same canonical-byte bound used by the durable writer/reader.
fn checked_metadata_charge(actual: usize, reserved: usize) -> Result<u64> {
    let charge = actual.max(reserved);
    if charge > crate::disk::MAX_METADATA {
        return Err(Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "workspace lifecycle metadata exceeds the durable manifest limit",
        ));
    }
    Ok(charge as u64)
}

/// Read-only admission counters. Remaining metadata bytes include lifecycle
/// reservations, not free disk space; terminal records still consume capacity.
fn capacity_scope(
    records: usize,
    cold: usize,
    bytes: u64,
    record_limit: usize,
    byte_limit: u64,
) -> WorkspaceCapacityScope {
    WorkspaceCapacityScope {
        records: CapacityMeasure {
            used: records as u64,
            limit: record_limit as u64,
            remaining: record_limit.saturating_sub(records) as u64,
        },
        cold_records: cold as u64,
        metadata_bytes: CapacityMeasure {
            used: bytes,
            limit: byte_limit,
            remaining: byte_limit.saturating_sub(bytes),
        },
    }
}

impl Engine {
    /// Report global and selected operation-actor retained evidence capacity.
    /// This is an observation, not a reservation or authorization decision.
    pub fn workspace_capacity(&self, principal: &str) -> WorkspaceCapacity {
        let limits = self.effective_workspace_limits();
        let budget = &self.workspace_budget;
        let principal_usage = budget
            .principals
            .get(principal)
            .copied()
            .unwrap_or_default();
        let counters = [
            (
                budget.resident as u128,
                limits.max_retained_workspaces as u128,
            ),
            (
                principal_usage.resident as u128,
                limits.max_retained_workspaces_per_principal as u128,
            ),
            (
                budget.bytes as u128,
                limits.max_retained_workspace_metadata_bytes as u128,
            ),
            (
                principal_usage.bytes as u128,
                limits.max_retained_workspace_metadata_bytes_per_principal as u128,
            ),
        ];
        let pressure = counters
            .into_iter()
            .map(|(used, limit)| {
                if used >= limit {
                    2
                } else if used * 5 >= limit * 4 {
                    1
                } else {
                    0
                }
            })
            .fold(0, usize::max);
        WorkspaceCapacity {
            global: capacity_scope(
                budget.resident,
                budget.records.len() - budget.resident,
                budget.bytes,
                limits.max_retained_workspaces,
                limits.max_retained_workspace_metadata_bytes,
            ),
            principal: capacity_scope(
                principal_usage.resident,
                principal_usage.cold,
                principal_usage.bytes,
                limits.max_retained_workspaces_per_principal,
                limits.max_retained_workspace_metadata_bytes_per_principal,
            ),
            warning: match pressure {
                2 => CapacityWarning::Exhausted,
                1 => CapacityWarning::NearLimit,
                _ => CapacityWarning::Ok,
            },
            automatic_retirement: true,
        }
    }

    /// Reserve lifecycle changes before publication, including the file-context
    /// receipt installed by a first inherited save. IDs are fixed-width; 64
    /// bytes covers every node/branch ID, including its prefix. Preserve larger
    /// existing values defensively rather than undercharging recovered records.
    fn workspace_metadata_charge(&self, workspace: &Workspace, actual: usize) -> Result<u64> {
        let mut reserved = workspace.clone();
        reserved.state = WorkspaceState::Committed;
        reserved.fencing_token = u64::MAX;
        let context_keys: BTreeMap<_, _> = reserved
            .selected_file_contexts
            .keys()
            .filter_map(|key| {
                key.split_once('/')
                    .map(|(_, path)| (path.to_owned(), key.clone()))
            })
            .collect();
        for file in workspace.files.values() {
            let path = file.path.key();
            // The repository prefix is a fixed-width repo ID. Use a stable
            // placeholder so charging also works before journal replay and
            // does not change when a repository's current main branch moves.
            let key = format!("repo_{}/{path}", "0".repeat(32));
            let existing = context_keys.get(path).cloned();
            let context = existing
                .and_then(|key| reserved.selected_file_contexts.remove(&key))
                .unwrap_or(FileContext {
                    node_id: None,
                    selection_generation: 0,
                    branch_id: String::new(),
                    inherited_from: None,
                });
            reserved.selected_file_contexts.insert(key, context);
        }
        for context in reserved.selected_file_contexts.values_mut() {
            context.node_id =
                Some("x".repeat(context.node_id.as_ref().map_or(64, |id| id.len().max(64))));
            context.selection_generation = u64::MAX;
            context.branch_id = "x".repeat(context.branch_id.len().max(64));
            context.inherited_from = Some(
                "x".repeat(
                    context
                        .inherited_from
                        .as_ref()
                        .map_or(64, |id| id.len().max(64)),
                ),
            );
        }
        // Do not undercharge unexpected escaped/oversized recovered fields.
        checked_metadata_charge(actual, canonical_json(&reserved)?.len())
    }

    /// Only the changed record is serialized; unrelated retained manifests are
    /// not rescanned on the write path. The admission token is installed only
    /// after durable success, or while reconstructing startup state.
    pub(super) fn check_workspace_budget(
        &self,
        candidate: &Workspace,
        canonical: &[u8],
        retention: WorkspaceRetention,
    ) -> Result<WorkspaceAdmission> {
        let limits = self.effective_workspace_limits();
        let budget = &self.workspace_budget;
        let old = budget.records.get(&candidate.workspace_id);
        if old.is_some_and(|old| old.principal != candidate.principal) {
            return Err(Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "retained workspace owner cannot change",
            ));
        }
        let charge = self.workspace_metadata_charge(candidate, canonical.len())?;
        let old_charge = old.map_or(0, |old| old.bytes);
        let was_resident = usize::from(old.is_some_and(|old| !old.retention.is_cold()));
        let resident = usize::from(!retention.is_cold());
        let principal = budget
            .principals
            .get(&candidate.principal)
            .copied()
            .unwrap_or_default();
        let bytes = budget
            .bytes
            .checked_sub(old_charge)
            .and_then(|bytes| bytes.checked_add(charge));
        let principal_bytes = principal
            .bytes
            .checked_sub(old_charge)
            .and_then(|bytes| bytes.checked_add(charge));
        let exhausted = if budget.resident - was_resident + resident
            > limits.max_retained_workspaces
        {
            Some("global record count")
        } else if principal.resident - was_resident + resident
            > limits.max_retained_workspaces_per_principal
        {
            Some("principal record count")
        } else if bytes.is_none_or(|bytes| bytes > limits.max_retained_workspace_metadata_bytes) {
            Some("global metadata bytes")
        } else if principal_bytes
            .is_none_or(|bytes| bytes > limits.max_retained_workspace_metadata_bytes_per_principal)
        {
            Some("principal metadata bytes")
        } else {
            None
        };
        if let Some(dimension) = exhausted {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                format!(
                    "workspace evidence {dimension} budget exhausted; all metadata and recovery evidence remain retained"
                ),
            ));
        }
        Ok(WorkspaceAdmission {
            id: candidate.workspace_id.clone(),
            charge: WorkspaceCharge {
                principal: candidate.principal.clone(),
                bytes: charge,
                retention,
            },
        })
    }

    pub(super) fn account_workspace(&mut self, admission: WorkspaceAdmission) {
        let budget = &mut self.workspace_budget;
        let principal = budget
            .principals
            .entry(admission.charge.principal.clone())
            .or_default();
        let resident = usize::from(!admission.charge.retention.is_cold());
        let cold = usize::from(admission.charge.retention.is_cold());
        let bytes = admission.charge.bytes;
        let old = budget.records.insert(admission.id, admission.charge);
        let old_bytes = old.as_ref().map_or(0, |old| old.bytes);
        let old_resident = usize::from(old.as_ref().is_some_and(|old| !old.retention.is_cold()));
        let old_cold = usize::from(old.as_ref().is_some_and(|old| old.retention.is_cold()));
        budget.bytes = budget.bytes - old_bytes + bytes;
        budget.resident = budget.resident - old_resident + resident;
        principal.bytes = principal.bytes - old_bytes + bytes;
        principal.resident = principal.resident - old_resident + resident;
        principal.cold = principal.cold - old_cold + cold;
    }

    pub(crate) fn retained_workspace_ids(&self) -> impl Iterator<Item = &str> {
        self.workspace_budget.records.keys().map(String::as_str)
    }

    pub(super) fn accounted_workspace(&self, id: &str) -> bool {
        self.workspace_budget.records.contains_key(id)
    }

    pub(super) fn cold_workspace_retention(&self, id: &str) -> Option<WorkspaceRetention> {
        self.workspace_budget
            .records
            .get(id)
            .and_then(|record| record.retention.is_cold().then_some(record.retention))
    }

    pub(crate) fn reset_workspace_budget(&mut self) {
        self.workspace_budget = WorkspaceBudget::default();
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/workspace_budget_helpers.rs"]
mod tests;

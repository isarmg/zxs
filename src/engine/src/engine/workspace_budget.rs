//! Admission for durable workspace evidence, not just currently open leases.
use super::Engine;
use crate::*;
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub(super) struct WorkspaceBudget {
    records: BTreeMap<String, (String, u64)>,
    principals: BTreeMap<String, (usize, u64)>,
    bytes: u64,
}

pub(crate) struct WorkspaceAdmission {
    id: String,
    principal: String,
    charge: u64,
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
        let (principal_records, principal_bytes) = budget
            .principals
            .get(principal)
            .copied()
            .unwrap_or_default();
        let counters = [
            (
                budget.records.len() as u128,
                limits.max_retained_workspaces as u128,
            ),
            (
                principal_records as u128,
                limits.max_retained_workspaces_per_principal as u128,
            ),
            (
                budget.bytes as u128,
                limits.max_retained_workspace_metadata_bytes as u128,
            ),
            (
                principal_bytes as u128,
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
                budget.records.len(),
                budget.bytes,
                limits.max_retained_workspaces,
                limits.max_retained_workspace_metadata_bytes,
            ),
            principal: capacity_scope(
                principal_records,
                principal_bytes,
                limits.max_retained_workspaces_per_principal,
                limits.max_retained_workspace_metadata_bytes_per_principal,
            ),
            warning: match pressure {
                2 => CapacityWarning::Exhausted,
                1 => CapacityWarning::NearLimit,
                _ => CapacityWarning::Ok,
            },
            automatic_retirement: false,
        }
    }

    /// Reserve lifecycle changes before publication, including the file-context
    /// receipt installed by a first inherited save. IDs are fixed-width; 64
    /// bytes covers every node/branch ID, including its prefix. Preserve larger
    /// existing values defensively rather than undercharging recovered records.
    fn workspace_metadata_charge(&self, workspace: &Workspace) -> Result<u64> {
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
            let path = name_policy::path_key(&file.path);
            // The repository prefix is a fixed-width repo ID. Use a stable
            // placeholder so charging also works before journal replay and
            // does not change when a repository's current main branch moves.
            let key = format!("repo_{}/{path}", "0".repeat(32));
            let existing = context_keys.get(&path).cloned();
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
        checked_metadata_charge(
            canonical_json(workspace)?.len(),
            canonical_json(&reserved)?.len(),
        )
    }

    /// Only the changed record is serialized; unrelated retained manifests are
    /// not rescanned on the write path. The admission token is installed only
    /// after durable success, or while reconstructing startup state.
    pub(crate) fn check_workspace_budget(
        &self,
        candidate: &Workspace,
    ) -> Result<WorkspaceAdmission> {
        let limits = self.effective_workspace_limits();
        let budget = &self.workspace_budget;
        let old = budget.records.get(&candidate.workspace_id);
        if old.is_some_and(|(owner, _)| owner != &candidate.principal) {
            return Err(Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "retained workspace owner cannot change",
            ));
        }
        let charge = self.workspace_metadata_charge(candidate)?;
        let old_charge = old.map_or(0, |(_, charge)| *charge);
        let added = usize::from(old.is_none());
        let (principal_count, principal_bytes) = budget
            .principals
            .get(&candidate.principal)
            .copied()
            .unwrap_or_default();
        let bytes = budget
            .bytes
            .checked_sub(old_charge)
            .and_then(|bytes| bytes.checked_add(charge));
        let principal_bytes = principal_bytes
            .checked_sub(old_charge)
            .and_then(|bytes| bytes.checked_add(charge));
        let exhausted = if budget.records.len().saturating_add(added)
            > limits.max_retained_workspaces
        {
            Some("global record count")
        } else if principal_count.saturating_add(added)
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
                    "retained workspace evidence {dimension} budget exhausted; terminal records still count and recovery evidence is preserved"
                ),
            ));
        }
        Ok(WorkspaceAdmission {
            id: candidate.workspace_id.clone(),
            principal: candidate.principal.clone(),
            charge,
        })
    }

    pub(crate) fn account_workspace(&mut self, admission: WorkspaceAdmission) {
        let budget = &mut self.workspace_budget;
        let old = budget.records.insert(
            admission.id,
            (admission.principal.clone(), admission.charge),
        );
        let old_charge = old.as_ref().map_or(0, |(_, charge)| *charge);
        budget.bytes = budget.bytes - old_charge + admission.charge;
        let principal = budget.principals.entry(admission.principal).or_default();
        principal.0 += usize::from(old.is_none());
        principal.1 = principal.1 - old_charge + admission.charge;
    }

    pub(crate) fn reset_workspace_budget(&mut self) {
        self.workspace_budget = WorkspaceBudget::default();
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/workspace_budget_helpers.rs"]
mod tests;

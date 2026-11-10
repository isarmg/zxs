//! Metadata-only SMB rebinding retains every admitted occurrence and I/O fence.
//! The permanent native receipt and closed State write set are rechecked before
//! the unchanged-namespace fast path, including replay without change tracking.
use super::*;
use crate::publication_shape::Table;
use crate::{
    Branch, CommitState, CurrentHealth, Event, OperationResult, ProtectionAtPublish, Repository,
};
use json_codec::{Value, json};
use storage_contract::{MapChanges, TrackedMap};

const KIND: &str = "smb.binding.update";

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "SMB binding publication differs from its metadata-only native request",
    )
}

impl CurrentNamespaces {
    /// Disabled and detached owners can be configured, but neither this record
    /// nor a successful binding changes their current entries or writable bit.
    pub(crate) fn smb_binding_source(
        &self,
        state: &State,
        repository: &Repository,
        branch: &Branch,
    ) -> Result<Value> {
        let owner = state
            .branches
            .get(&repository.main_branch_id)
            .ok_or_else(invalid)?;
        let history = state
            .histories
            .get(&repository.history_tree_id)
            .ok_or_else(invalid)?;
        if state.repositories.get(&repository.repository_id) != Some(repository)
            || state.branches.get(&branch.branch_id) != Some(branch)
            || history.repository_id != repository.repository_id
            || history.main_branch_id.as_deref() != Some(owner.branch_id.as_str())
            || owner.history_tree_id != repository.history_tree_id
            || branch.history_tree_id != repository.history_tree_id
            || branch.state != BranchState::Active
            || branch.file_id.is_some()
        {
            return Err(invalid());
        }
        self.branch_entries(&owner.branch_id, &owner.head_commit_id)?;
        self.branch_entries(&branch.branch_id, &branch.head_commit_id)?;
        Ok(json!({
            "registry": self.registry,
            "mask_hash": self.mask_hash,
            "owner_branch_id": owner.branch_id,
            "owner_head_commit_id": owner.head_commit_id,
            "target_head_commit_id": branch.head_commit_id
        }))
    }

    pub(in crate::engine) fn verify_new_smb_binding(
        &self,
        previous: &State,
        next: &State,
        result: &OperationResult,
    ) -> Result<()> {
        let receipt =
            publication_delta::receipt(previous, next, |receipt| receipt.kind == KIND, invalid)?
                .ok_or_else(invalid)?;
        if receipt != result {
            return Err(invalid());
        }
        self.verify_smb_binding(previous, next, receipt)
    }

    pub(super) fn verify_smb_bindings(&self, previous: &State, next: &State) -> Result<()> {
        let receipt =
            publication_delta::receipt(previous, next, |receipt| receipt.kind == KIND, invalid)?;
        let Some(receipt) = receipt else {
            // A forged kind or missing receipt cannot evade the proof by taking
            // the namespace fast path. Other admitted operations do not rebind
            // existing repository/history mains, history generations or SMB revisions.
            if binding_changed(previous, next) {
                return Err(invalid());
            }
            return Ok(());
        };
        self.verify_smb_binding(previous, next, receipt)
    }

    fn verify_smb_binding(
        &self,
        previous: &State,
        next: &State,
        receipt: &OperationResult,
    ) -> Result<()> {
        let repository_id = receipt.result["repository_id"]
            .as_str()
            .ok_or_else(invalid)?;
        let branch_id = receipt.result["branch_id"].as_str().ok_or_else(invalid)?;
        let enabled = receipt.result["smb_enabled"]
            .as_bool()
            .ok_or_else(invalid)?;
        let repository = previous
            .repositories
            .get(repository_id)
            .ok_or_else(invalid)?;
        let history = previous
            .histories
            .get(&repository.history_tree_id)
            .ok_or_else(invalid)?;
        let branch = previous.branches.get(branch_id).ok_or_else(invalid)?;
        let principal = next
            .operation_principals
            .get(&receipt.operation_id)
            .ok_or_else(invalid)?;
        Engine::require_actor(principal)?;
        let digest = Engine::native_request_digest(
            KIND,
            principal,
            &json!({
                "repository_id": repository_id,
                "branch_id": branch_id,
                "enabled": enabled,
                "expected_generation": history.generation.to_string()
            }),
        )?;
        let mut expected_repository = repository.clone();
        expected_repository.main_branch_id = branch_id.into();
        expected_repository.smb_enabled = enabled;
        expected_repository.smb_binding_generation = repository
            .smb_binding_generation
            .checked_add(1)
            .ok_or_else(invalid)?;
        let mut expected_history = history.clone();
        expected_history.main_branch_id = Some(branch_id.into());
        expected_history.generation = history.generation.checked_add(1).ok_or_else(invalid)?;
        let expected_result = json!({
            "repository_id": repository_id,
            "branch_id": branch_id,
            "smb_enabled": enabled,
            "generation": expected_history.generation.to_string(),
            "smb_binding_generation": expected_repository.smb_binding_generation.to_string(),
            "namespace_binding": self.smb_binding_source(previous, repository, branch)?,
            "timestamp": decimal_field(&receipt.result, "timestamp")?.to_string(),
            "branch_generation": branch.generation.to_string(),
            "membership_epoch": decimal_field(&receipt.result, "membership_epoch")?.to_string(),
            "version_protection": []
        });
        let expected_receipt = OperationResult {
            operation_id: receipt.operation_id,
            kind: KIND.into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: Some(branch_id.into()),
            commit_state: CommitState::Published,
            protection_at_publish: ProtectionAtPublish::NotApplicable,
            current_health: CurrentHealth::Healthy,
            request_digest: digest,
            sequence: next.sequence,
            result: expected_result,
        };
        // file_names/file_locations also drive physical container paths. Published
        // States already captured them; a cold decode loses tracking, not these values.
        // Rebinding must not repair missing names from a raw, possibly hidden HEAD.
        if receipt != &expected_receipt
            || next.sequence <= previous.sequence
            || previous.business_generation.checked_add(1) != Some(next.business_generation)
            || !only_changed(
                &previous.repositories,
                &next.repositories,
                repository_id,
                &expected_repository,
            )
            || !only_changed(
                &previous.histories,
                &next.histories,
                &history.history_tree_id,
                &expected_history,
            )
            || !publication_delta::only_added(
                &previous.operations,
                &next.operations,
                receipt.operation_id,
                receipt,
            )
            || !publication_delta::only_added(
                &previous.operation_principals,
                &next.operation_principals,
                receipt.operation_id,
                principal,
            )
            || !one_binding_event(previous, next, receipt)
            || !crate::publication_shape::unchanged_except(
                previous,
                next,
                &[Table::Repositories, Table::Histories],
            )
            || next.topology_generation != previous.topology_generation
            || next.control_auth != previous.control_auth
        {
            return Err(invalid());
        }
        Ok(())
    }
}

fn decimal_field(value: &Value, key: &str) -> Result<u64> {
    let value = value[key].as_str().ok_or_else(invalid)?;
    Ok(storage_contract::parse_u64_decimal(value)?)
}

fn binding_changed(previous: &State, next: &State) -> bool {
    let changed_repository = |id: &String| {
        previous.repositories.get(id).is_some_and(|before| {
            next.repositories.get(id).is_none_or(|after| {
                before.main_branch_id != after.main_branch_id
                    || before.smb_enabled != after.smb_enabled
                    || before.smb_binding_generation != after.smb_binding_generation
            })
        })
    };
    let changed_history = |id: &String| {
        previous.histories.get(id).is_some_and(|before| {
            next.histories.get(id).is_none_or(|after| {
                before.main_branch_id != after.main_branch_id
                    || before.generation != after.generation
            })
        })
    };
    let repositories = match next.repositories.changes_since(&previous.repositories) {
        MapChanges::Keys(keys) => keys.iter().any(changed_repository),
        MapChanges::All => previous.repositories.keys().any(changed_repository),
    };
    repositories
        || match next.histories.changes_since(&previous.histories) {
            MapChanges::Keys(keys) => keys.iter().any(changed_history),
            MapChanges::All => previous.histories.keys().any(changed_history),
        }
}

fn only_changed<T: PartialEq>(
    before: &TrackedMap<T>,
    after: &TrackedMap<T>,
    key: &str,
    expected: &T,
) -> bool {
    if before.len() != after.len() || after.get(key) != Some(expected) {
        return false;
    }
    match after.changes_since(before) {
        MapChanges::Keys(keys) => keys
            .iter()
            .all(|id| id == key || after.get(id) == before.get(id)),
        MapChanges::All => before
            .iter()
            .all(|(id, value)| id == key || after.get(id) == Some(value)),
    }
}

fn one_binding_event(previous: &State, next: &State, receipt: &OperationResult) -> bool {
    let expected = Event {
        sequence: next.sequence,
        operation_id: receipt.operation_id,
        kind: KIND.into(),
        object_id: receipt.operation_id.to_string(),
    };
    publication_delta::one_event(previous, next, &expected)
}

// One exhaustive State pattern owns the closed write set. Adding a field to
// State forces an explicit choice here; no namespace, placement, physical owner,
// authentication or historical record is implicitly authorized by a rebind.
#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_smb_binding_tests.rs"]
mod tests;

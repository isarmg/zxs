//! Explicit directory commits have a closed metadata plan, not a general permit.
//! Publication and cold replay reconstruct the same exact source-bound changes.
use super::*;
use crate::business::directory_staging::{CommitInput, apply_commit};
use crate::{CommitState, CurrentHealth, Event, OperationId, OperationResult, ProtectionAtPublish};
use json_codec::json;
use storage_contract::MapChanges;

fn accepts(kind: &str) -> bool {
    matches!(
        kind,
        "directory.changes.commit"
            | "directory.git.initialize"
            | "directory.git.stage-child"
            | "directory.git.stage-owned"
            | "directory.git.branch-create"
            | "directory.git.checkout"
    )
}

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "directory commit differs from its authenticated input and closed current occurrence graph",
    )
}

impl CurrentNamespaces {
    pub(in crate::engine) fn verify_new_directory_commit(
        &self,
        previous: &State,
        next: &State,
        result: &OperationResult,
    ) -> Result<()> {
        let receipt = new_receipt(previous, next)?.ok_or_else(invalid)?;
        if receipt != result {
            return Err(invalid());
        }
        self.verify_directory_commit(previous, next, receipt)
    }

    pub(super) fn verify_directory_commits(&self, previous: &State, next: &State) -> Result<()> {
        match new_receipt(previous, next)? {
            Some(receipt) => self.verify_directory_commit(previous, next, receipt),
            None if marked_history_changed(previous, next)
                || previous.directory_git_repositories != next.directory_git_repositories =>
            {
                Err(invalid())
            }
            None => Ok(()),
        }
    }

    fn verify_directory_commit(
        &self,
        previous: &State,
        next: &State,
        receipt: &OperationResult,
    ) -> Result<()> {
        let input = receipt.result["input"].clone();
        let principal = next
            .operation_principals
            .get(&receipt.operation_id)
            .ok_or_else(invalid)?;
        Engine::require_actor(principal)?;
        let digest = Engine::native_request_digest(&receipt.kind, principal, &input)?;
        let mut expected = previous.clone();
        expected.begin_tracking();
        crate::container_storage::capture_names(&mut expected);
        expected.sequence = next.sequence;
        expected.business_generation = previous
            .business_generation
            .checked_add(1)
            .ok_or_else(invalid)?;
        if next.sequence <= previous.sequence {
            return Err(invalid());
        }
        let mut result = if receipt.kind == "directory.changes.commit" {
            let commit: CommitInput = json_codec::from_value(input.clone()).map_err(|source| {
                Error::from(source).context(
                    ErrorCode::PublicationInvalid,
                    "directory commit input is invalid",
                )
            })?;
            apply_commit(
                &mut expected,
                Some(self),
                &commit,
                principal,
                receipt.operation_id,
            )?
        } else {
            crate::business::directory_git::apply_command(
                &mut expected,
                Some(self),
                &receipt.kind,
                &input,
                principal,
                receipt.operation_id,
            )?
        };
        crate::container_storage::capture_changed_names(previous, &mut expected);
        for key in ["timestamp", "membership_epoch"] {
            let value = receipt.result[key].as_str().ok_or_else(invalid)?;
            storage_contract::parse_u64_decimal(value).map_err(|source| {
                Error::from(source).context(
                    ErrorCode::PublicationInvalid,
                    "directory commit envelope decimal is invalid",
                )
            })?;
            result[key] = json!(value);
        }
        if let Some(branch) = result["branch_id"]
            .as_str()
            .and_then(|id| expected.branches.get(id))
        {
            result["branch_generation"] = json!(branch.generation.to_string());
        }
        result["version_protection"] = json!([]);
        let operation = OperationResult {
            operation_id: receipt.operation_id,
            kind: receipt.kind.clone(),
            commit_id: result["commit_id"].as_str().map(str::to_owned),
            history_tree_id: result["history_tree_id"].as_str().map(str::to_owned),
            branch_id: result["branch_id"].as_str().map(str::to_owned),
            commit_state: CommitState::Published,
            protection_at_publish: ProtectionAtPublish::NotApplicable,
            current_health: CurrentHealth::Healthy,
            request_digest: digest,
            sequence: next.sequence,
            result,
        };
        if operation != *receipt {
            return Err(invalid());
        }
        expected.operations.insert(receipt.operation_id, operation);
        expected
            .operation_principals
            .insert(receipt.operation_id, principal.clone());
        expected.events.push(Event {
            sequence: next.sequence,
            operation_id: receipt.operation_id,
            kind: receipt.kind.clone(),
            object_id: receipt
                .commit_id
                .clone()
                .or_else(|| receipt.history_tree_id.clone())
                .unwrap_or_else(|| receipt.operation_id.to_string()),
        });
        // The comparison covers every State field, including unchanged bodies,
        // placements, old history, owners and receipts. Tracking is no authority.
        if expected != *next {
            return Err(invalid());
        }
        Ok(())
    }
}

fn new_receipt<'a>(previous: &State, next: &'a State) -> Result<Option<&'a OperationResult>> {
    let mut receipt = None;
    let mut inspect = |id: &OperationId| -> Result<()> {
        let before = previous.operations.get(id);
        let after = next.operations.get(id);
        if before.is_some_and(|operation| accepts(&operation.kind)) {
            if before != after
                || previous.operation_principals.get(id) != next.operation_principals.get(id)
            {
                return Err(invalid());
            }
        } else if let Some(after) = after.filter(|operation| accepts(&operation.kind))
            && (before.is_some() || after.operation_id != *id || receipt.replace(after).is_some())
        {
            return Err(invalid());
        }
        Ok(())
    };
    match next.operations.changes_since(&previous.operations) {
        MapChanges::Keys(keys) => keys.iter().try_for_each(&mut inspect)?,
        MapChanges::All => {
            for id in previous.operations.keys() {
                inspect(id)?;
            }
            for id in next
                .operations
                .keys()
                .filter(|id| !previous.operations.contains_key(*id))
            {
                inspect(id)?;
            }
        }
    }
    let owner_changed = |id: &OperationId| {
        previous
            .operations
            .get(id)
            .is_some_and(|operation| accepts(&operation.kind))
            && previous.operation_principals.get(id) != next.operation_principals.get(id)
    };
    if match next
        .operation_principals
        .changes_since(&previous.operation_principals)
    {
        MapChanges::Keys(keys) => keys.iter().any(owner_changed),
        MapChanges::All => previous.operations.keys().any(owner_changed),
    } {
        return Err(invalid());
    }
    Ok(receipt)
}

fn marked_history_changed(previous: &State, next: &State) -> bool {
    let changed = |id: &String| {
        let before = previous.commits.get(id);
        let after = next.commits.get(id);
        before != after
            && (before.is_some_and(|commit| commit.kind == "directory_commit")
                || after.is_some_and(|commit| commit.kind == "directory_commit"))
    };
    let commit_changed = match next.commits.changes_since(&previous.commits) {
        MapChanges::Keys(keys) => keys.iter().any(changed),
        MapChanges::All => previous
            .commits
            .keys()
            .chain(next.commits.keys())
            .any(changed),
    };
    let changed = |id: &String| {
        let before = previous.directory_trees.get(id);
        let after = next.directory_trees.get(id);
        before != after
            && (before.is_some_and(|tree| {
                storage_contract::directory_staging::directory_tree_is_committed(previous, tree)
            }) || after.is_some_and(|tree| {
                storage_contract::directory_staging::directory_tree_is_committed(next, tree)
            }))
    };
    commit_changed
        || match next
            .directory_trees
            .changes_since(&previous.directory_trees)
        {
            MapChanges::Keys(keys) => keys.iter().any(changed),
            MapChanges::All => previous
                .directory_trees
                .keys()
                .chain(next.directory_trees.keys())
                .any(changed),
        }
}

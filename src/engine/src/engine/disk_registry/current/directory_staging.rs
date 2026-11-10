//! Explicit directory commits have a closed metadata plan, not a general permit.
//! Publication and cold replay reconstruct the same exact source-bound changes.
use super::*;
use crate::business::{
    directory_git::staged_owned_files,
    directory_staging::{CommitInput, apply_commit},
};
use crate::{
    CommitState, ContentState, CurrentHealth, DirectoryTree, Event, OperationResult,
    ProtectionAtPublish,
};
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
    /// A staged owned snapshot may differ from the live working tree, including
    /// after a working deletion. Its bodies still need current availability.
    /// Explicit child commits are historical references and are checked by the
    /// Git marker closure, not by current child HEAD or body availability.
    pub(crate) fn require_staged_commit_source(
        &self,
        state: &State,
        staged: &DirectoryTree,
        boundaries: &BTreeMap<String, String>,
    ) -> Result<()> {
        for entry in staged_owned_files(staged, boundaries) {
            if self.unavailable_versions.contains(&entry.version_id)
                || state.content_states.get(&entry.version_id) != Some(&ContentState::Available)
            {
                return Err(Error::new(
                    ErrorCode::DirectoryBranchConflict,
                    "staged directory content is unavailable; stage this repository again before committing",
                ));
            }
        }
        Ok(())
    }

    pub(in crate::engine) fn verify_new_directory_commit(
        &self,
        previous: &State,
        next: &State,
        result: &OperationResult,
    ) -> Result<()> {
        let receipt =
            publication_delta::receipt(previous, next, |receipt| accepts(&receipt.kind), invalid)?
                .ok_or_else(invalid)?;
        if receipt != result {
            return Err(invalid());
        }
        self.verify_directory_commit(previous, next, receipt)
    }

    pub(super) fn verify_directory_commits(&self, previous: &State, next: &State) -> Result<()> {
        match publication_delta::receipt(previous, next, |receipt| accepts(&receipt.kind), invalid)?
        {
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
        if !crate::publication_shape::same_transition(previous, &expected, next) {
            return Err(invalid());
        }
        Ok(())
    }
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

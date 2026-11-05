//! Branch archival changes one state/generation pair, never an owned namespace.
//! Reconstruct its permanent request before the unchanged-head fast path, both
//! in live publication and when replay has lost transaction-local tracking.
use super::*;
use crate::{
    Branch, CommitState, CurrentHealth, Event, OperationId, OperationResult, ProtectionAtPublish,
};
use json_codec::{Value, json};
use storage_contract::{MapChanges, SequenceChanges};

const KIND: &str = "branch.archive";

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "branch archival differs from its authenticated metadata-only request",
    )
}

fn is_archive(receipt: &OperationResult) -> bool {
    receipt.kind == KIND || receipt.result.get("namespace_archive").is_some()
}

impl CurrentNamespaces {
    /// An archive needs the admitted scope identity, not a writable body route.
    /// Disabled and detached scopes stay exactly as projected before archival.
    pub(crate) fn branch_archive_source(&self, state: &State, id: &str) -> Result<Value> {
        let branch = state.branches.get(id).ok_or_else(invalid)?;
        let scope = self.branches.get(id).ok_or_else(stale)?;
        let head = state
            .commits
            .get(&branch.head_commit_id)
            .ok_or_else(invalid)?;
        let fork = state
            .commits
            .get(&branch.fork_commit_id)
            .ok_or_else(invalid)?;
        let history = state
            .histories
            .get(&branch.history_tree_id)
            .ok_or_else(invalid)?;
        let repository = state
            .repositories
            .get(&history.repository_id)
            .ok_or_else(invalid)?;
        if branch.branch_id != id
            || scope.head != branch.head_commit_id
            || head.commit_id != branch.head_commit_id
            || head.history_tree_id != branch.history_tree_id
            || fork.commit_id != branch.fork_commit_id
            || fork.history_tree_id != branch.history_tree_id
            || history.history_tree_id != branch.history_tree_id
            || repository.repository_id != history.repository_id
            || repository.history_tree_id != branch.history_tree_id
        {
            return Err(invalid());
        }
        Ok(json!({
            "registry": self.registry,
            "activation_sequence": self.activation_sequence.to_string(),
            "mask_hash": self.mask_hash,
            "repository_id": repository.repository_id,
            "branch_id": id,
            "head_commit_id": branch.head_commit_id,
            "branch_generation": branch.generation.to_string(),
            "previous_state": branch.state,
        }))
    }

    pub(in crate::engine) fn verify_new_branch_archive(
        &self,
        previous: &State,
        next: &State,
        result: &OperationResult,
    ) -> Result<()> {
        let receipt = new_archive_receipt(previous, next)?.ok_or_else(invalid)?;
        if receipt != result {
            return Err(invalid());
        }
        self.verify_branch_archive(previous, next, receipt)
    }

    pub(super) fn verify_branch_archives(&self, previous: &State, next: &State) -> Result<()> {
        if let Some(receipt) = new_archive_receipt(previous, next)? {
            return self.verify_branch_archive(previous, next, receipt);
        }
        // Missing or retagged receipts cannot hide archival, including a
        // second archive of the same branch. No admitted operation unarchives it.
        let archival_changed = |id: &String| {
            let before = previous.branches.get(id);
            let after = next.branches.get(id);
            before != after
                && before
                    .into_iter()
                    .chain(after)
                    .any(|branch| branch.state == BranchState::Archived)
        };
        let missing_receipt = match next.branches.changes_since(&previous.branches) {
            MapChanges::Keys(ids) => ids.iter().any(archival_changed),
            MapChanges::All => previous
                .branches
                .keys()
                .chain(next.branches.keys())
                .any(archival_changed),
        };
        if missing_receipt {
            return Err(invalid());
        }
        Ok(())
    }

    fn verify_branch_archive(
        &self,
        previous: &State,
        next: &State,
        receipt: &OperationResult,
    ) -> Result<()> {
        let id = receipt.operation_id;
        let branch_id = receipt.branch_id.as_deref().ok_or_else(invalid)?;
        let before = previous.branches.get(branch_id).ok_or_else(invalid)?;
        let after = next.branches.get(branch_id).ok_or_else(invalid)?;
        verify_archive_eligibility(previous, before)?;
        let principal = next.operation_principals.get(&id).ok_or_else(invalid)?;
        Engine::require_actor(principal).map_err(|error| {
            error.context(
                ErrorCode::PublicationInvalid,
                "branch archive actor is invalid",
            )
        })?;
        let expected = OperationResult {
            operation_id: id,
            kind: KIND.into(),
            commit_id: None,
            history_tree_id: Some(before.history_tree_id.clone()),
            branch_id: Some(branch_id.into()),
            commit_state: CommitState::Published,
            protection_at_publish: ProtectionAtPublish::NotApplicable,
            current_health: CurrentHealth::Healthy,
            request_digest: Engine::native_request_digest(
                KIND,
                principal,
                &json!({"branch_id": branch_id}),
            )?,
            sequence: next.sequence,
            result: json!({
                "branch_id": branch_id,
                "history_tree_id": before.history_tree_id,
                "branch_generation": after.generation.to_string(),
                "namespace_archive": self.branch_archive_source(previous, branch_id)?,
                "timestamp": decimal_field(&receipt.result, "timestamp")?.to_string(),
                "membership_epoch": decimal_field(&receipt.result, "membership_epoch")?.to_string(),
                "version_protection": [],
            }),
        };
        if receipt != &expected || !archived_branch_matches(before, after) {
            return Err(invalid());
        }
        verify_write_set(previous, next, before, id)
    }
}

fn decimal_field(value: &Value, key: &str) -> Result<u64> {
    storage_contract::parse_u64_decimal(value[key].as_str().ok_or_else(invalid)?).map_err(|error| {
        Error::from(error).context(
            ErrorCode::PublicationInvalid,
            "branch archive envelope has an invalid decimal field",
        )
    })
}

/// Tracking is only an optimization. Persisted States compare the complete
/// relevant maps, and independent owner/event deltas cannot hide behind HEAD.
fn new_archive_receipt<'a>(
    previous: &State,
    next: &'a State,
) -> Result<Option<&'a OperationResult>> {
    let mut receipt = None;
    let mut inspect = |id: &OperationId| -> Result<()> {
        let before = previous.operations.get(id);
        let after = next.operations.get(id);
        if before.is_some_and(is_archive) {
            if before != after {
                return Err(invalid());
            }
        } else if let Some(after) = after.filter(|after| is_archive(after))
            && (before.is_some() || after.operation_id != *id || receipt.replace(after).is_some())
        {
            return Err(invalid());
        }
        Ok(())
    };
    match next.operations.changes_since(&previous.operations) {
        MapChanges::Keys(ids) => ids.iter().try_for_each(&mut inspect)?,
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
    let preserved_owner = |id: &OperationId| {
        previous.operations.get(id).is_none_or(|old| {
            !is_archive(old)
                || previous.operation_principals.get(id) == next.operation_principals.get(id)
        })
    };
    let owners_unchanged = match next
        .operation_principals
        .changes_since(&previous.operation_principals)
    {
        MapChanges::Keys(ids) => ids.iter().all(preserved_owner),
        MapChanges::All => previous.operations.keys().all(preserved_owner),
    };
    if !owners_unchanged {
        return Err(invalid());
    }
    let start = match next.events.changes_since(&previous.events) {
        SequenceChanges::Append { start } => start,
        SequenceChanges::All => 0,
    };
    for index in start..previous.events.len().max(next.events.len()) {
        let before = previous.events.get(index);
        let after = next.events.get(index);
        if before != after
            && (before.is_some_and(|event| event.kind == KIND)
                || after.is_some_and(|event| {
                    event.kind == KIND
                        && (before.is_some()
                            || Some(event.operation_id)
                                != receipt.map(|receipt| receipt.operation_id))
                }))
        {
            return Err(invalid());
        }
    }
    Ok(receipt)
}

fn verify_archive_eligibility(state: &State, branch: &Branch) -> Result<()> {
    let history = state
        .histories
        .get(&branch.history_tree_id)
        .ok_or_else(invalid)?;
    let repository = state
        .repositories
        .get(&history.repository_id)
        .ok_or_else(invalid)?;
    if repository.main_branch_id == branch.branch_id
        || state
            .file_selections
            .values()
            .any(|selected| selected == &branch.branch_id)
        || state
            .file_contexts
            .values()
            .any(|context| context.branch_id == branch.branch_id)
        // The immutable merge history requires consumed names to remain Merged
        // and every final alias owner to remain Active. Archival cannot retarget
        // or delete those edges as a side effect of changing one branch state.
        || state.file_branch_aliases.contains_key(&branch.branch_id)
        || state
            .file_branch_aliases
            .values()
            .any(|target| target == &branch.branch_id)
    {
        return Err(invalid());
    }
    Ok(())
}

fn archived_branch_matches(before: &Branch, after: &Branch) -> bool {
    let Branch {
        branch_id,
        history_tree_id,
        name,
        head_commit_id,
        fork_commit_id,
        branch_kind,
        file_id,
        file_node_id,
        pending_file_versions,
        generation,
        state: _,
    } = before;
    branch_id == &after.branch_id
        && history_tree_id == &after.history_tree_id
        && name == &after.name
        && head_commit_id == &after.head_commit_id
        && fork_commit_id == &after.fork_commit_id
        && branch_kind == &after.branch_kind
        && file_id == &after.file_id
        && file_node_id == &after.file_node_id
        && pending_file_versions == &after.pending_file_versions
        && generation.checked_add(1) == Some(after.generation)
        && after.state == BranchState::Archived
}

fn verify_write_set(
    previous: &State,
    next: &State,
    branch: &Branch,
    id: OperationId,
) -> Result<()> {
    // Exhaustive State destructuring keeps additions an explicit write-set
    // decision. Valid provenance avoids comparing unrelated historical records.
    macro_rules! preserve {
        ($($field:ident),* $(,)?) => {{
            let State {
                sequence: _, business_generation: _, topology_generation: _,
                control_auth: _, branches: _, operations: _, operation_principals: _,
                events: _, $($field,)*
            } = previous;
            $(let unchanged = match next.$field.changes_since($field) {
                MapChanges::Keys(keys) => keys.iter().all(|key| next.$field.get(key) == $field.get(key)),
                MapChanges::All => &next.$field == $field,
            };
            if !unchanged { return Err(invalid()); })*
        }};
    }
    preserve!(
        repositories,
        histories,
        file_names,
        file_locations,
        file_selections,
        directory_branches,
        directory_trees,
        file_write_origins,
        file_contexts,
        file_path_nodes,
        file_metadata_nodes,
        file_branch_merges,
        file_branch_aliases,
        file_node_parents,
        file_path_notes,
        directory_tree_selections,
        file_backup_paths,
        file_backup_policies,
        directory_selections,
        directory_selection_generations,
        file_selection_generations,
        file_notes,
        version_notes,
        commits,
        versions,
        placements,
        merge_links,
        history_boundaries,
        content_states,
        plans,
        placement_rules,
        protection_policies,
    );
    macro_rules! only_change {
        ($field:ident, $key:expr) => {{
            let same = |key| key == $key || previous.$field.get(key) == next.$field.get(key);
            match next.$field.changes_since(&previous.$field) {
                MapChanges::Keys(keys) => keys.iter().all(same),
                MapChanges::All => previous.$field.keys().chain(next.$field.keys()).all(same),
            }
        }};
    }
    if next.sequence <= previous.sequence
        || previous.business_generation.checked_add(1) != Some(next.business_generation)
        || next.topology_generation != previous.topology_generation
        || next.control_auth != previous.control_auth
        || next.branches.len() != previous.branches.len()
        || !only_change!(branches, &branch.branch_id)
        || previous.operations.len().checked_add(1) != Some(next.operations.len())
        || previous.operations.contains_key(&id)
        || !only_change!(operations, &id)
        || previous.operation_principals.len().checked_add(1)
            != Some(next.operation_principals.len())
        || previous.operation_principals.contains_key(&id)
        || !only_change!(operation_principals, &id)
    {
        return Err(invalid());
    }
    verify_event(previous, next, branch, id)
}

fn verify_event(previous: &State, next: &State, branch: &Branch, id: OperationId) -> Result<()> {
    if previous.events.len().checked_add(1) != Some(next.events.len()) {
        return Err(invalid());
    }
    match next.events.changes_since(&previous.events) {
        SequenceChanges::Append { start } if start == previous.events.len() => {}
        SequenceChanges::Append { .. } | SequenceChanges::All => {
            if next.events[..previous.events.len()] != previous.events[..] {
                return Err(invalid());
            }
        }
    }
    let expected = Event {
        sequence: next.sequence,
        operation_id: id,
        kind: KIND.into(),
        object_id: branch.history_tree_id.clone(),
    };
    if next.events.last() != Some(&expected) {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_branch_archive_tests.rs"]
mod tests;

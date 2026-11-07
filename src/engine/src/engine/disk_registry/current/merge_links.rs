//! A relation links two frozen history nodes without granting body authority.
//! Prove its native request and closed write set before the unchanged-head fast
//! path, including authenticated cold replay without transaction-local tracking.
use super::*;
use crate::{
    Branch, CommitState, CurrentHealth, Event, MergeLink, OperationId, OperationResult,
    ProtectionAtPublish,
};
use json_codec::{Value, json};
use storage_contract::{MapChanges, SequenceChanges};

const KIND: &str = "merge-link.create";

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "merge link differs from its authenticated metadata-only request",
    )
}

fn is_link(receipt: &OperationResult) -> bool {
    receipt.kind == KIND || receipt.result.get("namespace_merge_link").is_some()
}

impl CurrentNamespaces {
    /// Historical endpoints need an admitted scope, not writable content. A
    /// removed or disabled body remains unavailable after recording a relation.
    pub(crate) fn merge_link_source(&self, state: &State, link: &MergeLink) -> Result<Value> {
        let source = state
            .branches
            .get(&link.source_branch_id)
            .ok_or_else(invalid)?;
        let target = state
            .branches
            .get(&link.target_branch_id)
            .ok_or_else(invalid)?;
        let history = state
            .histories
            .get(&link.history_tree_id)
            .ok_or_else(invalid)?;
        let repository = state
            .repositories
            .get(&history.repository_id)
            .ok_or_else(invalid)?;
        if source.branch_id == target.branch_id
            || source.history_tree_id != link.history_tree_id
            || target.history_tree_id != link.history_tree_id
            || source.file_id != target.file_id
            || history.history_tree_id != link.history_tree_id
            || repository.repository_id != history.repository_id
            || repository.history_tree_id != link.history_tree_id
        {
            return Err(invalid());
        }
        Ok(json!({
            "registry": self.registry,
            "activation_sequence": self.activation_sequence.to_string(),
            "mask_hash": self.mask_hash,
            "repository_id": repository.repository_id,
            "history_tree_id": link.history_tree_id,
            "source": self.link_endpoint_source(state, source, &link.source_branch_id, &link.source_commit_id)?,
            "target": self.link_endpoint_source(state, target, &link.target_branch_id, &link.target_commit_id)?,
        }))
    }

    fn link_endpoint_source(
        &self,
        state: &State,
        branch: &Branch,
        branch_id: &str,
        selected: &str,
    ) -> Result<Value> {
        let scope = self.branches.get(branch_id).ok_or_else(stale)?;
        let fork = state
            .commits
            .get(&branch.fork_commit_id)
            .ok_or_else(invalid)?;
        if branch.branch_id != branch_id
            || scope.head != branch.head_commit_id
            || branch.state == BranchState::Migrated
            || fork.commit_id != branch.fork_commit_id
            || fork.history_tree_id != branch.history_tree_id
        {
            return Err(invalid());
        }
        // Match the native first-parent membership rule. These are persisted
        // source records at replay, so reject incomplete, rebound or cyclic
        // ancestry rather than relying on the live caller's State invariants.
        let mut cursor = branch.head_commit_id.as_str();
        let mut visited = BTreeSet::new();
        loop {
            let commit = state.commits.get(cursor).ok_or_else(invalid)?;
            if !visited.insert(cursor)
                || commit.commit_id != cursor
                || commit.history_tree_id != branch.history_tree_id
            {
                return Err(invalid());
            }
            if cursor == selected {
                break;
            }
            cursor = commit.parent_commit_ids.first().ok_or_else(invalid)?;
        }
        Ok(json!({
            "branch_id": branch_id,
            "head_commit_id": branch.head_commit_id,
            "fork_commit_id": branch.fork_commit_id,
            "branch_generation": branch.generation.to_string(),
            "branch_state": branch.state,
            "branch_kind": branch.branch_kind,
            "file_id": branch.file_id,
            "file_node_id": branch.file_node_id,
            "selected_commit_id": selected,
        }))
    }

    pub(in crate::engine) fn verify_new_merge_link(
        &self,
        previous: &State,
        next: &State,
        result: &OperationResult,
    ) -> Result<()> {
        let receipt = new_link_receipt(previous, next)?.ok_or_else(invalid)?;
        if receipt != result {
            return Err(invalid());
        }
        self.verify_merge_link(previous, next, receipt)
    }

    pub(super) fn verify_merge_links(&self, previous: &State, next: &State) -> Result<()> {
        if let Some(receipt) = new_link_receipt(previous, next)? {
            return self.verify_merge_link(previous, next, receipt);
        }
        // No other admitted operation remaps relations. A missing or retagged
        // receipt must not turn link creation, replacement or deletion into an
        // unchanged-head transition. Future split admission needs its own proof.
        let changed = |id: &String| previous.merge_links.get(id) != next.merge_links.get(id);
        let changed = match next.merge_links.changes_since(&previous.merge_links) {
            MapChanges::Keys(ids) => ids.iter().any(changed),
            MapChanges::All => previous
                .merge_links
                .keys()
                .chain(next.merge_links.keys())
                .any(changed),
        };
        if changed {
            return Err(invalid());
        }
        Ok(())
    }

    fn verify_merge_link(
        &self,
        previous: &State,
        next: &State,
        receipt: &OperationResult,
    ) -> Result<()> {
        let id = receipt.operation_id;
        let link_id = receipt.result["merge_link_id"]
            .as_str()
            .ok_or_else(invalid)?;
        storage_contract::validate_id(link_id, "link").map_err(|error| {
            Error::from(error).context(
                ErrorCode::PublicationInvalid,
                "merge link identity is invalid",
            )
        })?;
        let link = next.merge_links.get(link_id).ok_or_else(invalid)?;
        let principal = next.operation_principals.get(&id).ok_or_else(invalid)?;
        Engine::require_actor(principal).map_err(|error| {
            error.context(ErrorCode::PublicationInvalid, "merge link actor is invalid")
        })?;
        if link.merge_link_id != link_id || link.operation_id != id || link.author != *principal {
            return Err(invalid());
        }
        let expected = OperationResult {
            operation_id: id,
            kind: KIND.into(),
            commit_id: None,
            history_tree_id: Some(link.history_tree_id.clone()),
            branch_id: None,
            commit_state: CommitState::Published,
            protection_at_publish: ProtectionAtPublish::NotApplicable,
            current_health: CurrentHealth::Healthy,
            request_digest: Engine::native_request_digest(
                KIND,
                principal,
                &json!({
                    "source_branch_id": link.source_branch_id,
                    "source_commit_id": link.source_commit_id,
                    "target_branch_id": link.target_branch_id,
                    "target_commit_id": link.target_commit_id,
                }),
            )?,
            sequence: next.sequence,
            result: json!({
                "merge_link_id": link_id,
                "history_tree_id": link.history_tree_id,
                "namespace_merge_link": self.merge_link_source(previous, link)?,
                "timestamp": decimal_field(&receipt.result, "timestamp")?.to_string(),
                "membership_epoch": decimal_field(&receipt.result, "membership_epoch")?.to_string(),
                "version_protection": [],
            }),
        };
        if receipt != &expected {
            return Err(invalid());
        }
        verify_write_set(previous, next, link, id)
    }
}

fn decimal_field(value: &Value, key: &str) -> Result<u64> {
    storage_contract::parse_u64_decimal(value[key].as_str().ok_or_else(invalid)?).map_err(|error| {
        Error::from(error).context(
            ErrorCode::PublicationInvalid,
            "merge link envelope has an invalid decimal field",
        )
    })
}

/// Tracking is only an optimization. Persisted States compare the complete
/// relevant maps, and independent owner/event deltas cannot hide behind HEAD.
fn new_link_receipt<'a>(previous: &State, next: &'a State) -> Result<Option<&'a OperationResult>> {
    let mut receipt = None;
    let mut inspect = |id: &OperationId| -> Result<()> {
        let before = previous.operations.get(id);
        let after = next.operations.get(id);
        if before.is_some_and(is_link) {
            if before != after {
                return Err(invalid());
            }
        } else if let Some(after) = after.filter(|after| is_link(after))
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
            !is_link(old)
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

fn verify_write_set(
    previous: &State,
    next: &State,
    link: &MergeLink,
    id: OperationId,
) -> Result<()> {
    // Exhaustive State destructuring keeps additions an explicit write-set
    // decision. Valid provenance avoids comparing unrelated historical records.
    macro_rules! preserve {
        ($($field:ident),* $(,)?) => {{
            let State {
                sequence: _, business_generation: _, topology_generation: _,
                control_auth: _, merge_links: _, operations: _, operation_principals: _,
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
        branches,
        repositories,
        histories,
        file_names,
        file_locations,
        file_selections,
        directory_branches,
        directory_git_repositories,
        directory_trees,
        file_write_origins,
        file_contexts,
        file_path_nodes,
        file_metadata_nodes,
        file_deletions,
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
        || previous.merge_links.len().checked_add(1) != Some(next.merge_links.len())
        || previous.merge_links.contains_key(&link.merge_link_id)
        || !only_change!(merge_links, &link.merge_link_id)
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
    verify_event(previous, next, link, id)
}

fn verify_event(previous: &State, next: &State, link: &MergeLink, id: OperationId) -> Result<()> {
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
        object_id: link.history_tree_id.clone(),
    };
    if next.events.last() != Some(&expected) {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_merge_link_tests.rs"]
mod tests;

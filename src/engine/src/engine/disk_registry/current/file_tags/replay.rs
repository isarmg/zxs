//! Rebuild exactly the authenticated tag request before any publication effects.
use super::*;
use storage_contract::{MapChanges, SequenceChanges};

fn is_tag(receipt: &OperationResult) -> bool {
    receipt.kind == KIND || receipt.result.get("namespace_tags").is_some()
}

impl CurrentNamespaces {
    pub(in crate::engine) fn verify_new_file_tags(
        &self,
        previous: &State,
        next: &State,
        result: &OperationResult,
    ) -> Result<()> {
        let receipt = new_tag_receipt(previous, next)?.ok_or_else(invalid)?;
        if receipt != result {
            return Err(invalid());
        }
        verify_metadata_sources(previous, next, Some(receipt.operation_id))?;
        self.verify_file_tags(previous, next, receipt)
    }

    pub(in crate::engine::disk_registry::current) fn verify_file_tag_publications(
        &self,
        previous: &State,
        next: &State,
    ) -> Result<()> {
        let receipt = new_tag_receipt(previous, next)?;
        verify_metadata_sources(previous, next, receipt.map(|receipt| receipt.operation_id))?;
        match receipt {
            Some(receipt) => self.verify_file_tags(previous, next, receipt),
            None => Ok(()),
        }
    }

    fn verify_file_tags(
        &self,
        previous: &State,
        next: &State,
        receipt: &OperationResult,
    ) -> Result<()> {
        let principal = next
            .operation_principals
            .get(&receipt.operation_id)
            .ok_or_else(invalid)?;
        Engine::require_actor(principal).map_err(|error| {
            error.context(ErrorCode::PublicationInvalid, "file tag actor is invalid")
        })?;
        let input: Input = json_codec::from_value(
            receipt
                .result
                .get("source_request")
                .ok_or_else(invalid)?
                .clone(),
        )
        .map_err(|error| {
            Error::from(error).context(ErrorCode::PublicationInvalid, "file tag request is invalid")
        })?;
        let created_at = decimal_field(&receipt.result, "metadata_created_at")?;
        // Live planning uses Config::max_entries. Replay only reconstructs rows
        // from bounded, admitted sources: a directory commit can concatenate its
        // projected descendants and owner ancestors, never invent more entries.
        let source_maximum = self
            .branches
            .values()
            .chain(self.directories.values())
            .map(|scope| scope.entries.len())
            .max()
            .unwrap_or(0);
        let maximum = source_maximum
            .checked_mul(2)
            .ok_or_else(invalid)?
            .max(input.targets.len());
        let plan = self
            .file_tag_plan(
                previous,
                &input,
                principal,
                TagPublication {
                    operation: receipt.operation_id,
                    sequence: next.sequence,
                    created_at,
                },
                maximum,
            )
            .map_err(|error| {
                error.context(
                    ErrorCode::PublicationInvalid,
                    "file tag source proof is invalid",
                )
            })?;
        let mut expected_result = plan.result.clone();
        expected_result["timestamp"] =
            json!(decimal_field(&receipt.result, "timestamp")?.to_string());
        expected_result["membership_epoch"] =
            json!(decimal_field(&receipt.result, "membership_epoch")?.to_string());
        expected_result["version_protection"] = json!([]);
        if let Some(id) = plan.result["branch_id"].as_str() {
            let branch = plan
                .branches
                .get(id)
                .and_then(Option::as_ref)
                .ok_or_else(invalid)?;
            expected_result["branch_generation"] = json!(branch.generation.to_string());
        }
        let expected = OperationResult {
            operation_id: receipt.operation_id,
            kind: KIND.into(),
            commit_id: plan.result["commit_id"].as_str().map(str::to_owned),
            history_tree_id: plan.result["history_tree_id"].as_str().map(str::to_owned),
            branch_id: plan.result["branch_id"].as_str().map(str::to_owned),
            commit_state: CommitState::Published,
            protection_at_publish: ProtectionAtPublish::NotApplicable,
            current_health: CurrentHealth::Healthy,
            request_digest: Engine::native_request_digest(KIND, principal, &input.body())?,
            sequence: next.sequence,
            result: expected_result,
        };
        if receipt != &expected {
            return Err(invalid());
        }
        plan.verify(previous, next, receipt, principal)
    }
}

// Metadata children are immutable. A merge provenance row is not enough to
// authorize a new child: its selected tags and actual branch adoption must also
// hold before advance can take the unchanged-head shortcut.
fn verify_metadata_sources(
    previous: &State,
    next: &State,
    tag_operation: Option<OperationId>,
) -> Result<()> {
    let inspect = |id: &String| -> Result<()> {
        let before = previous.file_metadata_nodes.get(id);
        let after = next.file_metadata_nodes.get(id);
        if before == after {
            return Ok(());
        }
        if before.is_some() {
            return Err(invalid());
        }
        let node = after.ok_or_else(invalid)?;
        if node.node_id != *id {
            return Err(invalid());
        }
        match node.kind {
            FileMetadataKind::Tags if Some(node.operation_id) == tag_operation => {}
            FileMetadataKind::Tags => return Err(invalid()),
            FileMetadataKind::Merge => verify_merge_child(previous, next, node)?,
        }
        Ok(())
    };
    match next
        .file_metadata_nodes
        .changes_since(&previous.file_metadata_nodes)
    {
        MapChanges::Keys(ids) => ids.iter().try_for_each(inspect),
        MapChanges::All => {
            for id in previous
                .file_metadata_nodes
                .keys()
                .chain(next.file_metadata_nodes.keys())
            {
                inspect(id)?;
            }
            Ok(())
        }
    }
}

fn verify_merge_child(previous: &State, next: &State, node: &FileMetadataNode) -> Result<()> {
    let merge = next
        .file_branch_merges
        .values()
        .find(|merge| {
            !previous.file_branch_merges.contains_key(&merge.merge_id)
                && !merge.reused_node
                && merge.result_node_id == node.node_id
                && merge.operation_id == node.operation_id
                && merge.surviving_branch_id == node.branch_id
                && merge.history_tree_id == node.history_tree_id
                && merge.file_id == node.file_id
                && merge.parent_node_ids == node.parent_node_ids
        })
        .ok_or_else(invalid)?;
    let receipt = next
        .operations
        .get(&node.operation_id)
        .ok_or_else(invalid)?;
    let principal = next
        .operation_principals
        .get(&node.operation_id)
        .ok_or_else(invalid)?;
    let branch = next.branches.get(&node.branch_id).ok_or_else(invalid)?;
    let commit = next
        .commits
        .get(&branch.head_commit_id)
        .ok_or_else(invalid)?;
    if previous.operations.contains_key(&node.operation_id)
        || !matches!(
            receipt.kind.as_str(),
            "file.branches.merge" | "file.history.merge"
        )
        || receipt.operation_id != node.operation_id
        || receipt.sequence != next.sequence
        || merge.sequence != next.sequence
        || node.sequence != next.sequence
        || node.author != *principal
        || branch.state != BranchState::Active
        || branch.file_node_id.as_deref() != Some(node.node_id.as_str())
        || previous
            .branches
            .get(&node.branch_id)
            .is_some_and(|old| old.head_commit_id == branch.head_commit_id)
        || previous.commits.contains_key(&branch.head_commit_id)
        || commit.kind != "file_metadata"
        || commit.provenance.as_deref() != Some(node.node_id.as_str())
        || commit.operation_id != node.operation_id
        || commit.sequence != next.sequence
        || commit.author != *principal
        || receipt.kind == "file.branches.merge"
            && merge.input_node_ids.contains(&merge.chosen_node_id)
    {
        return Err(invalid());
    }
    let version = checked_node_version(
        previous,
        &node.file_id,
        &merge.chosen_node_id,
        &node.history_tree_id,
    )
    .map_err(|source| source.context(ErrorCode::PublicationInvalid, "invalid file merge source"))?;
    if node.version_id != version
        || node.tags != crate::business::file_nodes::node_tags(previous, &merge.chosen_node_id)
    {
        return Err(invalid());
    }
    Ok(())
}

fn decimal_field(value: &Value, key: &str) -> Result<u64> {
    storage_contract::parse_u64_decimal(value[key].as_str().ok_or_else(invalid)?).map_err(|error| {
        Error::from(error).context(
            ErrorCode::PublicationInvalid,
            "file tags envelope has an invalid decimal field",
        )
    })
}

/// Tracking is only an optimization. Persisted States compare the complete
/// relevant maps, and independent owner/event deltas cannot hide behind HEAD.
fn new_tag_receipt<'a>(previous: &State, next: &'a State) -> Result<Option<&'a OperationResult>> {
    let mut receipt = None;
    let mut inspect = |id: &OperationId| -> Result<()> {
        let before = previous.operations.get(id);
        let after = next.operations.get(id);
        if before.is_some_and(is_tag) {
            if before != after {
                return Err(invalid());
            }
        } else if let Some(after) = after.filter(|after| is_tag(after))
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
            !is_tag(old)
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

impl Plan {
    fn verify(
        &self,
        previous: &State,
        next: &State,
        receipt: &OperationResult,
        principal: &str,
    ) -> Result<()> {
        macro_rules! verify_patch {
            ($field:ident, $patch:expr) => {{
                let patch = $patch;
                let changes = next.$field.changes_since(&previous.$field);
                let check = |id| -> Result<()> {
                    let expected = patch
                        .get(id)
                        .map(Option::as_ref)
                        .unwrap_or_else(|| previous.$field.get(id));
                    if next.$field.get(id) != expected {
                        return Err(invalid());
                    }
                    Ok(())
                };
                match &changes {
                    storage_contract::MapChanges::Keys(ids) => {
                        for id in ids.iter() {
                            check(id)?;
                        }
                    }
                    storage_contract::MapChanges::All => {
                        for id in previous.$field.keys().chain(next.$field.keys()) {
                            check(id)?;
                        }
                    }
                }
                for id in patch.keys() {
                    check(id)?;
                }
            }};
        }
        macro_rules! unchanged {
            ($($field:ident),* $(,)?) => { $(
                let same = match next.$field.changes_since(&previous.$field) {
                    MapChanges::Keys(ids) => ids.iter().all(|id| previous.$field.get(id) == next.$field.get(id)),
                    MapChanges::All => next.$field == previous.$field,
                };
                if !same { return Err(invalid()); }
            )* };
        }
        // Exhaustive coverage: adding a State field requires a write-set decision.
        let State {
            sequence: _,
            business_generation: _,
            topology_generation: _,
            control_auth: _,
            events: _,
            repositories: _,
            histories: _,
            branches: _,
            file_selections: _,
            directory_branches: _,
            directory_git_repositories: _,
            directory_trees: _,
            file_write_origins: _,
            file_contexts: _,
            file_path_nodes: _,
            file_metadata_nodes: _,
            file_deletions: _,
            file_branch_merges: _,
            file_branch_aliases: _,
            file_node_parents: _,
            file_path_notes: _,
            directory_tree_selections: _,
            file_locations: _,
            file_backup_paths: _,
            file_backup_policies: _,
            directory_selections: _,
            directory_selection_generations: _,
            file_selection_generations: _,
            file_notes: _,
            version_notes: _,
            file_names: _,
            commits: _,
            versions: _,
            placements: _,
            merge_links: _,
            history_boundaries: _,
            content_states: _,
            operations: _,
            plans: _,
            placement_rules: _,
            protection_policies: _,
            operation_principals: _,
        } = next;
        unchanged!(
            repositories,
            directory_git_repositories,
            histories,
            file_selections,
            file_write_origins,
            file_path_nodes,
            file_deletions,
            file_branch_merges,
            file_branch_aliases,
            file_node_parents,
            file_path_notes,
            file_locations,
            file_backup_paths,
            file_backup_policies,
            directory_selections,
            directory_selection_generations,
            file_selection_generations,
            file_notes,
            version_notes,
            file_names,
            versions,
            placements,
            merge_links,
            history_boundaries,
            content_states,
            plans,
            placement_rules,
            protection_policies
        );
        verify_patch!(branches, &self.branches);
        verify_patch!(commits, &self.commits);
        verify_patch!(file_metadata_nodes, &self.file_metadata_nodes);
        verify_patch!(directory_branches, &self.directory_branches);
        verify_patch!(directory_trees, &self.directory_trees);
        verify_patch!(file_contexts, &self.file_contexts);
        verify_patch!(directory_tree_selections, &self.directory_tree_selections);
        let operation = BTreeMap::from([(receipt.operation_id, Some(receipt.clone()))]);
        let actor = BTreeMap::from([(receipt.operation_id, Some(principal.to_owned()))]);
        verify_patch!(operations, &operation);
        verify_patch!(operation_principals, &actor);
        if next.sequence <= previous.sequence
            || next.business_generation != increment(previous.business_generation)?
            || next.topology_generation != previous.topology_generation
            || next.control_auth != previous.control_auth
            || previous.events.len().checked_add(1) != Some(next.events.len())
        {
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
        let event = &next.events[previous.events.len()];
        if event.operation_id != receipt.operation_id
            || event.sequence != next.sequence
            || event.kind != "file.tags.update"
            || event.object_id
                != receipt
                    .commit_id
                    .clone()
                    .unwrap_or_else(|| receipt.operation_id.to_string())
        {
            return Err(invalid());
        }
        Ok(())
    }
}

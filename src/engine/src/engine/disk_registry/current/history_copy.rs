//! Exact online history-fork admission, shared by preparation and cold recovery.
use super::*;
use crate::business::history_copy::{self, CopyKind, CopyRoute, FileHistoryCopyRequest, invalid};
use crate::engine::{Prepared, ReadSource};
use crate::{CommitState, CurrentHealth, Event, OperationResult, ProtectionAtPublish};
use json_codec::json;

fn receipt<'a>(previous: &State, next: &'a State) -> Result<Option<&'a OperationResult>> {
    let mut found = None;
    let mut inspect = |id: &crate::OperationId| -> Result<()> {
        let before = previous.operations.get(id);
        let after = next.operations.get(id);
        if before == after {
            return Ok(());
        }
        if before.into_iter().chain(after).any(|op| {
            history_copy::is_kind(&op.kind) || op.result.get("history_copy_request").is_some()
        }) {
            let current = after.ok_or_else(invalid)?;
            if before.is_some()
                || current.operation_id != *id
                || !history_copy::is_kind(&current.kind)
                || found.replace(current).is_some()
            {
                return Err(invalid());
            }
        }
        Ok(())
    };
    match next.operations.changes_since(&previous.operations) {
        storage_contract::MapChanges::Keys(keys) => {
            for id in keys.iter() {
                inspect(id)?;
            }
        }
        storage_contract::MapChanges::All => {
            for id in previous
                .operations
                .keys()
                .chain(next.operations.keys())
                .collect::<BTreeSet<_>>()
            {
                inspect(id)?;
            }
        }
    }
    Ok(found)
}

// Compare the complete schema, using exact tracked keys on the live path.
// Cold deserialization has no tracking authority and compares full maps.
fn same_write_set(previous: &State, expected: &State, actual: &State) -> bool {
    macro_rules! maps {
        ($($field:ident),* $(,)?) => {{
            let State { sequence: _, business_generation: _, topology_generation: _, control_auth: _, events: _, $($field: _,)* } = actual;
            $(let same = match (expected.$field.changes_since(&previous.$field), actual.$field.changes_since(&previous.$field)) {
                (storage_contract::MapChanges::Keys(left), storage_contract::MapChanges::Keys(right)) => left.iter().chain(right.iter()).all(|key| expected.$field.get(key) == actual.$field.get(key)),
                _ => expected.$field == actual.$field,
            };
            if !same { return false; })*
        }};
    }
    maps!(
        repositories,
        histories,
        branches,
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
        file_locations,
        file_backup_paths,
        file_backup_policies,
        directory_selections,
        directory_selection_generations,
        file_selection_generations,
        file_notes,
        version_notes,
        file_names,
        commits,
        versions,
        placements,
        merge_links,
        history_boundaries,
        content_states,
        operations,
        plans,
        placement_rules,
        protection_policies,
        operation_principals
    );
    let events = match actual.events.changes_since(&previous.events) {
        storage_contract::SequenceChanges::Append { start } if start == previous.events.len() => {
            actual.events.len() == expected.events.len()
                && actual.events.last() == expected.events.last()
        }
        _ => actual.events == expected.events,
    };
    events
        && actual.sequence == expected.sequence
        && actual.business_generation == expected.business_generation
        && actual.topology_generation == expected.topology_generation
        && actual.control_auth == expected.control_auth
}

impl CurrentNamespaces {
    pub(crate) fn reject_degraded_history_copy(
        candidate: &crate::journal::Candidate,
        next: &State,
    ) -> Result<()> {
        if next
            .operations
            .get(&candidate.operation_id)
            .is_some_and(|op| history_copy::is_kind(&op.kind))
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "complete file-history copy requires its full original payload set; preserve the candidate and recover the same operation",
            ));
        }
        Ok(())
    }

    pub(super) fn advance_history_copy(
        &self,
        previous: &State,
        next: &State,
    ) -> Result<Option<Self>> {
        let Some(op) = receipt(previous, next)? else {
            return Ok(None);
        };
        let kind = CopyKind::parse(&op.kind)?;
        let principal = next
            .operation_principals
            .get(&op.operation_id)
            .ok_or_else(invalid)?;
        Engine::require_actor(principal)?;
        let request: FileHistoryCopyRequest =
            json_codec::from_value(op.result["history_copy_request"].clone())?;
        let route: CopyRoute = json_codec::from_value(op.result["history_copy_route"].clone())?;
        let target = previous
            .repositories
            .get(&request.target_repository_id)
            .ok_or_else(invalid)?;
        // This legacy request carries a copy count, not an explicit fork
        // selection. Cold replay must reproduce the SAME live route policy,
        // not invent selection authority from its stored result vector.
        if self.backup_configuration_route(&target.primary_slot_id, target.target_copies)?
            != route.slots
            || route.bindings.len() != route.slots.len()
        {
            return Err(invalid());
        }
        for slot in &route.slots {
            let disk = self.backup_configuration_disk(slot)?;
            if route.bindings.get(slot) != Some(&disk.identity.binding_generation) {
                return Err(invalid());
            }
        }
        if op.sequence != next.sequence
            || next.sequence <= previous.sequence
            || op.request_digest
                != Engine::native_request_digest(&op.kind, principal, &json!(request))?
            || op.commit_state != CommitState::Published
            || op.current_health != CurrentHealth::Healthy
            || op.protection_at_publish != ProtectionAtPublish::TargetMet
            || previous.operations.contains_key(&op.operation_id)
            || previous.operation_principals.contains_key(&op.operation_id)
        {
            return Err(invalid());
        }
        let mut expected = previous.clone();
        expected.begin_tracking();
        crate::container_storage::capture_names(&mut expected);
        expected.sequence = next.sequence;
        expected.business_generation = previous
            .business_generation
            .checked_add(1)
            .ok_or_else(invalid)?;
        let mut result = kind.transform()(
            previous,
            &mut expected,
            Some(self),
            &request,
            &route,
            principal,
            op.operation_id,
        )?;
        crate::container_storage::capture_changed_names(previous, &mut expected);
        for field in ["timestamp", "membership_epoch"] {
            storage_contract::parse_u64_decimal(op.result[field].as_str().ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
            result[field] = op.result[field].clone();
        }
        result["branch_generation"] = json!(
            expected.branches[&target.main_branch_id]
                .generation
                .to_string()
        );
        let versions = result["version_ids"].as_array().ok_or_else(invalid)?;
        let protection = versions.iter().map(|id| {
            let id = id.as_str().ok_or_else(invalid)?;
            let placement = &expected.placements[id];
            let slots = placement.route_slot_ids.iter().map(|slot| { let disk = self.backup_configuration_disk(slot)?; Ok(json!({"slot_id":slot,"copy_level":disk.level,"disk_id":disk.identity.disk_id,"binding_generation":disk.identity.binding_generation.to_string(),"state":"COMPLETE_VERIFIED"})) }).collect::<Result<Vec<_>>>()?;
            Ok(json!({"version_id":id,"primary_slot_id":placement.primary_slot_id,"target_copies":placement.target_copies,"required_slots":placement.route_slot_ids,"topology_generation":placement.topology_generation.to_string(),"slots":slots}))
        }).collect::<Result<Vec<_>>>()?;
        result["version_protection"] = json!(protection);
        if result != op.result
            || op.branch_id.as_deref() != Some(target.main_branch_id.as_str())
            || op.history_tree_id.as_deref() != Some(target.history_tree_id.as_str())
            || op.commit_id.as_deref() != result["commit_id"].as_str()
        {
            return Err(invalid());
        }
        expected.operations.insert(op.operation_id, op.clone());
        expected
            .operation_principals
            .insert(op.operation_id, principal.clone());
        expected.events.push(Event {
            sequence: next.sequence,
            operation_id: op.operation_id,
            kind: op.kind.clone(),
            object_id: op.commit_id.clone().ok_or_else(invalid)?,
        });
        if !same_write_set(previous, &expected, next) {
            return Err(invalid());
        }
        let mut view = self.clone();
        for (id, branch) in next
            .branches
            .iter()
            .filter(|(id, branch)| previous.branches.get(*id) != Some(*branch))
        {
            let entries = &next.commits[&branch.head_commit_id].entries;
            Arc::make_mut(&mut view.branches).insert(
                id.clone(),
                Arc::new(CurrentScope {
                    head: branch.head_commit_id.clone(),
                    entries: Arc::new(entries.clone()),
                    source_head: branch.head_commit_id.clone(),
                    source_entries: Arc::new(entries.clone()),
                    writable: true,
                    hidden_paths: BTreeSet::new(),
                }),
            );
        }
        // Directory export advances selected working checkpoints in the same
        // closed transformation. The exact full-state replay above, not a raw
        // serialized directory tree, grants these new scopes their authority.
        for (id, branch) in next.directory_branches.iter().filter(|(id, branch)| {
            previous.directory_branches.get(*id) != Some(*branch)
        }) {
            Arc::make_mut(&mut view.directories).insert(
                id.clone(),
                Arc::new(CurrentScope {
                    head: branch.head_tree_id.clone(),
                    entries: Arc::new(branch.entries.clone()),
                    source_head: branch.head_tree_id.clone(),
                    source_entries: Arc::new(branch.entries.clone()),
                    writable: true,
                    hidden_paths: BTreeSet::new(),
                }),
            );
        }
        view.reserve_history_copy_identities(previous, &expected);
        Ok(Some(view))
    }

    fn reserve_history_copy_identities(&mut self, previous: &State, next: &State) {
        // Historical occurrences reserve IDs too, even outside every HEAD.
        // Tracking may legitimately fall back to All at its bounded key budget.
        let mut reserve = |commit: &Commit| {
            for entry in &commit.entries {
                Arc::make_mut(&mut self.known_entries).insert(entry.entry_id.clone());
                if entry.kind == NamespaceEntryKind::File {
                    Arc::make_mut(&mut self.known_files).insert(entry.file_id.clone());
                }
            }
        };
        match next.commits.changes_since(&previous.commits) {
            storage_contract::MapChanges::Keys(commits) => {
                for id in commits.iter() {
                    reserve(&next.commits[id]);
                }
            }
            storage_contract::MapChanges::All => {
                for (id, commit) in next.commits.iter() {
                    if !previous.commits.contains_key(id) {
                        reserve(commit);
                    }
                }
            }
        }
    }

    pub(in crate::engine) fn verify_history_copy_prepared(
        &self,
        previous: &State,
        next: &State,
        result: &OperationResult,
        prepared: &[Prepared],
    ) -> Result<()> {
        let Some(op) = receipt(previous, next)? else {
            return Ok(());
        };
        if op != result {
            return Err(invalid());
        }
        self.verify_history_copy_payloads(
            next,
            result,
            prepared.iter().map(|copy| (&copy.version, &copy.placement)),
        )?;
        let request: FileHistoryCopyRequest =
            json_codec::from_value(result.result["history_copy_request"].clone())?;
        let sources = CopyKind::parse(&result.kind)?.sources(previous, Some(self), &request)?;
        if sources.versions.len() != prepared.len() {
            return Err(invalid());
        }
        for (id, copy) in sources.versions.iter().zip(prepared) {
            let ReadSource::VerifiedReplica(source) = &copy.source else {
                return Err(invalid());
            };
            let disk = self.backup_configuration_disk(&source.owner().seal.slot_id)?;
            let placement = &previous.placements[id];
            if source.version() != &previous.versions[id]
                || source.owner().seal.pool_id != disk.identity.pool_id
                || source.owner().seal.disk_id != disk.identity.disk_id
                || source.owner().seal.binding_generation != disk.identity.binding_generation
                || !placement.route_slot_ids.contains(&disk.identity.slot_id)
                || placement.binding_generations.get(&disk.identity.slot_id)
                    != Some(&disk.identity.binding_generation)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    pub(in crate::engine) fn verify_history_copy_candidate(
        &self,
        previous: &State,
        next: &State,
        candidate: &crate::journal::Candidate,
    ) -> Result<bool> {
        let Some(op) = receipt(previous, next)? else {
            return Ok(false);
        };
        if candidate.registry() != Some(&self.registry)
            || candidate.operation_id != op.operation_id
            || candidate.sequence != op.sequence
            || candidate.request_digest != op.request_digest
        {
            return Err(invalid());
        }
        self.verify_history_copy_payloads(
            next,
            op,
            candidate
                .versions
                .iter()
                .map(|copy| (&copy.version, &copy.placement)),
        )?;
        Ok(true)
    }

    fn verify_history_copy_payloads<'a>(
        &self,
        next: &State,
        op: &OperationResult,
        payloads: impl Iterator<Item = (&'a crate::FileVersion, &'a crate::Placement)>,
    ) -> Result<()> {
        let expected = op.result["version_ids"].as_array().ok_or_else(invalid)?;
        let actual = payloads.collect::<Vec<_>>();
        if actual.len() != expected.len() {
            return Err(invalid());
        }
        for (id, (version, placement)) in expected.iter().zip(actual) {
            let id = id.as_str().ok_or_else(invalid)?;
            if next.versions.get(id) != Some(version) || next.placements.get(id) != Some(placement)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/history_copy_admission.rs"]
mod tests;

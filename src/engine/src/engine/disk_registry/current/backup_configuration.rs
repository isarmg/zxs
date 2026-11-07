//! Backup configuration authenticates its actual policy, owner-path and body
//! placement plan before the unchanged-namespace fast path. It is not an I/O grant.
use super::*;
use crate::engine::{Prepared, ReadSource};
use crate::journal::Candidate;
use crate::{
    CommitState, CurrentHealth, Event, FileVersion, HistoricalDiskDefinition, OperationId,
    OperationResult, Placement, ProtectionAtPublish, RecursiveBackupCapture,
    RecursiveBackupConfigurationRequest, ScopedBackupConfigurationRequest,
};
use data_serialization::Deserialize;
use json_codec::{Value, json};
use storage_contract::{MapChanges, SequenceChanges, TrackedMap, TrackedOperations};

const KIND: &str = "backup.configure";
const RECURSIVE_KIND: &str = "backup.configure-recursive";

fn native_backup_kind(kind: &str) -> bool {
    matches!(kind, KIND | RECURSIVE_KIND)
}

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "backup configuration publication differs from its exact current source and native request",
    )
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct BackupInput {
    repository_id: String,
    path: String,
    expected_scope_id: Option<String>,
    target_copies: u32,
    backup_prefixes: BTreeMap<String, String>,
    #[serialization(with = "storage_contract::u64_string")]
    expected_generation: u64,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct RecursiveBackupInput {
    repository_id: String,
    path: String,
    expected_scope_id: String,
    target_copies: u32,
    #[serialization(with = "storage_contract::u64_string")]
    expected_generation: u64,
    capture: RecursiveBackupCapture,
}

impl CurrentNamespaces {
    pub(in crate::engine) fn is_backup_configuration(receipt: &OperationResult) -> bool {
        native_backup_kind(&receipt.kind)
            || receipt.result.get("namespace_backup").is_some()
            || receipt.result.get("backup_request").is_some()
    }

    /// No current-registry backup may create or install an outcome overlay
    /// until both ordinary and archived namespace replay prove that protocol.
    pub(crate) fn reject_degraded_backup(candidate: &Candidate, next: &State) -> Result<()> {
        if candidate.registry().is_some()
            && next
                .operations
                .get(&candidate.operation_id)
                .is_some_and(Self::is_backup_configuration)
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "current-registry backup does not support degraded recovery; preserve the original candidate and complete its full-copy publication",
            ));
        }
        Ok(())
    }

    pub(in crate::engine) fn verify_new_backup_configuration(
        &self,
        previous: &State,
        next: &State,
        result: &OperationResult,
    ) -> Result<()> {
        let receipt = new_receipt(previous, next)?.ok_or_else(invalid)?;
        if receipt != result {
            return Err(invalid());
        }
        self.verify_backup_configuration(previous, next, receipt)
    }

    /// Called only after the complete State proof. The expected ordered list
    /// was reconstructed from the native request, including all eligible history.
    pub(in crate::engine) fn verify_backup_prepared(
        &self,
        previous: &State,
        next: &State,
        result: &OperationResult,
        prepared: &[Prepared],
    ) -> Result<()> {
        match new_receipt(previous, next)? {
            Some(receipt) if receipt == result => (),
            Some(_) => return Err(invalid()),
            None if Self::is_backup_configuration(result) => return Err(invalid()),
            None => return Ok(()),
        }
        verify_payloads(
            next,
            result,
            prepared.iter().map(|copy| (&copy.version, &copy.placement)),
        )?;
        for copy in prepared {
            // Re-protection has a managed, enabled historical source. An
            // external path or merely identical bytes is not source admission.
            let ReadSource::VerifiedReplica(source) = &copy.source else {
                return Err(invalid());
            };
            let owner = source.owner();
            if source.version() != &copy.version {
                return Err(invalid());
            }
            let disk = self.backup_configuration_disk(&owner.seal.slot_id)?;
            let source = previous
                .placements
                .get(&copy.version.version_id)
                .ok_or_else(invalid)?;
            if owner.seal.pool_id != disk.identity.pool_id
                || owner.seal.disk_id != disk.identity.disk_id
                || owner.seal.binding_generation != disk.identity.binding_generation
                || previous.versions.get(&copy.version.version_id) != Some(&copy.version)
                || !source.route_slot_ids.contains(&disk.identity.slot_id)
                || source.binding_generations.get(&disk.identity.slot_id)
                    != Some(&disk.identity.binding_generation)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    /// State admission does not bind the separate Candidate payload vector.
    /// Replay calls this before local proof discovery or any repair work.
    pub(in crate::engine) fn verify_backup_candidate_payloads(
        &self,
        previous: &State,
        next: &State,
        candidate: &Candidate,
    ) -> Result<bool> {
        let result = next
            .operations
            .get(&candidate.operation_id)
            .ok_or_else(invalid)?;
        match new_receipt(previous, next)? {
            Some(receipt) if receipt == result => (),
            Some(_) => return Err(invalid()),
            None if Self::is_backup_configuration(result) => return Err(invalid()),
            None => return Ok(false),
        }
        if candidate.registry() != Some(&self.registry)
            || result.operation_id != candidate.operation_id
            || result.sequence != candidate.sequence
            || result.request_digest != candidate.request_digest
        {
            return Err(invalid());
        }
        verify_payloads(
            next,
            result,
            candidate
                .versions
                .iter()
                .map(|copy| (&copy.version, &copy.placement)),
        )?;
        Ok(true)
    }

    pub(crate) fn backup_configuration_disk(
        &self,
        slot: &str,
    ) -> Result<&HistoricalDiskDefinition> {
        let (disk, mode, _) = self
            .registry_state
            .registered_definitions()
            .find(|(disk, _, _)| disk.identity.slot_id.eq_ignore_ascii_case(slot))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::SlotNotFound,
                    "backup slot is no longer registered",
                )
            })?;
        if mode != super::super::RegistrationMode::Enabled {
            return Err(Error::new(
                ErrorCode::PlacementInvalid,
                "backup route contains a disabled storage slot",
            ));
        }
        Ok(disk)
    }

    pub(crate) fn verify_recursive_backup_capture(
        &self,
        state: &State,
        capture: &RecursiveBackupCapture,
    ) -> Result<()> {
        capture.validate_predecessor(
            state,
            &self.registry,
            self.registry_state
                .registered_definitions()
                .filter(|(_, mode, _)| *mode == super::super::RegistrationMode::Enabled)
                .map(|(disk, _, _)| {
                    (
                        disk.identity.clone(),
                        disk.level,
                        disk.upstream_slot_id.clone(),
                    )
                }),
        )
    }

    pub(crate) fn backup_configuration_route(
        &self,
        primary: &str,
        copies: u32,
    ) -> Result<Vec<String>> {
        let mut disk = self.backup_configuration_disk(primary)?;
        if disk.level != 1 || copies == 0 {
            return Err(Error::new(
                ErrorCode::PlacementInvalid,
                "placement requires a primary slot and at least one copy",
            ));
        }
        let mut route = vec![disk.identity.slot_id.clone()];
        while route.len() < copies as usize {
            let next =
                self.registry_state
                    .historical_topology()
                    .find(|next| {
                        next.level == disk.level + 1
                            && next.upstream_slot_id.as_deref().is_some_and(|slot| {
                                slot.eq_ignore_ascii_case(&disk.identity.slot_id)
                            })
                    })
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::RouteIncomplete,
                            "requested copies exceed the configured fixed route",
                        )
                    })?;
            disk = self.backup_configuration_disk(&next.identity.slot_id)?;
            route.push(disk.identity.slot_id.clone());
        }
        Ok(route)
    }

    pub(crate) fn backup_configuration_placement(
        &self,
        version: &str,
        primary: &str,
        copies: u32,
        topology_generation: u64,
    ) -> Result<Placement> {
        let route = self.backup_configuration_route(primary, copies)?;
        Ok(Placement {
            version_id: version.into(),
            primary_slot_id: route[0].clone(),
            target_copies: copies,
            topology_generation,
            binding_generations: route
                .iter()
                .map(|slot| {
                    Ok((
                        slot.clone(),
                        self.backup_configuration_disk(slot)?
                            .identity
                            .binding_generation,
                    ))
                })
                .collect::<Result<_>>()?,
            route_slot_ids: route,
        })
    }

    pub(crate) fn require_backup_configuration_source(
        &self,
        state: &State,
        version: &FileVersion,
    ) -> Result<()> {
        let placement = state
            .placements
            .get(&version.version_id)
            .ok_or_else(invalid)?;
        if self.unavailable_versions.contains(&version.version_id)
            || !self
                .registry_state
                .registered_definitions()
                .any(|(disk, mode, _)| {
                    mode == super::super::RegistrationMode::Enabled
                        && placement.route_slot_ids.contains(&disk.identity.slot_id)
                        && placement.binding_generations.get(&disk.identity.slot_id)
                            == Some(&disk.identity.binding_generation)
                })
        {
            return Err(Error::new(
                ErrorCode::ContentUnavailable,
                "backup source body has no enabled retained registration",
            ));
        }
        Ok(())
    }

    pub(crate) fn backup_configuration_source(
        &self,
        state: &State,
        repository: &str,
        scope: &str,
    ) -> Result<Value> {
        let repo = state.repositories.get(repository).ok_or_else(invalid)?;
        let branch = state
            .branches
            .get(&repo.main_branch_id)
            .ok_or_else(invalid)?;
        let history = state
            .histories
            .get(&repo.history_tree_id)
            .ok_or_else(invalid)?;
        if branch.history_tree_id != repo.history_tree_id
            || branch.file_id.is_some()
            || history.repository_id != repo.repository_id
            || history.main_branch_id.as_deref() != Some(repo.main_branch_id.as_str())
        {
            return Err(invalid());
        }
        self.branch_entries(&branch.branch_id, &branch.head_commit_id)?;
        let mut directories = BTreeMap::new();
        for directory in state.directory_branches.values().filter(|directory| {
            directory.repository_id == repository && directory.directory_id == scope
        }) {
            self.directory_entries(&directory.directory_branch_id, &directory.head_tree_id)?;
            directories.insert(&directory.directory_branch_id, &directory.head_tree_id);
        }
        Ok(json!({
            "registry": self.registry,
            "mask_hash": self.mask_hash,
            "source_snapshot": self.source_snapshot,
            "source_sequence": self.source_sequence.to_string(),
            "source_business_generation": self.source_business_generation.to_string(),
            "activation_sequence": self.activation_sequence.to_string(),
            "previous_sequence": state.sequence.to_string(),
            "previous_business_generation": state.business_generation.to_string(),
            "owner_branch_id": branch.branch_id,
            "owner_head_commit_id": branch.head_commit_id,
            "directory_heads": directories
        }))
    }

    pub(super) fn verify_backup_configurations(
        &self,
        previous: &State,
        next: &State,
    ) -> Result<()> {
        match new_receipt(previous, next)? {
            Some(receipt) => self.verify_backup_configuration(previous, next, receipt),
            None if scoped_policy_changed(previous, next) => Err(invalid()),
            None => Ok(()),
        }
    }

    fn verify_backup_configuration(
        &self,
        previous: &State,
        next: &State,
        receipt: &OperationResult,
    ) -> Result<()> {
        let generated: Vec<String> =
            json_codec::from_value(receipt.result["generated_policy_ids"].clone())?;
        let principal = next
            .operation_principals
            .get(&receipt.operation_id)
            .ok_or_else(invalid)?;
        Engine::require_actor(principal)?;
        let mut ids = generated.iter();
        let (repository_id, plan) = match receipt.kind.as_str() {
            KIND => {
                let input: BackupInput =
                    json_codec::from_value(receipt.result["backup_request"].clone())?;
                let plan = self.plan_backup_configuration(
                    previous,
                    ScopedBackupConfigurationRequest {
                        repository_id: &input.repository_id,
                        path: &input.path,
                        expected_scope_id: input.expected_scope_id.as_deref(),
                        target_copies: input.target_copies,
                        backup_prefixes: &input.backup_prefixes,
                        expected_generation: input.expected_generation,
                        principal,
                        operation_id: receipt.operation_id,
                    },
                    || ids.next().cloned().ok_or_else(invalid),
                )?;
                (input.repository_id, plan)
            }
            RECURSIVE_KIND => {
                let input: RecursiveBackupInput =
                    json_codec::from_value(receipt.result["backup_request"].clone())?;
                let plan = self.plan_recursive_backup_configuration(
                    previous,
                    RecursiveBackupConfigurationRequest {
                        repository_id: &input.repository_id,
                        path: &input.path,
                        expected_scope_id: &input.expected_scope_id,
                        target_copies: input.target_copies,
                        expected_generation: input.expected_generation,
                        capture: &input.capture,
                        principal,
                        operation_id: receipt.operation_id,
                    },
                    || ids.next().cloned().ok_or_else(invalid),
                )?;
                (input.repository_id, plan)
            }
            _ => return Err(invalid()),
        };
        if ids.next().is_some() {
            return Err(invalid());
        }
        let digest = Engine::native_request_digest(
            &receipt.kind,
            principal,
            &plan.result["backup_request"],
        )?;
        let mut result = plan.result;
        for key in ["configured_at", "timestamp", "membership_epoch"] {
            let decimal = receipt.result[key].as_str().ok_or_else(invalid)?;
            result[key] = json!(storage_contract::parse_u64_decimal(decimal)?.to_string());
        }
        let mut protection = Vec::new();
        for version in &plan.versions {
            let placement = &plan.placements[&version.version_id];
            let mut slots = Vec::new();
            for slot in &placement.route_slot_ids {
                let disk = self.backup_configuration_disk(slot)?;
                slots.push(json!({
                    "slot_id": slot,
                    "copy_level": disk.level,
                    "disk_id": disk.identity.disk_id,
                    "binding_generation": disk.identity.binding_generation.to_string(),
                    "state": "COMPLETE_VERIFIED"
                }));
            }
            protection.push(json!({
                "version_id": version.version_id,
                "primary_slot_id": placement.primary_slot_id,
                "target_copies": placement.target_copies,
                "required_slots": placement.route_slot_ids,
                "topology_generation": placement.topology_generation.to_string(),
                "slots": slots
            }));
        }
        result["version_protection"] = json!(protection);
        let expected = OperationResult {
            operation_id: receipt.operation_id,
            kind: receipt.kind.clone(),
            commit_id: None,
            history_tree_id: Some(
                previous.repositories[&repository_id]
                    .history_tree_id
                    .clone(),
            ),
            branch_id: None,
            commit_state: CommitState::Published,
            protection_at_publish: if plan.versions.is_empty() {
                ProtectionAtPublish::NotApplicable
            } else {
                ProtectionAtPublish::TargetMet
            },
            current_health: CurrentHealth::Healthy,
            request_digest: digest,
            sequence: next.sequence,
            result,
        };
        if receipt != &expected
            || next.sequence <= previous.sequence
            || previous.business_generation.checked_add(1) != Some(next.business_generation)
            || !exact_updates(
                &previous.protection_policies,
                &next.protection_policies,
                &plan.policies,
            )
            || !exact_upserts(
                &previous.file_backup_policies,
                &next.file_backup_policies,
                &plan.bindings,
            )
            || !exact_upserts(
                &previous.file_backup_paths,
                &next.file_backup_paths,
                &plan.paths,
            )
            || !exact_upserts(&previous.placements, &next.placements, &plan.placements)
            || !only_added(
                &previous.operations,
                &next.operations,
                receipt.operation_id,
                receipt,
            )
            || !only_added(
                &previous.operation_principals,
                &next.operation_principals,
                receipt.operation_id,
                principal,
            )
            || !one_event(previous, next, receipt)
            || !unchanged_state(previous, next)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

fn new_receipt<'a>(previous: &State, next: &'a State) -> Result<Option<&'a OperationResult>> {
    let mut ids = BTreeSet::new();
    for changes in [
        next.operations.changes_since(&previous.operations),
        next.operation_principals
            .changes_since(&previous.operation_principals),
    ] {
        match changes {
            MapChanges::Keys(keys) => ids.extend(keys.iter().copied()),
            MapChanges::All => {
                ids.extend(previous.operations.keys().copied());
                ids.extend(next.operations.keys().copied());
            }
        }
    }
    let mut receipt = None;
    for id in ids {
        let before = previous.operations.get(&id);
        let after = next.operations.get(&id);
        if before.is_some_and(CurrentNamespaces::is_backup_configuration) {
            if before != after
                || previous.operation_principals.get(&id) != next.operation_principals.get(&id)
            {
                return Err(invalid());
            }
        } else if let Some(after) =
            after.filter(|operation| CurrentNamespaces::is_backup_configuration(operation))
            && (before.is_some()
                || !native_backup_kind(&after.kind)
                || after.operation_id != id
                || receipt.replace(after).is_some())
        {
            return Err(invalid());
        }
    }
    let start = match next.events.changes_since(&previous.events) {
        SequenceChanges::Append { start } => start,
        SequenceChanges::All => 0,
    };
    for index in start..previous.events.len().max(next.events.len()) {
        let before = previous.events.get(index);
        let after = next.events.get(index);
        if before != after
            && (before.is_some_and(|event| native_backup_kind(&event.kind))
                || after.is_some_and(|event| {
                    native_backup_kind(&event.kind)
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

// This consumes the exact result already proved by verify_backup_configuration,
// not an independently supplied plan or a list inferred from disk contents.
fn verify_payloads<'a>(
    next: &State,
    result: &OperationResult,
    copies: impl ExactSizeIterator<Item = (&'a FileVersion, &'a Placement)>,
) -> Result<()> {
    if !native_backup_kind(&result.kind)
        || next.operations.get(&result.operation_id) != Some(result)
    {
        return Err(invalid());
    }
    let expected = result.result["version_protection"]
        .as_array()
        .ok_or_else(invalid)?;
    if copies.len() != expected.len() {
        return Err(invalid());
    }
    for ((version, placement), expected) in copies.zip(expected) {
        if expected["version_id"].as_str() != Some(version.version_id.as_str())
            || next.versions.get(&version.version_id) != Some(version)
            || next.placements.get(&version.version_id) != Some(placement)
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn scoped_policy_changed(previous: &State, next: &State) -> bool {
    let changed = |id: &String| {
        let before = previous.protection_policies.get(id);
        let after = next.protection_policies.get(id);
        before != after
            && (after.is_some_and(|policy| {
                policy.scope_file_id.is_some()
                    || policy.scope_directory_id.is_some()
                    || !policy.inheritable
            }) || before.is_some_and(|policy| {
                policy.inheritable
                    && (policy.scope_file_id.is_some() || policy.scope_directory_id.is_some())
            }))
    };
    match next
        .protection_policies
        .changes_since(&previous.protection_policies)
    {
        MapChanges::Keys(keys) => keys.iter().any(changed),
        MapChanges::All => previous
            .protection_policies
            .keys()
            .chain(next.protection_policies.keys())
            .any(changed),
    }
}

fn exact_updates<T: PartialEq>(
    before: &TrackedMap<T>,
    after: &TrackedMap<T>,
    updates: &BTreeMap<String, Option<T>>,
) -> bool {
    if updates
        .iter()
        .any(|(id, value)| after.get(id) != value.as_ref())
    {
        return false;
    }
    unchanged_except(before, after, |id| updates.contains_key(id))
}
fn exact_upserts<T: PartialEq>(
    before: &TrackedMap<T>,
    after: &TrackedMap<T>,
    updates: &BTreeMap<String, T>,
) -> bool {
    if updates
        .iter()
        .any(|(id, value)| after.get(id) != Some(value))
    {
        return false;
    }
    unchanged_except(before, after, |id| updates.contains_key(id))
}
fn unchanged_except<T: PartialEq>(
    before: &TrackedMap<T>,
    after: &TrackedMap<T>,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    let equal = |id: &String| allowed(id) || after.get(id) == before.get(id);
    match after.changes_since(before) {
        MapChanges::Keys(keys) => keys.iter().all(equal),
        MapChanges::All => before.keys().chain(after.keys()).all(equal),
    }
}
fn only_added<T: PartialEq>(
    before: &TrackedOperations<T>,
    after: &TrackedOperations<T>,
    key: OperationId,
    expected: &T,
) -> bool {
    if before.contains_key(&key)
        || before.len().checked_add(1) != Some(after.len())
        || after.get(&key) != Some(expected)
    {
        return false;
    }
    let equal = |id: &OperationId| *id == key || after.get(id) == before.get(id);
    match after.changes_since(before) {
        MapChanges::Keys(keys) => keys.iter().all(equal),
        MapChanges::All => before.keys().all(equal),
    }
}
fn one_event(previous: &State, next: &State, receipt: &OperationResult) -> bool {
    if previous.events.len().checked_add(1) != Some(next.events.len()) {
        return false;
    }
    let expected = Event {
        sequence: next.sequence,
        operation_id: receipt.operation_id,
        kind: receipt.kind.clone(),
        object_id: receipt
            .history_tree_id
            .clone()
            .expect("invariant: verified backup receipt has its history"),
    };
    next.events.last() == Some(&expected)
        && match next.events.changes_since(&previous.events) {
            SequenceChanges::Append { start } => start == previous.events.len(),
            SequenceChanges::All => next.events[..previous.events.len()] == previous.events[..],
        }
}

// Every State field must be classified. In particular physical file_names and
// file_locations are immutable inputs here, never cache-repair permissions.
macro_rules! unchanged_fields {
    ($($field:ident),* $(,)?) => {
        fn unchanged_state(previous: &State, next: &State) -> bool {
            let State {
                sequence: _,
                business_generation: _,
                operations: _,
                operation_principals: _,
                events: _,
                protection_policies: _,
                file_backup_paths: _,
                file_backup_policies: _,
                placements: _,
                topology_generation,
                control_auth,
                $($field,)*
            } = next;
            topology_generation == &previous.topology_generation
                && control_auth == &previous.control_auth
                $(&& match $field.changes_since(&previous.$field) {
                    MapChanges::Keys(keys) => keys
                        .iter()
                        .all(|id| $field.get(id) == previous.$field.get(id)),
                    MapChanges::All => $field == &previous.$field,
                })*
        }
    };
}
unchanged_fields! {
    repositories, histories, branches, file_selections, directory_branches, directory_git_repositories, directory_trees,
    file_write_origins, file_contexts, file_path_nodes, file_metadata_nodes, file_deletions, file_branch_merges,
    file_branch_aliases, file_node_parents, file_path_notes, directory_tree_selections, file_locations,
    directory_selections, directory_selection_generations, file_selection_generations, file_notes,
    version_notes, file_names, commits, versions, merge_links, history_boundaries, content_states,
    plans, placement_rules,
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_backup_configuration_tests.rs"]
mod tests;

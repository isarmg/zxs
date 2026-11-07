//! A new native rename/move carries a mandatory, exact recursive path plan.
//! The index is derived only from verified publications, never from receipt labels.
use super::*;
use crate::protection::{
    BATCH_FOLLOW_KIND, BATCH_ROUTE_KIND, DirectoryFollowChange, DirectoryFollowPlan,
    DirectoryNamespace, FILE_FOLLOW_KIND, FILE_ROUTE_KIND, follows_directories, follows_routes,
};
use crate::{OperationId, OperationResult, ProtectionPolicy, RecursiveBackupCapture};
use data_serialization::{Deserialize, Serialize};
use json_codec::{Value, json};
use storage_contract::{MapChanges, SequenceChanges, TrackedMap};

pub(crate) const FILE_KIND: &str = "file.namespace.recursive";
pub(crate) const BATCH_KIND: &str = "directory.namespace.batch.recursive";
const FORMAT: &str = "recursive-namespace-follow";

pub(crate) fn is_kind(kind: &str) -> bool {
    matches!(
        kind,
        FILE_KIND
            | BATCH_KIND
            | FILE_FOLLOW_KIND
            | BATCH_FOLLOW_KIND
            | FILE_ROUTE_KIND
            | BATCH_ROUTE_KIND
    )
}

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "recursive namespace publication differs from its bound policy and exact path plan",
    )
}

#[derive(Clone, Default)]
pub(in crate::engine::disk_registry) struct PolicyIndex {
    policies: BTreeMap<String, Arc<BoundPolicy>>,
    scopes: BTreeMap<String, BTreeSet<String>>,
    directories: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Clone)]
struct BoundPolicy {
    policy: ProtectionPolicy,
    authority: OperationId,
    capture: RecursiveBackupCapture,
    roots: BTreeMap<String, String>,
    directory_namespace: Option<DirectoryNamespace>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct PolicyChange {
    source_operation_id: OperationId,
    policy: ProtectionPolicy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct PathChange {
    source_operation_id: OperationId,
    policy_id: String,
    previous_path: String,
    path: String,
    backup_paths: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct FollowPlan {
    format: String,
    policies: BTreeMap<String, PolicyChange>,
    paths: BTreeMap<String, PathChange>,
}

pub(super) struct VerifiedFollow<'a> {
    receipt: &'a OperationResult,
    plan: FollowPlan,
    directories: Option<DirectoryFollowPlan>,
}

fn changed_keys<T>(before: &TrackedMap<T>, after: &TrackedMap<T>) -> BTreeSet<String> {
    match after.changes_since(before) {
        MapChanges::Keys(keys) => Arc::unwrap_or_clone(keys),
        MapChanges::All => before.keys().chain(after.keys()).cloned().collect(),
    }
}

fn joined(root: &str, suffix: &str) -> Result<String> {
    let path = if suffix.is_empty() {
        root.to_owned()
    } else {
        format!("{root}/{suffix}")
    };
    Ok(name_policy::PortablePath::parse(&path)?.as_str().to_owned())
}

mod directories;
mod request;
mod routes;

fn one_event(previous: &State, next: &State, receipt: &OperationResult) -> bool {
    let expected = crate::Event {
        sequence: next.sequence,
        operation_id: receipt.operation_id,
        kind: receipt.kind.clone(),
        object_id: receipt
            .commit_id
            .clone()
            .or_else(|| receipt.history_tree_id.clone())
            .unwrap_or_else(|| receipt.operation_id.to_string()),
    };
    previous.events.len().checked_add(1) == Some(next.events.len())
        && next.events.last() == Some(&expected)
        && match next.events.changes_since(&previous.events) {
            SequenceChanges::Append { start } => start == previous.events.len(),
            SequenceChanges::All => next.events[..previous.events.len()] == previous.events[..],
        }
}

impl PolicyIndex {
    fn remove(&mut self, id: &str) {
        if let Some(bound) = self.policies.remove(id) {
            if let Some(namespace) = &bound.directory_namespace {
                for entry in namespace.entry_ids().collect::<BTreeSet<_>>() {
                    let ids = self
                        .directories
                        .get_mut(entry)
                        .expect("invariant: indexed directory exists");
                    ids.remove(id);
                    if ids.is_empty() {
                        self.directories.remove(entry);
                    }
                }
            }
            let scope = bound
                .policy
                .scope_file_id
                .as_ref()
                .or(bound.policy.scope_directory_id.as_ref())
                .expect("invariant: a native recursive policy has a stable scope");
            let ids = self
                .scopes
                .get_mut(scope)
                .expect("invariant: indexed scope exists");
            ids.remove(id);
            if ids.is_empty() {
                self.scopes.remove(scope);
            }
        }
    }

    fn insert(&mut self, bound: BoundPolicy) {
        let id = bound.policy.policy_id.clone();
        self.remove(&id);
        if let Some(namespace) = &bound.directory_namespace {
            for entry in namespace.entry_ids() {
                self.directories
                    .entry(entry.clone())
                    .or_default()
                    .insert(id.clone());
            }
        }
        let scope = bound
            .policy
            .scope_file_id
            .as_ref()
            .or(bound.policy.scope_directory_id.as_ref())
            .expect("invariant: a native recursive policy has a stable scope");
        self.scopes
            .entry(scope.clone())
            .or_default()
            .insert(id.clone());
        self.policies.insert(id, Arc::new(bound));
    }
}

impl CurrentNamespaces {
    /// Old native namespace receipts never rewrote recursive backup paths.
    /// A different kind cannot acquire that new capability by dropping its plan.
    pub(super) fn require_recursive_paths_unchanged(
        &self,
        previous: &State,
        next: &State,
    ) -> Result<()> {
        for file in changed_keys(&previous.file_backup_paths, &next.file_backup_paths) {
            if previous.file_backup_paths.get(&file) != next.file_backup_paths.get(&file)
                && previous
                    .file_backup_policies
                    .get(&file)
                    .is_some_and(|policy| {
                        self.recursive_policies
                            .policies
                            .get(policy)
                            .is_some_and(|bound| bound.directory_namespace.is_none())
                    })
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    pub(crate) fn recursive_namespace_kind(kind: &str) -> bool {
        is_kind(kind)
    }

    /// The caller has already verified the backup publication's entire State.
    pub(super) fn advance_recursive_index(
        &self,
        previous: &State,
        next: &State,
        backup: Option<&OperationResult>,
        follow: Option<&VerifiedFollow<'_>>,
    ) -> Result<Arc<PolicyIndex>> {
        let mut index = Arc::clone(&self.recursive_policies);
        for id in changed_keys(&previous.protection_policies, &next.protection_policies) {
            if previous.protection_policies.get(&id) != next.protection_policies.get(&id)
                && index.policies.contains_key(&id)
            {
                Arc::make_mut(&mut index).remove(&id);
            }
        }
        if let Some(follow) = follow {
            for (id, change) in &follow.plan.policies {
                let mut bound = self.recursive_policies.policies[id].as_ref().clone();
                bound.policy = change.policy.clone();
                bound.authority = follow.receipt.operation_id;
                if let Some(change) = follow
                    .directories
                    .as_ref()
                    .and_then(|plan| plan.policies.get(id))
                {
                    bound.directory_namespace = Some(change.namespace.clone());
                }
                Arc::make_mut(&mut index).insert(bound);
            }
        }
        // Both protocols derive authority only after complete backup admission.
        // Old namespace kinds still ignore empty snapshot bindings.
        if let Some(receipt) = backup.filter(|receipt| receipt.kind == "backup.configure-recursive")
        {
            let policy: ProtectionPolicy =
                json_codec::from_value(receipt.result["policy"].clone())?;
            let capture: RecursiveBackupCapture =
                json_codec::from_value(receipt.result["backup_request"]["capture"].clone())?;
            let repository = previous
                .repositories
                .get(&policy.repository_id)
                .ok_or_else(invalid)?;
            let roots = capture.follow_roots(repository, policy.backup_prefixes.keys())?;
            Arc::make_mut(&mut index).insert(BoundPolicy {
                policy,
                authority: receipt.operation_id,
                capture,
                roots,
                directory_namespace: crate::protection::captured_directory_namespace(receipt)?,
            });
        }
        Ok(index)
    }

    fn active_recursive_policy<'a>(
        &'a self,
        previous: &State,
        id: &str,
        include_directories: bool,
    ) -> Result<Option<&'a BoundPolicy>> {
        let Some(bound) = self.recursive_policies.policies.get(id) else {
            return Ok(None);
        };
        if bound.directory_namespace.is_some() && !include_directories {
            return Ok(None);
        }
        if previous.protection_policies.get(id) != Some(&bound.policy) || !bound.policy.inheritable
        {
            return Err(invalid());
        }
        Ok(Some(bound))
    }

    fn moved_scopes(
        &self,
        previous: &State,
        next: &State,
    ) -> Result<BTreeMap<String, (String, String)>> {
        let mut moved = BTreeMap::new();
        for id in changed_keys(&previous.branches, &next.branches) {
            let (Some(before), Some(after)) = (previous.branches.get(&id), next.branches.get(&id))
            else {
                continue;
            };
            if before.file_id.is_some() || before.head_commit_id == after.head_commit_id {
                continue;
            }
            let history = previous
                .histories
                .get(&before.history_tree_id)
                .ok_or_else(invalid)?;
            let repository = previous
                .repositories
                .get(&history.repository_id)
                .ok_or_else(invalid)?;
            if repository.main_branch_id != id {
                continue;
            }
            let next_entries = &next
                .commits
                .get(&after.head_commit_id)
                .ok_or_else(invalid)?
                .entries;
            let paths: BTreeMap<_, _> = next_entries
                .iter()
                .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
                .map(|entry| (entry.entry_id.as_str(), entry.path()))
                .collect();
            for entry in self.branch_entries(&id, &before.head_commit_id)? {
                if entry.kind != NamespaceEntryKind::Directory
                    || !(self.recursive_policies.scopes.contains_key(&entry.entry_id)
                        || self
                            .recursive_policies
                            .directories
                            .contains_key(&entry.entry_id))
                {
                    continue;
                }
                if let Some(path) = paths.get(entry.entry_id.as_str())
                    && *path != entry.path()
                {
                    moved.insert(
                        entry.entry_id.clone(),
                        (entry.path().to_owned(), (*path).to_owned()),
                    );
                }
            }
        }
        for file in changed_keys(&previous.file_locations, &next.file_locations) {
            if let (Some(before), Some(after)) = (
                previous.file_locations.get(&file),
                next.file_locations.get(&file),
            ) && before != after
                && self.recursive_policies.scopes.contains_key(&file)
            {
                moved.insert(file.to_owned(), (before.clone(), after.clone()));
            }
        }
        Ok(moved)
    }

    fn recursive_follow_plan(
        &self,
        previous: &State,
        next: &State,
        kind: &str,
    ) -> Result<(FollowPlan, Option<DirectoryFollowPlan>)> {
        let include_directories = follows_directories(kind);
        let routed = follows_routes(kind);
        let changed_files = changed_keys(&previous.file_locations, &next.file_locations);
        let routes = if routed {
            let mut followed_files = BTreeSet::new();
            for file in &changed_files {
                let (Some(before), Some(after)) = (
                    previous.file_locations.get(file),
                    next.file_locations.get(file),
                ) else {
                    continue;
                };
                if before == after {
                    continue;
                }
                if let Some(id) = previous.file_backup_policies.get(file)
                    && self.active_recursive_policy(previous, id, true)?.is_some()
                {
                    followed_files.insert(file.clone());
                }
            }
            routes::collect(previous, &followed_files)?
        } else {
            BTreeMap::new()
        };
        let mut plan = FollowPlan {
            format: FORMAT.into(),
            policies: BTreeMap::new(),
            paths: BTreeMap::new(),
        };
        let moves = self.moved_scopes(previous, next)?;
        for (scope, (_, path)) in &moves {
            let Some(policies) = self.recursive_policies.scopes.get(scope) else {
                continue;
            };
            for id in policies {
                let Some(bound) =
                    self.active_recursive_policy(previous, id, include_directories)?
                else {
                    continue;
                };
                if !routed {
                    bound
                        .capture
                        .verify_follow_routes(self, bound.roots.keys())?;
                }
                let mut policy = bound.policy.clone();
                if policy.path_prefix == *path {
                    continue;
                }
                policy.path_prefix = path.clone();
                policy.generation = policy.generation.checked_add(1).ok_or_else(|| {
                    Error::new(
                        ErrorCode::ResourceLimit,
                        "recursive policy generation exhausted",
                    )
                })?;
                let parent = if policy.scope_file_id.is_some() {
                    policy
                        .path_prefix
                        .rsplit_once('/')
                        .map_or("", |(parent, _)| parent)
                } else {
                    &policy.path_prefix
                };
                policy.backup_prefixes = bound
                    .roots
                    .iter()
                    .map(|(slot, root)| Ok((slot.clone(), joined(root, parent)?)))
                    .collect::<Result<_>>()?;
                plan.policies.insert(
                    id.clone(),
                    PolicyChange {
                        source_operation_id: bound.authority,
                        policy,
                    },
                );
            }
        }
        for file in changed_files {
            let (Some(before), Some(after)) = (
                previous.file_locations.get(&file),
                next.file_locations.get(&file),
            ) else {
                continue;
            };
            if before == after {
                continue;
            }
            let Some(id) = previous.file_backup_policies.get(&file) else {
                continue;
            };
            let Some(bound) = self.active_recursive_policy(previous, id, include_directories)?
            else {
                continue;
            };
            let route = if routed {
                let route = routes.get(&file).ok_or_else(invalid)?;
                route.verify(self, bound)?;
                Some(route)
            } else {
                bound
                    .capture
                    .verify_follow_routes(self, bound.roots.keys())?;
                None
            };
            if bound.roots.is_empty() {
                continue;
            }
            let mut paths = previous
                .file_backup_paths
                .get(&file)
                .ok_or_else(invalid)?
                .clone();
            for (slot, root) in &bound.roots {
                if let Some(route) = route
                    && !paths.contains_key(slot)
                    && !route.backups.contains(slot)
                {
                    continue;
                }
                if paths.get(slot) != Some(&joined(root, before)?) {
                    return Err(Error::new(
                        ErrorCode::ScopeChanged,
                        "recursive backup path no longer matches its primary location; explicitly configure it again",
                    ));
                }
                paths.insert(slot.clone(), joined(root, after)?);
            }
            plan.paths.insert(
                file,
                PathChange {
                    source_operation_id: bound.authority,
                    policy_id: id.clone(),
                    previous_path: before.clone(),
                    path: after.clone(),
                    backup_paths: paths,
                },
            );
        }
        let directories = if include_directories {
            Some(self.directory_follow_plan(previous, &moves, &mut plan)?)
        } else {
            None
        };
        Ok((plan, directories))
    }

    pub(crate) fn apply_recursive_follow(
        &self,
        previous: &State,
        next: &mut State,
        kind: &str,
    ) -> Result<(Value, Option<Value>)> {
        let (plan, directories) = self.recursive_follow_plan(previous, next, kind)?;
        for (id, change) in &plan.policies {
            next.protection_policies
                .insert(id.clone(), change.policy.clone());
        }
        for (id, change) in &plan.paths {
            next.file_backup_paths
                .insert(id.clone(), change.backup_paths.clone());
        }
        Ok((json!(plan), directories.map(|plan| json!(plan))))
    }

    pub(super) fn verify_recursive_follow<'a>(
        &self,
        previous: &State,
        next: &'a State,
    ) -> Result<Option<VerifiedFollow<'a>>> {
        let mut found = None;
        let mut ids = BTreeSet::new();
        for changes in [
            next.operations.changes_since(&previous.operations),
            next.operation_principals
                .changes_since(&previous.operation_principals),
        ] {
            match changes {
                MapChanges::Keys(keys) => ids.extend(keys.iter().copied()),
                MapChanges::All => ids.extend(
                    previous
                        .operations
                        .keys()
                        .chain(next.operations.keys())
                        .copied(),
                ),
            }
        }
        for id in ids {
            let before = previous.operations.get(&id);
            let after = next.operations.get(&id);
            if let Some(before) = before.filter(|receipt| is_kind(&receipt.kind)) {
                if Some(before) != after
                    || previous.operation_principals.get(&id) != next.operation_principals.get(&id)
                {
                    return Err(invalid());
                }
            } else if let Some(after) = after.filter(|receipt| {
                is_kind(&receipt.kind)
                    || receipt.result.get("recursive_follow").is_some()
                    || receipt.result.get("recursive_directory_follow").is_some()
            }) && (before.is_some()
                || !is_kind(&after.kind)
                || after.operation_id != id
                || found.replace(after).is_some())
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
                && (before.is_some_and(|event| is_kind(&event.kind))
                    || after.is_some_and(|event| {
                        is_kind(&event.kind)
                            && (before.is_some()
                                || Some(event.operation_id)
                                    != found.map(|receipt| receipt.operation_id))
                    }))
            {
                return Err(invalid());
            }
        }
        let Some(receipt) = found else {
            return Ok(None);
        };
        self.verify_follow_request(previous, next, receipt)?;
        let plan: FollowPlan = json_codec::from_value(receipt.result["recursive_follow"].clone())?;
        let (expected, expected_directories) =
            self.recursive_follow_plan(previous, next, &receipt.kind)?;
        let directories = if follows_directories(&receipt.kind) {
            Some(DirectoryFollowPlan::from_receipt(receipt)?)
        } else {
            if receipt.result.get("recursive_directory_follow").is_some() {
                return Err(invalid());
            }
            None
        };
        let policies = expected
            .policies
            .iter()
            .map(|(id, change)| (id.clone(), change.policy.clone()))
            .collect();
        let paths = expected
            .paths
            .iter()
            .map(|(id, change)| (id.clone(), change.backup_paths.clone()))
            .collect();
        let principal = next
            .operation_principals
            .get(&receipt.operation_id)
            .ok_or_else(invalid)?;
        Engine::require_actor(principal)?;
        if plan != expected
            || directories != expected_directories
            || receipt.commit_state != crate::CommitState::Published
            || receipt.protection_at_publish != crate::ProtectionAtPublish::NotApplicable
            || receipt.current_health != crate::CurrentHealth::Healthy
            || receipt.sequence != next.sequence
            || next.sequence <= previous.sequence
            || previous.business_generation.checked_add(1) != Some(next.business_generation)
            || Engine::native_request_digest(
                &receipt.kind,
                principal,
                &receipt.result["namespace_request"],
            )? != receipt.request_digest
            || !super::backup_configuration::only_added(
                &previous.operations,
                &next.operations,
                receipt.operation_id,
                receipt,
            )
            || !super::backup_configuration::only_added(
                &previous.operation_principals,
                &next.operation_principals,
                receipt.operation_id,
                principal,
            )
            || !one_event(previous, next, receipt)
            || !super::backup_configuration::exact_upserts(
                &previous.file_deletions,
                &next.file_deletions,
                &BTreeMap::new(),
            )
            || !super::backup_configuration::exact_upserts(
                &previous.protection_policies,
                &next.protection_policies,
                &policies,
            )
            || !super::backup_configuration::exact_upserts(
                &previous.file_backup_paths,
                &next.file_backup_paths,
                &paths,
            )
            || !super::backup_configuration::exact_upserts(
                &previous.file_backup_policies,
                &next.file_backup_policies,
                &BTreeMap::new(),
            )
            || !super::backup_configuration::exact_upserts(
                &previous.placements,
                &next.placements,
                &BTreeMap::new(),
            )
        {
            return Err(invalid());
        }
        Ok(Some(VerifiedFollow {
            receipt,
            plan,
            directories,
        }))
    }
}

impl Engine {
    /// This selects a native semantics, not a retry fallback. An existing
    /// operation keeps its original kind; mutate still checks its full digest
    /// and principal before returning the permanent outcome.
    pub(crate) fn namespace_mutation_kind(
        &self,
        legacy: &'static str,
        operation: OperationId,
        follows: bool,
    ) -> &'static str {
        let current = match legacy {
            "file.namespace" => FILE_KIND,
            "directory.namespace.batch" => BATCH_KIND,
            _ => unreachable!("invariant: only namespace rename callers select this semantics"),
        };
        let with_directories = match legacy {
            "file.namespace" => FILE_FOLLOW_KIND,
            "directory.namespace.batch" => BATCH_FOLLOW_KIND,
            _ => unreachable!("invariant: only namespace rename callers select this semantics"),
        };
        let routed = match legacy {
            "file.namespace" => FILE_ROUTE_KIND,
            "directory.namespace.batch" => BATCH_ROUTE_KIND,
            _ => unreachable!("invariant: namespace caller"),
        };
        if let Some(receipt) = self.state.operations.get(&operation) {
            if receipt.kind == legacy {
                return legacy;
            }
            if receipt.kind == current {
                return current;
            }
            if receipt.kind == with_directories {
                return with_directories;
            }
            if receipt.kind == routed {
                return routed;
            }
        }
        if follows
            && self
                .current_namespaces
                .as_ref()
                .is_some_and(|view| !view.recursive_policies.policies.is_empty())
        {
            routed
        } else {
            legacy
        }
    }
}

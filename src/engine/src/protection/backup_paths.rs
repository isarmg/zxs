//! Stable backup roots and immediate re-protection reuse immutable file bodies.
use super::*;
use crate::{
    business::{find_unordered, now},
    engine::{CurrentNamespaces, Prepared},
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) mod directory_namespaces;
mod recursive;
pub use directory_namespaces::PublishedBackupDirectory;
pub(crate) use recursive::RecursiveBackupMode;
pub use recursive::{RecursiveBackupCapture, RecursiveBackupConfigurationRequest};

/// Backup placement for a path, fenced by the current policy generation.
#[derive(Clone, Copy, Debug)]
pub struct BackupConfigurationRequest<'a> {
    pub repository_id: &'a str,
    pub path: &'a str,
    pub target_copies: u32,
    pub backup_prefixes: &'a BTreeMap<String, String>,
    pub expected_generation: u64,
    pub principal: &'a str,
    pub operation_id: OperationId,
}

/// Backup placement with an additional stable file/directory identity fence.
#[derive(Clone, Copy, Debug)]
pub struct ScopedBackupConfigurationRequest<'a> {
    pub repository_id: &'a str,
    pub path: &'a str,
    pub expected_scope_id: Option<&'a str>,
    pub target_copies: u32,
    pub backup_prefixes: &'a BTreeMap<String, String>,
    pub expected_generation: u64,
    pub principal: &'a str,
    pub operation_id: OperationId,
}

fn namespace<'a>(state: &'a State, repository: &str) -> Result<&'a Commit> {
    let repo = state.repositories.get(repository).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::RepositoryNotFound,
            "backup repository missing",
        )
    })?;
    // AdmittedState validates repository -> main branch -> head commit.
    Ok(state
        .branches
        .get(&repo.main_branch_id)
        .and_then(|branch| state.commits.get(&branch.head_commit_id))
        .expect("invariant: admitted repository has its main namespace"))
}
fn within(root: &str, path: &str) -> bool {
    let root = name_policy::path_key(root);
    let path = name_policy::path_key(path);
    root.is_empty()
        || root == path
        || path
            .strip_prefix(&root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}
pub(super) fn scope_path<'a>(state: &'a State, policy: &'a ProtectionPolicy) -> Option<&'a str> {
    let entries = &namespace(state, &policy.repository_id)
        .expect("invariant: admitted policy belongs to an existing repository")
        .entries;
    scope_path_in(entries, policy)
}

/// Resolve stable scopes against the caller's admitted namespace. Historical
/// callers explicitly retain their own Commit entries instead of becoming current.
pub(super) fn scope_path_in<'a>(
    entries: &'a [Entry],
    policy: &'a ProtectionPolicy,
) -> Option<&'a str> {
    if policy.scope_directory_id.as_deref() == Some(policy.repository_id.as_str()) {
        return Some("");
    }
    if let Some(directory) = &policy.scope_directory_id {
        return entries
            .iter()
            .find(|entry| {
                entry.kind == NamespaceEntryKind::Directory && entry.entry_id == *directory
            })
            .map(|entry| entry.path());
    }
    if let Some(file) = &policy.scope_file_id {
        return entries
            .iter()
            .find(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == *file)
            .map(|entry| entry.path());
    }
    Some(&policy.path_prefix)
}
fn policy_applies(
    entries: &[Entry],
    policy: &ProtectionPolicy,
    file: Option<&str>,
    path: &str,
) -> bool {
    if !policy.inheritable {
        return false;
    }
    if let Some(expected) = &policy.scope_file_id {
        return file.is_some_and(|file| file == expected);
    }
    scope_path_in(entries, policy).is_some_and(|root| within(root, path))
}
pub(super) fn effective_policy<'a>(
    state: &'a State,
    repository: &str,
    file: Option<&str>,
    path: &str,
) -> Option<&'a ProtectionPolicy> {
    let entries = &namespace(state, repository)
        .expect("invariant: placement caller resolved the repository")
        .entries;
    effective_policy_in(state, entries, repository, file, path)
}

pub(super) fn effective_policy_in<'a>(
    state: &'a State,
    entries: &[Entry],
    repository: &str,
    file: Option<&str>,
    path: &str,
) -> Option<&'a ProtectionPolicy> {
    let file = file.or_else(|| {
        find_unordered(entries, path)
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
            .map(|entry| entry.file_id.as_str())
    });
    if let Some(policy) = file
        .and_then(|file| state.file_backup_policies.get(file))
        .map(|id| {
            state
                .protection_policies
                .get(id)
                .expect("invariant: admitted file backup binding names its history policy")
        })
        // The public placement query accepts an optional file from its caller;
        // admission proves the binding, not that caller's repository choice.
        .filter(|policy| policy.repository_id == repository)
    {
        return Some(policy);
    }
    state
        .protection_policies
        .values()
        .filter(|policy| {
            policy.repository_id == repository && policy_applies(entries, policy, file, path)
        })
        .max_by_key(|policy| {
            let depth = scope_path_in(entries, policy).map_or(0, |path| {
                path.split('/').filter(|part| !part.is_empty()).count()
            });
            // A rename can bring a stable scope onto an existing path rule.
            // At equal depth the stable scope wins, never the random policy ID;
            // a more specific child rule still takes precedence.
            (
                depth,
                policy.scope_directory_id.is_some() || policy.scope_file_id.is_some(),
            )
        })
}

#[derive(Clone)]
struct Scope {
    id: String,
    kind: NamespaceEntryKind,
    path: String,
    files: BTreeMap<String, Entry>,
}
fn source_scope(
    state: &State,
    current: Option<&CurrentNamespaces>,
    repository: &str,
    path: &str,
) -> Result<Scope> {
    let requested = if path.is_empty() {
        None
    } else {
        Some(name_policy::PortablePath::parse(path)?)
    };
    let commit = namespace(state, repository)?;
    let entries = match current {
        Some(current) => current.branch_entries(
            &state.repositories[repository].main_branch_id,
            &commit.commit_id,
        )?,
        None => &commit.entries,
    };
    let (mut scope, target) = if let Some(requested) = &requested {
        let entry =
            crate::business::find_unordered_key(entries, requested.key()).ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::EntryNotFound,
                    "backup scope no longer exists",
                )
            })?;
        (
            Scope {
                id: if entry.kind == NamespaceEntryKind::File {
                    entry.file_id.clone()
                } else {
                    entry.entry_id.clone()
                },
                kind: entry.kind,
                path: entry.path().to_owned(),
                files: BTreeMap::new(),
            },
            Some(entry.location()),
        )
    } else {
        (
            Scope {
                id: repository.into(),
                kind: NamespaceEntryKind::Directory,
                path: String::new(),
                files: BTreeMap::new(),
            },
            None,
        )
    };
    let scope_key = target.map_or("", name_policy::PortablePath::key);
    for entry in entries.iter().filter(|entry| {
        entry.kind == NamespaceEntryKind::File
            && if scope.kind == NamespaceEntryKind::File {
                entry.file_id == scope.id
            } else {
                scope_key.is_empty()
                    || scope_key == entry.lookup_key()
                    || entry
                        .lookup_key()
                        .strip_prefix(scope_key)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            }
    }) {
        let prefer = state
            .file_locations
            .get(&entry.file_id)
            .is_some_and(|owner| name_policy::path_key(owner) == entry.lookup_key());
        if prefer || !scope.files.contains_key(&entry.file_id) {
            scope.files.insert(entry.file_id.clone(), entry.clone());
        }
    }
    // A configured directory protects files retained by its other named views,
    // including versions absent from the currently selected subtree.
    if scope.kind == NamespaceEntryKind::Directory {
        for branch in state
            .directory_branches
            .values()
            .filter(|branch| branch.repository_id == repository && branch.directory_id == scope.id)
        {
            let depth = if branch.path.is_empty() {
                0
            } else {
                branch.path.split('/').count()
            };
            let entries = match current {
                Some(current) => {
                    current.directory_entries(&branch.directory_branch_id, &branch.head_tree_id)?
                }
                None => &branch.entries,
            };
            for entry in entries
                .iter()
                .filter(|entry| entry.kind == NamespaceEntryKind::File)
            {
                if let std::collections::btree_map::Entry::Vacant(slot) =
                    scope.files.entry(entry.file_id.clone())
                {
                    let mut entry = entry.clone();
                    let path = if let Some(target) = target {
                        entry.location().rebase(depth, target)?
                    } else {
                        entry
                            .location()
                            .strip_components(depth)
                            .expect("invariant: retained file is below its directory scope")
                    };
                    entry.relocate(path);
                    slot.insert(entry);
                }
            }
        }
    }
    Ok(scope)
}
fn exact_policy<'a>(
    state: &'a State,
    repository: &str,
    scope: &Scope,
) -> Option<&'a ProtectionPolicy> {
    state
        .protection_policies
        .values()
        .find(|policy| {
            policy.inheritable
                && policy.repository_id == repository
                && if scope.kind == NamespaceEntryKind::File {
                    policy.scope_file_id.as_deref() == Some(&scope.id)
                } else {
                    policy.scope_directory_id.as_deref() == Some(&scope.id)
                }
        })
        .or_else(|| {
            state.protection_policies.values().find(|policy| {
                policy.inheritable
                    && policy.repository_id == repository
                    && policy.scope_file_id.is_none()
                    && policy.scope_directory_id.is_none()
                    && name_policy::path_key(&policy.path_prefix)
                        == name_policy::path_key(&scope.path)
            })
        })
}
fn inherited_by_scope(
    entries: &[Entry],
    scope_root: &str,
    previous: Option<&ProtectionPolicy>,
    effective: Option<&ProtectionPolicy>,
) -> bool {
    let Some(effective) = effective else {
        return true;
    };
    effective.inheritable
        && (previous.is_some_and(|own| own.policy_id == effective.policy_id)
            || (effective.scope_file_id.is_none()
                && scope_path_in(entries, effective).is_some_and(|root| within(root, scope_root))))
}
fn retain_inherited_files(
    state: &State,
    entries: &[Entry],
    repository: &str,
    scope: &mut Scope,
    previous: Option<&ProtectionPolicy>,
) {
    if scope.kind != NamespaceEntryKind::Directory {
        return;
    }
    scope.files.retain(|file, entry| {
        inherited_by_scope(
            entries,
            &scope.path,
            previous,
            effective_policy_in(state, entries, repository, Some(file), entry.path()),
        )
    });
}

// Before the first registry publication the admitted immutable main HEAD is
// also the current namespace. Keep this initial-state consumer explicit.
fn retain_initial_inherited_files(
    state: &State,
    repository: &str,
    scope: &mut Scope,
    previous: Option<&ProtectionPolicy>,
) {
    if scope.kind != NamespaceEntryKind::Directory {
        return;
    }
    scope.files.retain(|file, entry| {
        effective_policy(state, repository, Some(file), entry.path()).is_none_or(|effective| {
            effective.inheritable
                && (previous.is_some_and(|own| own.policy_id == effective.policy_id)
                    || effective.scope_file_id.is_none()
                        && scope_path(state, effective)
                            .is_some_and(|root| within(root, &scope.path)))
        })
    });
}

fn backup_primaries(engine: &Engine, repository: &str, scope: &Scope) -> Result<BTreeSet<String>> {
    let current = engine.namespace_tree(&engine.state.repositories[repository].main_branch_id)?;
    let mut primaries = BTreeSet::from([engine
        .file_placement_settings(repository, None, &scope.path)?
        .0]);
    // Empty and future descendants also inherit this directory's settings.
    // Independent child backup policies do not constrain the parent's routes.
    if scope.kind == NamespaceEntryKind::Directory {
        let previous = exact_policy(&engine.state, repository, scope);
        for rule in engine.state.placement_rules.values().filter(|rule| {
            rule.repository_id == repository && within(&scope.path, &rule.path_prefix)
        }) {
            if inherited_by_scope(
                current.entries(),
                &scope.path,
                previous,
                effective_policy_in(
                    &engine.state,
                    current.entries(),
                    repository,
                    None,
                    &rule.path_prefix,
                ),
            ) {
                primaries.insert(rule.primary_slot_id.clone());
            }
        }
    }
    // Existing files may have a more specific future placement rule than the
    // directory itself, even before their first version on that primary.
    for (file, entry) in &scope.files {
        primaries.insert(
            engine
                .file_placement_settings(repository, Some(file), entry.path())?
                .0,
        );
    }
    for version in engine.state.versions.values().filter(|version| {
        scope.files.contains_key(&version.file_id)
            && engine
                .state
                .content_states
                .get(&version.version_id)
                .is_none_or(|state| *state == storage_contract::ContentState::Available)
    }) {
        let placement = engine
            .state
            .placements
            .get(&version.version_id)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::PlacementNotFound,
                    "backup source placement missing",
                )
            })?;
        primaries.insert(placement.primary_slot_id.clone());
    }
    Ok(primaries)
}

pub(crate) fn prune_retained_policies(state: &mut State, repository: &str) {
    let referenced = state
        .file_backup_policies
        .values()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    state.protection_policies.retain_keys(|id, policy| {
        policy.repository_id != repository || policy.inheritable || referenced.contains(id.as_str())
    });
}

fn relative_path(scope_path: &str, file_path: &str, is_file: bool) -> String {
    if is_file {
        return file_path.rsplit('/').next().unwrap_or(file_path).into();
    }
    if within(scope_path, file_path) {
        let depth = if scope_path.is_empty() {
            0
        } else {
            scope_path.split('/').count()
        };
        file_path
            .split('/')
            .skip(depth)
            .collect::<Vec<_>>()
            .join("/")
    } else {
        file_path.rsplit('/').next().unwrap_or(file_path).into()
    }
}
fn backup_path(prefix: &str, relative: &str) -> Result<String> {
    if !prefix.is_empty() {
        name_policy::validate_path(prefix)?;
    }
    let path = if prefix.is_empty() {
        relative.into()
    } else {
        format!("{prefix}/{relative}")
    };
    name_policy::validate_path(&path)?;
    Ok(path)
}
pub(crate) fn apply_backup_paths(
    state: &mut State,
    current_entries: &[Entry],
    repository: &str,
    file: &str,
    path: &str,
    route: &[String],
) -> Result<()> {
    let Some(policy) =
        effective_policy_in(state, current_entries, repository, Some(file), path).cloned()
    else {
        return Ok(());
    };
    if policy.backup_prefixes.is_empty() {
        // A scoped one-copy rule still follows the file after a primary move.
        if policy.scope_directory_id.is_some() || policy.scope_file_id.is_some() {
            state
                .file_backup_policies
                .entry(file.into())
                .or_insert(policy.policy_id);
        }
        return Ok(());
    }
    let root = scope_path_in(current_entries, &policy).unwrap_or(&policy.path_prefix);
    let relative = relative_path(root, path, policy.scope_file_id.is_some());
    let locations = route
        .iter()
        .skip(1)
        .map(|slot| {
            let prefix = policy.backup_prefixes.get(slot).ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::BackupPrefixRequired,
                    format!("backup slot {slot} requires a user-chosen prefix"),
                )
            })?;
            Ok((slot.clone(), backup_path(prefix, &relative)?))
        })
        .collect::<Result<Vec<_>>>()?;
    for (slot, path) in locations {
        state
            .file_backup_paths
            .entry(file.into())
            .or_default()
            .entry(slot)
            .or_insert(path);
    }
    state
        .file_backup_policies
        .entry(file.into())
        .or_insert(policy.policy_id);
    Ok(())
}

impl Engine {
    pub fn backup_configuration(
        &self,
        repository: &str,
        path: &str,
    ) -> Result<storage_contract::BackupConfigurationSnapshot> {
        let admitted_current = self.admitted_current_namespaces()?;
        let mut scope = source_scope(&self.state, admitted_current, repository, path)?;
        let current = self.namespace_tree(&self.state.repositories[repository].main_branch_id)?;
        let exact = exact_policy(&self.state, repository, &scope);
        let effective = effective_policy_in(
            &self.state,
            current.entries(),
            repository,
            (scope.kind == NamespaceEntryKind::File).then_some(scope.id.as_str()),
            &scope.path,
        );
        let inherited = effective.is_some_and(|policy| {
            policy.inheritable && exact.is_none_or(|exact| exact.policy_id != policy.policy_id)
        });
        retain_inherited_files(
            &self.state,
            current.entries(),
            repository,
            &mut scope,
            exact,
        );
        let (primary, copies) = self.file_placement_settings(
            repository,
            (scope.kind == NamespaceEntryKind::File).then_some(scope.id.as_str()),
            &scope.path,
        )?;
        // GET and PUT use the same future and historical source chains.
        let primaries = backup_primaries(self, repository, &scope)?;
        let mut routes = Vec::new();
        for source in primaries {
            // Validate the primary itself. Only later route growth may stop at
            // a Disabled/Detached downstream; PUT still rejects that route.
            let mut route = self.route(&source, 1)?;
            while route.len() < self.config.disks.len() {
                match self.route(&source, route.len() as u32 + 1) {
                    Ok(next) => route = next,
                    Err(error)
                        if error.code() == storage_contract::ErrorCode::RouteIncomplete
                            || admitted_current.is_some()
                                && error.code()
                                    == storage_contract::ErrorCode::PlacementInvalid =>
                    {
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
            routes.push(route);
        }
        let available_route = routes
            .iter()
            .find(|route| route.first() == Some(&primary))
            .cloned()
            .expect("invariant: default primary chain is included");
        let maximum = routes
            .iter()
            .map(Vec::len)
            .min()
            .expect("invariant: routes include the default primary chain");
        let requested_route = match self.route(&primary, copies) {
            Ok(route) => route,
            Err(error)
                if error.code() == storage_contract::ErrorCode::RouteIncomplete
                    || admitted_current.is_some()
                        && error.code() == storage_contract::ErrorCode::PlacementInvalid =>
            {
                Vec::new()
            }
            Err(error) => return Err(error),
        };
        let backup_slots = routes
            .iter()
            .flat_map(|route| route.iter().skip(1))
            .cloned()
            .collect::<BTreeSet<_>>();
        let backup_paths = if scope.kind == NamespaceEntryKind::File {
            self.state
                .file_backup_paths
                .get(&scope.id)
                .cloned()
                .unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        Ok(storage_contract::BackupConfigurationSnapshot {
            scope: storage_contract::BackupScope {
                id: scope.id,
                kind: scope.kind,
                path: scope.path,
            },
            policy_id: exact.map(|policy| policy.policy_id.clone()),
            effective_policy_id: effective.map(|policy| policy.policy_id.clone()),
            generation: exact.map_or(0, |policy| policy.generation),
            effective_generation: effective.map_or(0, |policy| policy.generation),
            inherited,
            target_copies: copies,
            backup_prefixes: effective
                .map(|policy| policy.backup_prefixes.clone())
                .unwrap_or_default(),
            route_slot_ids: requested_route,
            available_route_slot_ids: available_route,
            backup_routes: routes,
            backup_slot_ids: backup_slots,
            max_copies: maximum,
            file_count: scope.files.len() as u64,
            backup_paths,
        })
    }

    pub fn configure_backups(
        &mut self,
        request: BackupConfigurationRequest<'_>,
    ) -> Result<OperationResult> {
        let BackupConfigurationRequest {
            repository_id: repository,
            path,
            target_copies: copies,
            backup_prefixes: prefixes,
            expected_generation,
            principal,
            operation_id: operation,
        } = request;
        self.configure_backups_scoped(crate::ScopedBackupConfigurationRequest {
            repository_id: repository,
            path,
            expected_scope_id: None,
            target_copies: copies,
            backup_prefixes: prefixes,
            expected_generation,
            principal,
            operation_id: operation,
        })
    }
    pub fn configure_backups_scoped(
        &mut self,
        request: ScopedBackupConfigurationRequest<'_>,
    ) -> Result<OperationResult> {
        let ScopedBackupConfigurationRequest {
            repository_id: repository,
            path,
            expected_scope_id,
            target_copies: copies,
            backup_prefixes: prefixes,
            expected_generation,
            principal,
            operation_id: operation,
        } = request;
        self.mutate(
            "backup.configure", principal, operation,
            json!({"repository_id":repository,"path":path,"expected_scope_id":expected_scope_id,"target_copies":copies,"backup_prefixes":prefixes,"expected_generation":expected_generation.to_string()}),
            |engine, state, pending| {
                if let Some(current) = engine.admitted_current_namespaces()? {
                    let plan = current.plan_backup_configuration(
                        engine.state(),
                        request,
                        || Ok(new_id("policy")?),
                    )?;
                    return apply_current_plan(engine, state, pending, plan);
                }
                let repo = state.repositories.get(repository).cloned()
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::RepositoryNotFound, "backup repository missing"))?;

                let mut scope = source_scope(state, None, repository, path)?;
                if expected_scope_id.is_some_and(|expected| expected != scope.id) {
                    return Err(Error::new(storage_contract::ErrorCode::ScopeChanged, "backup path now describes a different file or directory"));
                }
                let previous = exact_policy(state, repository, &scope).cloned();
                let generation = previous.as_ref().map_or(0, |policy| policy.generation);
                if generation != expected_generation {
                    return Err(Error::new(storage_contract::ErrorCode::GenerationMismatch, "backup configuration changed; reread before updating"));
                }
                // An ancestor cannot silently replace independent child rules
                // or the binding of a file moved from another protected scope.
                retain_initial_inherited_files(state, repository, &mut scope, previous.as_ref());
                let mut normalized = BTreeMap::new();
                for (slot, prefix) in prefixes {
                    let disk = engine.disk(slot)?;
                    if disk.level <= 1 {
                        return Err(Error::new(storage_contract::ErrorCode::InvalidBackupPrefix, "backup prefixes may only target backup slots"));
                    }
                    if !prefix.is_empty() { name_policy::validate_path(prefix)?; }
                    if normalized.insert(disk.slot_id.clone(), prefix.clone()).is_some() {
                        return Err(Error::new(storage_contract::ErrorCode::InvalidBackupPrefix, "backup slot is listed more than once"));
                    }
                }
                let mut required = BTreeSet::new();
                for primary in backup_primaries(engine, repository, &scope)? {
                    required.extend(engine.route(&primary, copies)?.into_iter().skip(1));
                }
                let versions = state.versions.values().filter(|version| {
                    scope.files.contains_key(&version.file_id)
                        && state.content_states.get(&version.version_id).is_none_or(|state| *state == storage_contract::ContentState::Available)
                }).cloned().collect::<Vec<_>>();
                for slot in required {
                    if !normalized.contains_key(&slot) {
                        return Err(Error::new(storage_contract::ErrorCode::BackupPrefixRequired, format!("backup slot {slot} requires a user-chosen prefix")));
                    }
                }
                let policy = ProtectionPolicy {
                    policy_id: match previous.as_ref() { Some(policy) => policy.policy_id.clone(), None => new_id("policy")? },
                    repository_id: repository.into(), path_prefix: scope.path.clone(), target_copies: copies,
                    inheritable: true,
                    scope_directory_id: (scope.kind == NamespaceEntryKind::Directory).then_some(scope.id.clone()),
                    scope_file_id: (scope.kind == NamespaceEntryKind::File).then_some(scope.id.clone()),
                    backup_prefixes: normalized.clone(),
                    generation: generation.checked_add(1).ok_or_else(|| Error::new(storage_contract::ErrorCode::ResourceLimit, "backup policy generation exhausted"))?,
                };
                if let Some(previous) = previous {
                    let detached = state.file_backup_policies.iter()
                        .filter(|(file, id)| *id == &previous.policy_id && !scope.files.contains_key(*file))
                        .map(|(file, _)| file.clone()).collect::<Vec<_>>();
                    if !detached.is_empty() {
                        let mut retained = previous;
                        retained.policy_id = new_id("policy")?;
                        retained.inheritable = false;
                        for file in detached {
                            state.file_backup_policies.insert(file, retained.policy_id.clone());
                        }
                        state.protection_policies.insert(retained.policy_id.clone(), retained);
                    }
                }
                for (file, entry) in &scope.files {
                    let relative = relative_path(&scope.path, entry.path(), scope.kind == NamespaceEntryKind::File);
                    for (slot, prefix) in &normalized {
                        state.file_backup_paths.entry(file.clone()).or_default()
                            .insert(slot.clone(), backup_path(prefix, &relative)?);
                    }
                    state.file_backup_policies.insert(file.clone(), policy.policy_id.clone());
                }
                state.protection_policies.insert(policy.policy_id.clone(), policy.clone());
                prune_retained_policies(state, repository);
                for version in &versions {
                    let previous = state.placements[&version.version_id].clone();
                    let route = engine.route(&previous.primary_slot_id, copies)?;
                    let placement = Placement {
                        version_id: version.version_id.clone(), primary_slot_id: previous.primary_slot_id,
                        target_copies: copies, route_slot_ids: route.clone(), topology_generation: state.topology_generation,
                        binding_generations: route.iter().map(|slot| Ok((slot.clone(), engine.disk(slot)?.binding_generation))).collect::<Result<_>>()?,
                    };
                    let (source, _) = engine.valid_replica(&version.version_id)?;
                    state.placements.insert(version.version_id.clone(), placement.clone());
                    pending.push(Prepared { version: version.clone(), placement, source });
                }
                Ok(json!({"history_tree_id":repo.history_tree_id,"policy":policy,"scope":{"id":scope.id,"kind":scope.kind,"path":scope.path},"file_count":scope.files.len().to_string(),"version_count":versions.len().to_string(),"target_copies":copies,"backup_prefixes":normalized,"configured_at":now()?.to_string()}))
            },
        )
    }
}

/// The exact current configuration delta, shared by production planning and
/// publication replay. No State clone or payload capability is held here.
pub(crate) struct CurrentBackupConfigurationPlan {
    pub(crate) policies: BTreeMap<String, Option<ProtectionPolicy>>,
    pub(crate) bindings: BTreeMap<String, String>,
    pub(crate) paths: BTreeMap<String, BTreeMap<String, String>>,
    pub(crate) placements: BTreeMap<String, Placement>,
    pub(crate) versions: Vec<FileVersion>,
    pub(crate) result: json_codec::Value,
}

fn apply_current_plan(
    engine: &Engine,
    state: &mut State,
    pending: &mut Vec<Prepared>,
    plan: CurrentBackupConfigurationPlan,
) -> Result<json_codec::Value> {
    for version in &plan.versions {
        let source = engine.valid_backup_replica(&version.version_id)?;
        pending.push(Prepared {
            version: version.clone(),
            placement: plan.placements[&version.version_id].clone(),
            source,
        });
    }
    plan.apply(state);
    let mut result = plan.result;
    result["configured_at"] = json!(now()?.to_string());
    Ok(result)
}

impl CurrentBackupConfigurationPlan {
    fn apply(&self, state: &mut State) {
        for (id, policy) in &self.policies {
            match policy {
                Some(policy) => {
                    state.protection_policies.insert(id.clone(), policy.clone());
                }
                None => {
                    state.protection_policies.remove(id);
                }
            }
        }
        for (file, policy) in &self.bindings {
            state
                .file_backup_policies
                .insert(file.clone(), policy.clone());
        }
        for (file, paths) in &self.paths {
            state.file_backup_paths.insert(file.clone(), paths.clone());
        }
        for (version, placement) in &self.placements {
            state.placements.insert(version.clone(), placement.clone());
        }
    }
}

fn configured_primary<'a>(state: &'a State, repository: &str, path: &str) -> &'a str {
    state
        .placement_rules
        .values()
        .filter(|rule| rule.repository_id == repository && super::applies(&rule.path_prefix, path))
        .max_by_key(|rule| {
            rule.path_prefix
                .split('/')
                .filter(|part| !part.is_empty())
                .count()
        })
        .map(|rule| rule.primary_slot_id.as_str())
        .unwrap_or(&state.repositories[repository].primary_slot_id)
}

impl CurrentNamespaces {
    pub(crate) fn plan_backup_configuration(
        &self,
        state: &State,
        request: ScopedBackupConfigurationRequest<'_>,
        fresh_policy: impl FnMut() -> Result<String>,
    ) -> Result<CurrentBackupConfigurationPlan> {
        self.plan_backup_configuration_with_paths(state, request, None, fresh_policy)
    }

    fn plan_backup_configuration_with_paths(
        &self,
        state: &State,
        request: ScopedBackupConfigurationRequest<'_>,
        recursive: Option<recursive::RecursivePaths<'_>>,
        mut fresh_policy: impl FnMut() -> Result<String>,
    ) -> Result<CurrentBackupConfigurationPlan> {
        let repository = request.repository_id;
        let mut scope = source_scope(state, Some(self), repository, request.path)?;
        if recursive.is_some_and(|paths| paths.mode == RecursiveBackupMode::DirectoryTree)
            && scope.kind != NamespaceEntryKind::Directory
        {
            return Err(Error::new(
                ErrorCode::OperationUnsupported,
                "a complete recursive backup snapshot requires a directory scope",
            ));
        }
        let repo = &state.repositories[repository];
        let head = namespace(state, repository)?;
        let entries = self.branch_entries(&repo.main_branch_id, &head.commit_id)?;
        if request
            .expected_scope_id
            .is_some_and(|expected| expected != scope.id)
        {
            return Err(Error::new(
                ErrorCode::ScopeChanged,
                "backup path now describes a different file or directory",
            ));
        }
        let previous = exact_policy(state, repository, &scope);
        let generation = previous.map_or(0, |policy| policy.generation);
        if generation != request.expected_generation {
            return Err(Error::new(
                ErrorCode::GenerationMismatch,
                "backup configuration changed; reread before updating",
            ));
        }
        retain_inherited_files(state, entries, repository, &mut scope, previous);
        let mut prefixes = BTreeMap::new();
        for (slot, prefix) in request.backup_prefixes {
            let disk = self.backup_configuration_disk(slot)?;
            if disk.level <= 1 {
                return Err(Error::new(
                    ErrorCode::InvalidBackupPrefix,
                    "backup prefixes may only target backup slots",
                ));
            }
            if !prefix.is_empty() {
                name_policy::validate_path(prefix)?;
            }
            if prefixes
                .insert(disk.identity.slot_id.clone(), prefix.clone())
                .is_some()
            {
                return Err(Error::new(
                    ErrorCode::InvalidBackupPrefix,
                    "backup slot is listed more than once",
                ));
            }
        }
        let mut primaries = BTreeSet::from([configured_primary(state, repository, &scope.path)]);
        if scope.kind == NamespaceEntryKind::Directory {
            for rule in state.placement_rules.values().filter(|rule| {
                rule.repository_id == repository && within(&scope.path, &rule.path_prefix)
            }) {
                if inherited_by_scope(
                    entries,
                    &scope.path,
                    previous,
                    effective_policy_in(state, entries, repository, None, &rule.path_prefix),
                ) {
                    primaries.insert(&rule.primary_slot_id);
                }
            }
        }
        for entry in scope.files.values() {
            primaries.insert(configured_primary(state, repository, entry.path()));
        }
        let versions: Vec<_> = state
            .versions
            .values()
            .filter(|version| {
                scope.files.contains_key(&version.file_id)
                    && state
                        .content_states
                        .get(&version.version_id)
                        .is_none_or(|content| *content == ContentState::Available)
            })
            .cloned()
            .collect();
        let mut placements = BTreeMap::new();
        for version in &versions {
            self.require_backup_configuration_source(state, version)?;
            let previous = state.placements.get(&version.version_id).ok_or_else(|| {
                Error::new(
                    ErrorCode::PlacementNotFound,
                    "backup source placement missing",
                )
            })?;
            primaries.insert(&previous.primary_slot_id);
            placements.insert(
                version.version_id.clone(),
                self.backup_configuration_placement(
                    &version.version_id,
                    &previous.primary_slot_id,
                    request.target_copies,
                    state.topology_generation,
                )?,
            );
        }
        for primary in primaries {
            let route = self.backup_configuration_route(primary, request.target_copies)?;
            if let Some(paths) = recursive {
                paths
                    .capture
                    .extend_prefixes(repo, &scope, &route, &mut prefixes)?;
            }
            for slot in route.into_iter().skip(1) {
                if !prefixes.contains_key(&slot) {
                    return Err(Error::new(
                        ErrorCode::BackupPrefixRequired,
                        format!("backup slot {slot} requires a user-chosen prefix"),
                    ));
                }
            }
        }
        let mut generated = Vec::new();
        let mut fresh = || -> Result<String> {
            let id = fresh_policy()?;
            validate_id(&id, "policy")?;
            if state.protection_policies.contains_key(&id) || generated.contains(&id) {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "backup configuration reused an existing policy identity",
                ));
            }
            generated.push(id.clone());
            Ok(id)
        };
        let policy = ProtectionPolicy {
            policy_id: match previous {
                Some(policy) => policy.policy_id.clone(),
                None => fresh()?,
            },
            repository_id: repository.into(),
            path_prefix: scope.path.clone(),
            target_copies: request.target_copies,
            inheritable: true,
            scope_directory_id: (scope.kind == NamespaceEntryKind::Directory)
                .then_some(scope.id.clone()),
            scope_file_id: (scope.kind == NamespaceEntryKind::File).then_some(scope.id.clone()),
            backup_prefixes: prefixes.clone(),
            generation: generation.checked_add(1).ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "backup policy generation exhausted",
                )
            })?,
        };
        let mut policies = BTreeMap::new();
        let mut bindings = BTreeMap::new();
        if let Some(previous) = previous {
            let detached: Vec<_> = state
                .file_backup_policies
                .iter()
                .filter(|(file, id)| *id == &previous.policy_id && !scope.files.contains_key(*file))
                .map(|(file, _)| file.clone())
                .collect();
            if !detached.is_empty() {
                let mut retained = previous.clone();
                retained.policy_id = fresh()?;
                retained.inheritable = false;
                for file in detached {
                    bindings.insert(file, retained.policy_id.clone());
                }
                policies.insert(retained.policy_id.clone(), Some(retained));
            }
        }
        let mut paths = BTreeMap::new();
        for (file, entry) in &scope.files {
            let relative = relative_path(
                &scope.path,
                entry.path(),
                scope.kind == NamespaceEntryKind::File,
            );
            if !prefixes.is_empty() {
                let mut changed = state
                    .file_backup_paths
                    .get(file)
                    .cloned()
                    .unwrap_or_default();
                for (slot, prefix) in &prefixes {
                    changed.insert(slot.clone(), backup_path(prefix, &relative)?);
                }
                paths.insert(file.clone(), changed);
            }
            bindings.insert(file.clone(), policy.policy_id.clone());
        }
        policies.insert(policy.policy_id.clone(), Some(policy.clone()));
        let referenced: BTreeSet<_> = state
            .file_backup_policies
            .iter()
            .filter(|(file, _)| !bindings.contains_key(*file))
            .map(|(_, id)| id)
            .chain(bindings.values())
            .collect();
        for (id, old) in state.protection_policies.iter() {
            if old.repository_id == repository && !old.inheritable && !referenced.contains(id) {
                policies.insert(id.clone(), None);
            }
        }
        let directory_namespace = recursive
            .filter(|paths| {
                scope.kind == NamespaceEntryKind::Directory
                    && (paths.mode == RecursiveBackupMode::DirectoryTree || scope.files.is_empty())
            })
            .map(|_| directory_namespaces::capture(self, state, repo, &scope, &policy, &prefixes))
            .transpose()?;
        let mut result = json!({
            "history_tree_id": repo.history_tree_id,
            "policy": policy,
            "scope": {"id":scope.id,"kind":scope.kind,"path":scope.path},
            "file_count":scope.files.len().to_string(),
            "version_count":versions.len().to_string(),
            "target_copies":request.target_copies,"backup_prefixes":prefixes,
            "backup_request": {
                "repository_id":repository,"path":request.path,
                "expected_scope_id":request.expected_scope_id,
                "target_copies":request.target_copies,"backup_prefixes":request.backup_prefixes,
                "expected_generation":request.expected_generation.to_string()
            },
            "generated_policy_ids":generated,
            "namespace_backup":self.backup_configuration_source(state, repository, &scope.id)?
        });
        if let Some(namespace) = directory_namespace {
            result["directory_namespace"] = namespace;
        }
        Ok(CurrentBackupConfigurationPlan {
            policies,
            bindings,
            paths,
            placements,
            versions,
            result,
        })
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/protection/backup_paths/tests.rs"]
mod tests;

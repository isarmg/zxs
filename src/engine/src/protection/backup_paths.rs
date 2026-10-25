//! Stable backup roots and immediate re-protection reuse immutable file bodies.
use super::*;
use crate::{
    business::{find_unordered, now},
    engine::Prepared,
};
use json_codec::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Backup placement for a path, fenced by the current policy generation.
#[derive(Clone, Copy, Debug)]
pub struct BackupConfigurationRequest<'a> {
    pub repository_id: &'a str,
    pub path: &'a str,
    pub target_copies: u32,
    pub backup_prefixes: &'a BTreeMap<String, String>,
    pub expected_generation: u64,
    pub principal: &'a str,
    pub operation_id: &'a str,
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
    pub operation_id: &'a str,
}

fn namespace<'a>(state: &'a State, repository: &str) -> Result<&'a Commit> {
    let repo = state
        .repositories
        .get(repository)
        .ok_or_else(|| Error::new("REPOSITORY_NOT_FOUND", "backup repository missing"))?;
    state
        .branches
        .get(&repo.main_branch_id)
        .and_then(|branch| state.commits.get(&branch.head_commit_id))
        .ok_or_else(|| Error::new("STATE_INVALID", "backup namespace missing"))
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
    if policy.scope_directory_id.as_deref() == Some(policy.repository_id.as_str()) {
        return Some("");
    }
    let commit = namespace(state, &policy.repository_id).ok()?;
    if let Some(directory) = &policy.scope_directory_id {
        return commit
            .entries
            .iter()
            .find(|entry| entry.kind == "directory" && entry.entry_id == *directory)
            .map(|entry| entry.path.as_str());
    }
    if let Some(file) = &policy.scope_file_id {
        return commit
            .entries
            .iter()
            .find(|entry| entry.kind == "file" && entry.file_id == *file)
            .map(|entry| entry.path.as_str());
    }
    Some(&policy.path_prefix)
}
fn policy_applies(
    state: &State,
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
    scope_path(state, policy).is_some_and(|root| within(root, path))
}
pub(super) fn effective_policy<'a>(
    state: &'a State,
    repository: &str,
    file: Option<&str>,
    path: &str,
) -> Option<&'a ProtectionPolicy> {
    let file = file.or_else(|| {
        namespace(state, repository)
            .ok()
            .and_then(|commit| find_unordered(&commit.entries, path))
            .filter(|entry| entry.kind == "file")
            .map(|entry| entry.file_id.as_str())
    });
    if let Some(policy) = file
        .and_then(|file| state.file_backup_policies.get(file))
        .and_then(|id| state.protection_policies.get(id))
        .filter(|policy| policy.repository_id == repository)
    {
        return Some(policy);
    }
    state
        .protection_policies
        .values()
        .filter(|policy| {
            policy.repository_id == repository && policy_applies(state, policy, file, path)
        })
        .max_by_key(|policy| {
            let depth = scope_path(state, policy).map_or(0, |path| {
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
    kind: String,
    path: String,
    files: BTreeMap<String, Entry>,
}
fn source_scope(state: &State, repository: &str, path: &str) -> Result<Scope> {
    if !path.is_empty() {
        name_policy::validate_path(path)?;
    }
    let commit = namespace(state, repository)?;
    let mut scope = if path.is_empty() {
        Scope {
            id: repository.into(),
            kind: "directory".into(),
            path: String::new(),
            files: BTreeMap::new(),
        }
    } else {
        let entry = find_unordered(&commit.entries, path)
            .ok_or_else(|| Error::new("ENTRY_NOT_FOUND", "backup scope no longer exists"))?;
        Scope {
            id: if entry.kind == "file" {
                entry.file_id.clone()
            } else {
                entry.entry_id.clone()
            },
            kind: entry.kind.clone(),
            path: entry.path.clone(),
            files: BTreeMap::new(),
        }
    };
    for entry in commit.entries.iter().filter(|entry| {
        entry.kind == "file"
            && if scope.kind == "file" {
                entry.file_id == scope.id
            } else {
                within(&scope.path, &entry.path)
            }
    }) {
        let prefer = state
            .file_locations
            .get(&entry.file_id)
            .is_some_and(|owner| name_policy::path_key(owner) == entry.lookup_key);
        if prefer || !scope.files.contains_key(&entry.file_id) {
            scope.files.insert(entry.file_id.clone(), entry.clone());
        }
    }
    // A configured directory protects files retained by its other named views,
    // including versions absent from the currently selected subtree.
    if scope.kind == "directory" {
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
            for entry in branch.entries.iter().filter(|entry| entry.kind == "file") {
                scope.files.entry(entry.file_id.clone()).or_insert_with(|| {
                    let mut entry = entry.clone();
                    let suffix = entry
                        .path
                        .split('/')
                        .skip(depth)
                        .collect::<Vec<_>>()
                        .join("/");
                    entry.path = if scope.path.is_empty() {
                        suffix
                    } else {
                        format!("{}/{suffix}", scope.path)
                    };
                    entry.lookup_key = name_policy::path_key(&entry.path);
                    entry
                });
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
                && if scope.kind == "file" {
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
    state: &State,
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
                && scope_path(state, effective).is_some_and(|root| within(root, scope_root))))
}
fn retain_inherited_files(
    state: &State,
    repository: &str,
    scope: &mut Scope,
    previous: Option<&ProtectionPolicy>,
) {
    if scope.kind != "directory" {
        return;
    }
    scope.files.retain(|file, entry| {
        inherited_by_scope(
            state,
            &scope.path,
            previous,
            effective_policy(state, repository, Some(file), &entry.path),
        )
    });
}

fn backup_primaries(engine: &Engine, repository: &str, scope: &Scope) -> Result<BTreeSet<String>> {
    let mut primaries = BTreeSet::from([engine
        .file_placement_settings(repository, None, &scope.path)?
        .0]);
    // Empty and future descendants also inherit this directory's settings.
    // Independent child backup policies do not constrain the parent's routes.
    if scope.kind == "directory" {
        let previous = exact_policy(&engine.state, repository, scope);
        for rule in engine.state.placement_rules.values().filter(|rule| {
            rule.repository_id == repository && within(&scope.path, &rule.path_prefix)
        }) {
            if inherited_by_scope(
                &engine.state,
                &scope.path,
                previous,
                effective_policy(&engine.state, repository, None, &rule.path_prefix),
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
                .file_placement_settings(repository, Some(file), &entry.path)?
                .0,
        );
    }
    for version in engine.state.versions.values().filter(|version| {
        scope.files.contains_key(&version.file_id)
            && engine
                .state
                .content_states
                .get(&version.version_id)
                .map(String::as_str)
                .unwrap_or("AVAILABLE")
                == "AVAILABLE"
    }) {
        let placement = engine
            .state
            .placements
            .get(&version.version_id)
            .ok_or_else(|| Error::new("PLACEMENT_NOT_FOUND", "backup source placement missing"))?;
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
    state.protection_policies.retain(|id, policy| {
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
    repository: &str,
    file: &str,
    path: &str,
    route: &[String],
) -> Result<()> {
    let Some(policy) = effective_policy(state, repository, Some(file), path).cloned() else {
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
    let root = scope_path(state, &policy).unwrap_or(&policy.path_prefix);
    let relative = relative_path(root, path, policy.scope_file_id.is_some());
    let locations = route
        .iter()
        .skip(1)
        .map(|slot| {
            let prefix = policy.backup_prefixes.get(slot).ok_or_else(|| {
                Error::new(
                    "BACKUP_PREFIX_REQUIRED",
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
    pub fn backup_configuration(&self, repository: &str, path: &str) -> Result<Value> {
        let mut scope = source_scope(&self.state, repository, path)?;
        let exact = exact_policy(&self.state, repository, &scope);
        let effective = effective_policy(
            &self.state,
            repository,
            (scope.kind == "file").then_some(scope.id.as_str()),
            &scope.path,
        );
        let inherited = effective.is_some_and(|policy| {
            policy.inheritable && exact.is_none_or(|exact| exact.policy_id != policy.policy_id)
        });
        retain_inherited_files(&self.state, repository, &mut scope, exact);
        let (primary, copies) = self.file_placement_settings(
            repository,
            (scope.kind == "file").then_some(scope.id.as_str()),
            &scope.path,
        )?;
        // GET and PUT use the same future and historical source chains.
        let primaries = backup_primaries(self, repository, &scope)?;
        let mut routes = Vec::new();
        for source in primaries {
            let mut route = self.route(&source, 1)?;
            while route.len() < self.config.disks.len() {
                match self.route(&source, route.len() as u32 + 1) {
                    Ok(next) => route = next,
                    Err(error) if error.code() == "ROUTE_INCOMPLETE" => break,
                    Err(error) => return Err(error),
                }
            }
            routes.push(route);
        }
        let available_route = routes
            .iter()
            .find(|route| route.first() == Some(&primary))
            .cloned()
            .expect("default primary chain is included");
        let maximum = routes.iter().map(Vec::len).min().unwrap_or(1);
        let requested_route = match self.route(&primary, copies) {
            Ok(route) => route,
            Err(error) if error.code() == "ROUTE_INCOMPLETE" => Vec::new(),
            Err(error) => return Err(error),
        };
        let backup_slots = routes
            .iter()
            .flat_map(|route| route.iter().skip(1))
            .cloned()
            .collect::<BTreeSet<_>>();
        Ok(
            json!({"scope":{"id":scope.id,"kind":scope.kind,"path":scope.path},"policy_id":exact.map(|policy|&policy.policy_id),"effective_policy_id":effective.map(|policy|&policy.policy_id),"generation":exact.map_or(0,|policy|policy.generation).to_string(),"effective_generation":effective.map_or(0,|policy|policy.generation).to_string(),"inherited":inherited,"target_copies":copies,"backup_prefixes":effective.map(|policy|policy.backup_prefixes.clone()).unwrap_or_default(),"route_slot_ids":requested_route,"available_route_slot_ids":available_route,"backup_routes":routes,"backup_slot_ids":backup_slots,"max_copies":maximum,"file_count":scope.files.len().to_string(),"backup_paths":if scope.kind=="file"{self.state.file_backup_paths.get(&scope.id).cloned().unwrap_or_default()}else{BTreeMap::new()}}),
        )
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
                let repo = state.repositories.get(repository).cloned()
                    .ok_or_else(|| Error::new("REPOSITORY_NOT_FOUND", "backup repository missing"))?;

                let mut scope = source_scope(state, repository, path)?;
                if expected_scope_id.is_some_and(|expected| expected != scope.id) {
                    return Err(Error::new("SCOPE_CHANGED", "backup path now describes a different file or directory"));
                }
                let previous = exact_policy(state, repository, &scope).cloned();
                let generation = previous.as_ref().map_or(0, |policy| policy.generation);
                if generation != expected_generation {
                    return Err(Error::new("GENERATION_MISMATCH", "backup configuration changed; reread before updating"));
                }
                // An ancestor cannot silently replace independent child rules
                // or the binding of a file moved from another protected scope.
                retain_inherited_files(state, repository, &mut scope, previous.as_ref());
                let mut normalized = BTreeMap::new();
                for (slot, prefix) in prefixes {
                    let disk = engine.disk(slot)?;
                    if disk.level <= 1 {
                        return Err(Error::new("INVALID_BACKUP_PREFIX", "backup prefixes may only target backup slots"));
                    }
                    if !prefix.is_empty() { name_policy::validate_path(prefix)?; }
                    if normalized.insert(disk.slot_id.clone(), prefix.clone()).is_some() {
                        return Err(Error::new("INVALID_BACKUP_PREFIX", "backup slot is listed more than once"));
                    }
                }
                let mut required = BTreeSet::new();
                for primary in backup_primaries(engine, repository, &scope)? {
                    required.extend(engine.route(&primary, copies)?.into_iter().skip(1));
                }
                let versions = state.versions.values().filter(|version| {
                    scope.files.contains_key(&version.file_id)
                        && state.content_states.get(&version.version_id).map(String::as_str).unwrap_or("AVAILABLE") == "AVAILABLE"
                }).cloned().collect::<Vec<_>>();
                for slot in required {
                    if !normalized.contains_key(&slot) {
                        return Err(Error::new("BACKUP_PREFIX_REQUIRED", format!("backup slot {slot} requires a user-chosen prefix")));
                    }
                }
                let policy = ProtectionPolicy {
                    policy_id: previous.as_ref().map(|policy| policy.policy_id.clone()).unwrap_or_else(|| new_id("policy")),
                    repository_id: repository.into(), path_prefix: scope.path.clone(), target_copies: copies,
                    inheritable: true,
                    scope_directory_id: (scope.kind == "directory").then_some(scope.id.clone()),
                    scope_file_id: (scope.kind == "file").then_some(scope.id.clone()),
                    backup_prefixes: normalized.clone(),
                    generation: generation.checked_add(1).ok_or_else(|| Error::new("RESOURCE_LIMIT", "backup policy generation exhausted"))?,
                };
                if let Some(previous) = previous {
                    let detached = state.file_backup_policies.iter()
                        .filter(|(file, id)| *id == &previous.policy_id && !scope.files.contains_key(*file))
                        .map(|(file, _)| file.clone()).collect::<Vec<_>>();
                    if !detached.is_empty() {
                        let mut retained = previous;
                        retained.policy_id = new_id("policy");
                        retained.inheritable = false;
                        for file in detached {
                            state.file_backup_policies.insert(file, retained.policy_id.clone());
                        }
                        state.protection_policies.insert(retained.policy_id.clone(), retained);
                    }
                }
                for (file, entry) in &scope.files {
                    let relative = relative_path(&scope.path, &entry.path, scope.kind == "file");
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
                Ok(json!({"history_tree_id":repo.history_tree_id,"policy":policy,"scope":{"id":scope.id,"kind":scope.kind,"path":scope.path},"file_count":scope.files.len().to_string(),"version_count":versions.len().to_string(),"target_copies":copies,"backup_prefixes":normalized,"configured_at":now().to_string()}))
            },
        )
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/protection/backup_paths/tests.rs"]
mod tests;

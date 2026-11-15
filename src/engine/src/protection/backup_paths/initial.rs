//! One retained-policy and placement transformation for initial namespaces.
//! Callers select their own path protocol and supply the actual copy source.
use crate::engine::PlanningContext;
use crate::protection::backup_paths::{
    Scope, ScopedBackupConfigurationRequest, backup_path, exact_policy, prune_retained_policies,
    relative_path, retain_initial_inherited_files, source_scope,
};
use crate::{ContentState, Error, ErrorCode, FileVersion, NamespaceEntryKind, Placement,
    ProtectionPolicy, Result, State};
use std::collections::BTreeMap;

pub(crate) struct Configuration {
    pub(crate) policy: ProtectionPolicy,
    pub(crate) scope: storage_contract::BackupScope,
    pub(crate) file_count: usize,
    pub(crate) version_count: usize,
}

pub(super) fn select(
    state: &State,
    request: ScopedBackupConfigurationRequest<'_>,
) -> Result<(Scope, Option<ProtectionPolicy>)> {
    let repository = request.repository_id;
    let mut scope = source_scope(state, None, repository, request.path)?;
    if request.expected_scope_id.is_some_and(|expected| expected != scope.id) {
        return Err(Error::new(ErrorCode::ScopeChanged,
            "backup path now describes a different file or directory"));
    }
    let previous = exact_policy(state, repository, &scope).cloned();
    let generation = previous.as_ref().map_or(0, |policy| policy.generation);
    if generation != request.expected_generation {
        return Err(Error::new(ErrorCode::GenerationMismatch,
            "backup configuration changed; reread before updating"));
    }
    retain_initial_inherited_files(state, repository, &mut scope, previous.as_ref());
    Ok((scope, previous))
}

pub(super) fn apply<T>(
    context: &PlanningContext<'_>,
    state: &mut State,
    request: ScopedBackupConfigurationRequest<'_>,
    scope: Scope,
    previous: Option<ProtectionPolicy>,
    normalized: BTreeMap<String, String>,
    mut fresh_policy: impl FnMut() -> Result<String>,
    pending: &mut Vec<T>,
    mut prepare: impl FnMut(&FileVersion, &Placement) -> Result<T>,
) -> Result<Configuration> {
    let repository = request.repository_id;
    let copies = request.target_copies;
    let versions = state.versions.values().filter(|version| {
        scope.files.contains_key(&version.file_id)
            && state.content_states.get(&version.version_id)
                .is_none_or(|state| *state == ContentState::Available)
    }).cloned().collect::<Vec<_>>();
    let generation = previous.as_ref().map_or(0, |policy| policy.generation);
    let policy = ProtectionPolicy {
        policy_id: match previous.as_ref() {
            Some(policy) => policy.policy_id.clone(),
            None => fresh_policy()?,
        },
        repository_id: repository.into(),
        path_prefix: scope.path.clone(), target_copies: copies, inheritable: true,
        scope_directory_id: (scope.kind == NamespaceEntryKind::Directory).then_some(scope.id.clone()),
        scope_file_id: (scope.kind == NamespaceEntryKind::File).then_some(scope.id.clone()),
        backup_prefixes: normalized.clone(),
        generation: generation.checked_add(1).ok_or_else(|| Error::new(
            ErrorCode::ResourceLimit, "backup policy generation exhausted"))?,
    };
    if let Some(previous) = previous {
        let detached = state.file_backup_policies.iter()
            .filter(|(file, id)| *id == &previous.policy_id && !scope.files.contains_key(*file))
            .map(|(file, _)| file.clone()).collect::<Vec<_>>();
        if !detached.is_empty() {
            let mut retained = previous;
            retained.policy_id = fresh_policy()?;
            retained.inheritable = false;
            for file in detached {
                state.file_backup_policies.insert(file, retained.policy_id.clone());
            }
            state.protection_policies.insert(retained.policy_id.clone(), retained);
        }
    }
    for (file, entry) in &scope.files {
        let relative = relative_path(&scope.path, entry.path(),
            scope.kind == NamespaceEntryKind::File);
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
        let route = context.route(&previous.primary_slot_id, copies)?;
        let placement = Placement {
            version_id: version.version_id.clone(), primary_slot_id: previous.primary_slot_id,
            target_copies: copies, route_slot_ids: route.clone(),
            topology_generation: state.topology_generation,
            binding_generations: route.iter().map(|slot| {
                Ok((slot.clone(), context.disk(slot)?.binding_generation))
            }).collect::<Result<_>>()?,
        };
        let payload = prepare(version, &placement)?;
        state.placements.insert(version.version_id.clone(), placement);
        pending.push(payload);
    }
    Ok(Configuration {
        policy, file_count: scope.files.len(), version_count: versions.len(),
        scope: storage_contract::BackupScope {
            id: scope.id, kind: scope.kind, path: scope.path,
        },
    })
}

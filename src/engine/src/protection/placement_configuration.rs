//! Object-bound editing of current path rules, without moving old versions.
use super::*;

#[derive(Clone, Copy, Debug)]
pub struct PlacementConfigurationRequest<'a> {
    pub repository_id: &'a str,
    pub path: &'a str,
    pub primary_slot_id: &'a str,
    pub expected_scope_id: &'a str,
    pub expected_rule_id: Option<&'a str>,
    pub expected_generation: u64,
    pub principal: &'a str,
    pub operation_id: OperationId,
}

fn directory_scope(engine: &Engine, repository: &str, path: &str) -> Result<PlacementScope> {
    let repo = engine.state().repositories.get(repository).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::RepositoryNotFound,
            "placement repository missing",
        )
    })?;
    let namespace = engine.namespace_tree(&repo.main_branch_id)?;
    if let Some(current) = engine.admitted_current_namespaces()? {
        current.require_branch_write(engine.state(), &repo.main_branch_id)?;
    }
    if path.is_empty() {
        return Ok(PlacementScope {
            id: repository.into(),
            path: String::new(),
        });
    }
    let requested = name_policy::PortablePath::parse(path)?;
    let entry = namespace.get_key(requested.key()).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::EntryNotFound,
            "placement directory no longer exists",
        )
    })?;
    if entry.kind != NamespaceEntryKind::Directory {
        return Err(Error::new(
            storage_contract::ErrorCode::InvalidRequest,
            "primary placement requires a directory",
        ));
    }
    Ok(PlacementScope {
        id: entry.entry_id.clone(),
        path: entry.path().into(),
    })
}

fn exact_rule<'a>(state: &'a State, repository: &str, path: &str) -> Option<&'a PlacementRule> {
    let key = name_policy::path_key(path);
    state.placement_rules.values().find(|rule| {
        rule.repository_id == repository && name_policy::path_key(&rule.path_prefix) == key
    })
}

fn within_key(root: &str, path: &str) -> bool {
    root.is_empty()
        || root == path
        || path
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

struct Requirements<'a> {
    copies: u32,
    policies: std::collections::BTreeMap<&'a str, &'a ProtectionPolicy>,
}

/// Independent child placement rules retain their own routes. Explicit backup
/// prefixes must also fit: chain length alone does not make a new primary safe.
fn requirements<'a>(engine: &'a Engine, repository: &str, root: &str) -> Result<Requirements<'a>> {
    let state = engine.state();
    let root_key = name_policy::path_key(root);
    let child_keys = state
        .placement_rules
        .values()
        .filter(|rule| rule.repository_id == repository)
        .map(|rule| name_policy::path_key(&rule.path_prefix))
        .filter(|key| key != &root_key && within_key(&root_key, key))
        .collect::<Vec<_>>();
    let affected = |key: &str| {
        within_key(&root_key, key) && !child_keys.iter().any(|child| within_key(child, key))
    };
    let repo = &state.repositories[repository];
    let current = engine.namespace_tree(&repo.main_branch_id)?;
    let namespaces = engine.admitted_current_namespaces()?;
    let mut requirements = Requirements {
        copies: 1,
        policies: std::collections::BTreeMap::new(),
    };
    let mut include = |file: Option<&str>, path: &str| {
        if let Some(policy) =
            backup_paths::effective_policy_in(state, current.entries(), repository, file, path)
        {
            requirements.copies = requirements.copies.max(policy.target_copies);
            if !policy.backup_prefixes.is_empty() {
                requirements
                    .policies
                    .insert(policy.policy_id.as_str(), policy);
            }
        } else {
            requirements.copies = requirements.copies.max(repo.target_copies);
        }
    };
    include(None, root);
    // Only admitted current occurrences constrain future writes. Immutable
    // heads and file_locations retain removed identities for history.
    for branch in state.branches.values().filter(|branch| {
        branch.state == BranchState::Active
            && state.histories[&branch.history_tree_id].repository_id == repository
    }) {
        if let Some(namespaces) = namespaces
            && !namespaces.branch_writable(state, &branch.branch_id)?
        {
            continue;
        }
        for entry in engine
            .namespace_tree(&branch.branch_id)?
            .entries()
            .iter()
            .filter(|entry| affected(entry.lookup_key()))
        {
            include(
                (entry.kind == NamespaceEntryKind::File).then_some(entry.file_id.as_str()),
                entry.path(),
            );
        }
    }
    for branch in state
        .directory_branches
        .values()
        .filter(|branch| branch.repository_id == repository)
    {
        if let Some(namespaces) = namespaces
            && !namespaces.directory_writable(state, &branch.directory_branch_id)?
        {
            continue;
        }
        for entry in engine
            .current_directory_entries(&branch.directory_branch_id)?
            .iter()
            .filter(|entry| affected(entry.lookup_key()))
        {
            include(
                (entry.kind == NamespaceEntryKind::File).then_some(entry.file_id.as_str()),
                entry.path(),
            );
        }
    }
    // Future path rules still matter before any file exists. Identity-bound
    // policies whose scope disappeared cannot reserve its former spelling.
    for policy in state
        .protection_policies
        .values()
        .filter(|policy| policy.repository_id == repository && policy.inheritable)
    {
        if let Some(path) = backup_paths::scope_path_in(current.entries(), policy)
            && affected(&name_policy::path_key(path))
        {
            include(None, path);
        }
    }
    Ok(requirements)
}

fn registered_config<'a>(
    engine: &'a Engine,
    registration: RegisteredDisk<'_>,
) -> Result<&'a DiskConfig> {
    let disk = engine.disk(registration.slot_id)?;
    if registration.pool_id != engine.config().pool_id
        || registration.disk_id != disk.disk_id
        || registration.slot_id != disk.slot_id
        || registration.binding_generation != disk.binding_generation
        || registration.level != disk.level
        || registration.upstream_slot_id != disk.upstream_slot_id.as_deref()
    {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "current placement registration differs from the admitted storage binding",
        ));
    }
    Ok(disk)
}

fn current_primary<'a>(engine: &'a Engine, primary: &str) -> Result<&'a DiskConfig> {
    let configured = engine.disk(primary)?;
    let registration = engine.registered_disk(&configured.disk_id).ok_or_else(|| {
        Error::new(
            ErrorCode::SlotNotFound,
            "primary storage slot is no longer registered",
        )
    })?;
    let disk = registered_config(engine, registration)?;
    if registration.level != 1 {
        return Err(Error::new(
            ErrorCode::PlacementInvalid,
            "primary placement requires a first-level storage slot",
        ));
    }
    Ok(disk)
}

fn unavailable_reason(
    engine: &Engine,
    primary: &str,
    requirements: &Requirements<'_>,
) -> Result<Option<PrimaryPlacementBlock>> {
    let primary = current_primary(engine, primary)?;
    let route = match engine.route(&primary.slot_id, requirements.copies) {
        Ok(route) => route,
        Err(error) if error.code() == storage_contract::ErrorCode::RouteIncomplete => {
            return Ok(Some(PrimaryPlacementBlock::InsufficientCopies));
        }
        Err(error) if error.code() == storage_contract::ErrorCode::PlacementInvalid => {
            return Ok(Some(PrimaryPlacementBlock::RouteUnavailable));
        }
        Err(error) => return Err(error),
    };
    for slot in &route {
        let disk = engine.disk(slot)?;
        let registration = engine.registered_disk(&disk.disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::SlotNotFound,
                "placement route storage slot is no longer registered",
            )
        })?;
        registered_config(engine, registration)?;
    }
    if requirements.policies.values().any(|policy| {
        route
            .iter()
            .skip(1)
            .take(policy.target_copies as usize - 1)
            .any(|slot| !policy.backup_prefixes.contains_key(slot))
    }) {
        return Ok(Some(PrimaryPlacementBlock::MissingBackupPrefixes));
    }
    Ok(None)
}

impl Engine {
    pub fn placement_configuration(
        &self,
        repository: &str,
        path: &str,
    ) -> Result<PlacementConfigurationSnapshot> {
        let scope = directory_scope(self, repository, path)?;
        let rule = exact_rule(self.state(), repository, &scope.path);
        let requirements = requirements(self, repository, &scope.path)?;
        let primary_slot_id = self
            .file_placement_settings(repository, None, &scope.path)?
            .0;
        let targets = self
            .registered_disks()
            .filter(|registration| registration.level == 1)
            .map(|registration| {
                let disk = registered_config(self, registration)?;
                Ok(PrimaryPlacementTarget {
                    slot_id: disk.slot_id.clone(),
                    root: disk.root.display().to_string(),
                    unavailable_reason: unavailable_reason(self, &disk.slot_id, &requirements)?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(PlacementConfigurationSnapshot {
            repository_id: repository.into(),
            repository_name: self.state().repositories[repository].name.clone(),
            scope,
            rule_id: rule.map(|rule| rule.rule_id.clone()),
            generation: rule.map_or(0, |rule| rule.generation),
            primary_slot_id,
            inherited: rule.is_none(),
            required_copies: requirements.copies,
            targets,
        })
    }

    pub fn configure_placement(
        &mut self,
        request: PlacementConfigurationRequest<'_>,
    ) -> Result<OperationResult> {
        let PlacementConfigurationRequest {
            repository_id,
            path,
            primary_slot_id,
            expected_scope_id,
            expected_rule_id,
            expected_generation,
            principal,
            operation_id,
        } = request;
        self.mutate(
            "placement.configure",
            principal,
            operation_id,
            json!({"repository_id":repository_id,"path":path,"primary_slot_id":primary_slot_id,
                "expected_scope_id":expected_scope_id,"expected_rule_id":expected_rule_id,
                "expected_generation":expected_generation.to_string()}),
            |engine, state, _pending| {
                let scope = directory_scope(engine, repository_id, path)?;
                if scope.id != expected_scope_id {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ScopeChanged,
                        "placement path now describes a different directory",
                    ));
                }
                let previous = exact_rule(state, repository_id, &scope.path);
                if previous.map(|rule| rule.rule_id.as_str()) != expected_rule_id
                    || previous.map_or(0, |rule| rule.generation) != expected_generation
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::GenerationMismatch,
                        "placement rule changed; reread before updating",
                    ));
                }
                let requirements = requirements(engine, repository_id, &scope.path)?;
                if let Some(reason) = unavailable_reason(engine, primary_slot_id, &requirements)? {
                    let (code, message) = match reason {
                        PrimaryPlacementBlock::InsufficientCopies => (storage_contract::ErrorCode::RouteIncomplete,
                            "selected primary cannot satisfy this directory's backup copy counts"),
                        PrimaryPlacementBlock::MissingBackupPrefixes => (storage_contract::ErrorCode::BackupPrefixRequired,
                            "selected primary needs backup prefixes not configured for this directory"),
                        PrimaryPlacementBlock::RouteUnavailable => (storage_contract::ErrorCode::PlacementInvalid,
                            "selected primary route contains a disabled or deregistered storage slot"),
                    };
                    return Err(Error::new(code, message));
                }
                let rule = PlacementRule {
                    rule_id: match previous {
                        Some(rule) => rule.rule_id.clone(),
                        None => new_id("rule")?,
                    },
                    repository_id: repository_id.into(),
                    path_prefix: scope.path,
                    primary_slot_id: engine.disk(primary_slot_id)?.slot_id.clone(),
                    generation: expected_generation.checked_add(1).ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::ResourceLimit,
                            "placement rule generation exhausted",
                        )
                    })?,
                };
                state
                    .placement_rules
                    .insert(rule.rule_id.clone(), rule.clone());
                Ok(json!({"rule":rule}))
            },
        )
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/protection/placement_configuration.rs"]
mod tests;

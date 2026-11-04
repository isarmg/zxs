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

fn directory_scope(state: &State, repository: &str, path: &str) -> Result<PlacementScope> {
    let repo = state.repositories.get(repository).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::RepositoryNotFound,
            "placement repository missing",
        )
    })?;
    if path.is_empty() {
        return Ok(PlacementScope {
            id: repository.into(),
            path: String::new(),
        });
    }
    let requested = name_policy::PortablePath::parse(path)?;
    let branch = &state.branches[&repo.main_branch_id];
    let commit = &state.commits[&branch.head_commit_id];
    let entry =
        crate::business::find_unordered_key(&commit.entries, requested.key()).ok_or_else(|| {
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
fn requirements<'a>(engine: &'a Engine, repository: &str, root: &str) -> Requirements<'a> {
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
    let branch = &state.branches[&repo.main_branch_id];
    let commit = &state.commits[&branch.head_commit_id];
    let mut requirements = Requirements {
        copies: 1,
        policies: std::collections::BTreeMap::new(),
    };
    let mut include = |file: Option<&str>, path: &str| {
        if let Some(policy) = backup_paths::effective_policy(state, repository, file, path) {
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
    for entry in commit
        .entries
        .iter()
        .filter(|entry| affected(entry.lookup_key()))
    {
        include(
            (entry.kind == NamespaceEntryKind::File).then_some(entry.file_id.as_str()),
            entry.path(),
        );
    }
    // Another named view can still write files absent from the main namespace.
    for (file, policy_id) in state.file_backup_policies.iter() {
        if state.protection_policies[policy_id].repository_id == repository
            && let Some(path) = state.file_locations.get(file)
            && affected(&name_policy::path_key(path))
        {
            include(Some(file), path);
        }
    }
    for policy in state
        .protection_policies
        .values()
        .filter(|policy| policy.repository_id == repository)
    {
        if let Some(path) = backup_paths::scope_path(state, policy)
            && affected(&name_policy::path_key(path))
        {
            include(None, path);
        }
    }
    requirements
}

fn unavailable_reason(
    engine: &Engine,
    primary: &str,
    requirements: &Requirements<'_>,
) -> Result<Option<PrimaryPlacementBlock>> {
    let route = match engine.route(primary, requirements.copies) {
        Ok(route) => route,
        Err(error) if error.code() == storage_contract::ErrorCode::RouteIncomplete => {
            return Ok(Some(PrimaryPlacementBlock::InsufficientCopies));
        }
        Err(error) => return Err(error),
    };
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
        let scope = directory_scope(self.state(), repository, path)?;
        let rule = exact_rule(self.state(), repository, &scope.path);
        let requirements = requirements(self, repository, &scope.path);
        let primary_slot_id = self
            .file_placement_settings(repository, None, &scope.path)?
            .0;
        let targets = self
            .config()
            .disks
            .iter()
            .filter(|disk| disk.level == 1)
            .map(|disk| {
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
                let scope = directory_scope(state, repository_id, path)?;
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
                let requirements = requirements(engine, repository_id, &scope.path);
                if let Some(reason) = unavailable_reason(engine, primary_slot_id, &requirements)? {
                    let (code, message) = match reason {
                        PrimaryPlacementBlock::InsufficientCopies => (storage_contract::ErrorCode::RouteIncomplete,
                            "selected primary cannot satisfy this directory's backup copy counts"),
                        PrimaryPlacementBlock::MissingBackupPrefixes => (storage_contract::ErrorCode::BackupPrefixRequired,
                            "selected primary needs backup prefixes not configured for this directory"),
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

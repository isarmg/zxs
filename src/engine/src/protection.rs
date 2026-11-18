//! Conditional policy updates affect future full versions only.
use crate::*;
use json_codec::json;
mod backup_paths;
mod placement_configuration;
pub(crate) use backup_paths::initial::Configuration as InitialBackupConfiguration;
pub(crate) use backup_paths::automatic::{
    DirectoryBackupSelection, VerifiedArchivedAutomaticBackup, candidate_directory_selection,
    verify_archived as verify_archived_automatic_backup,
    verify_addition_candidate_source as verify_automatic_addition_candidate_source,
    invalid as invalid_automatic_backup,
    verify_candidate as verify_automatic_backup_candidate,
    verify_readonly_candidate as verify_readonly_automatic_backup_candidate,
    verify_prepared as verify_automatic_backup_prepared,
};
pub(crate) use backup_paths::directory_namespaces::{
    BATCH_FOLLOW_KIND, BATCH_ROUTE_KIND, DirectoryFollowChange, DirectoryFollowPlan,
    DirectoryNamespace, FILE_FOLLOW_KIND, FILE_ROUTE_KIND, captured_directory_namespace,
    directory_catalog_changed, follows_directories, follows_routes,
    materialize as materialize_backup_directories, preflight as preflight_backup_directories,
    published_directories_at, repair_current as repair_current_backup_directories,
};
pub use backup_paths::{
    AutomaticBackupConfigurationRequest, AutomaticBackupError, AutomaticDirectoryBackupRequest,
    AutomaticFileBackupRequest,
    BackupConfigurationRequest, PublishedBackupDirectory, PublishedBackupFilePath,
    RecursiveBackupCapture,
    RecursiveBackupConfigurationRequest, ScopedBackupConfigurationRequest,
};
pub(crate) use backup_paths::{RecursiveBackupMode, apply_backup_paths, prune_retained_policies};
pub use placement_configuration::PlacementConfigurationRequest;

/// A conditional placement update, including its authenticated actor and replay identity.
#[derive(Clone, Copy, Debug)]
pub struct PlacementRuleUpdate<'a> {
    pub rule_id: &'a str,
    pub repository_id: &'a str,
    pub path_prefix: &'a str,
    pub primary_slot_id: &'a str,
    pub expected_generation: u64,
    pub principal: &'a str,
    pub operation_id: OperationId,
}

/// A conditional protection update for future immutable versions.
#[derive(Clone, Copy, Debug)]
pub struct ProtectionPolicyUpdate<'a> {
    pub policy_id: &'a str,
    pub repository_id: &'a str,
    pub path_prefix: &'a str,
    pub target_copies: u32,
    pub expected_generation: u64,
    pub principal: &'a str,
    pub operation_id: OperationId,
}

fn applies(prefix: &str, path: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    let prefix = name_policy::path_key(prefix);
    let path = name_policy::path_key(path);
    path == prefix || path.starts_with(&format!("{prefix}/"))
}
impl Engine {
    pub fn new_file_placement(&self, repository: &str, path: &str) -> Result<(String, u32)> {
        self.new_file_placement_for_file(repository, None, path)
    }
    pub fn new_file_placement_for_file(
        &self,
        repository: &str,
        file_id: Option<&str>,
        path: &str,
    ) -> Result<(String, u32)> {
        self.planning_context()
            .new_file_placement_for_file(repository, file_id, path)
    }
    // Resolve settings separately so configuration can repair a moved file
    // whose old copy count exceeds its new primary's fixed chain.
    fn file_placement_settings(
        &self,
        repository: &str,
        file_id: Option<&str>,
        path: &str,
    ) -> Result<(String, u32)> {
        self.planning_context()
            .file_placement_settings(repository, file_id, path)
    }
    pub fn update_placement_rule(
        &mut self,
        request: PlacementRuleUpdate<'_>,
    ) -> Result<OperationResult> {
        let PlacementRuleUpdate {
            rule_id: id,
            repository_id: repository,
            path_prefix,
            primary_slot_id: primary,
            expected_generation,
            principal,
            operation_id: operation,
        } = request;
        validate_id(id, "rule")?;
        if !path_prefix.is_empty() {
            name_policy::validate_path(path_prefix)?;
        }
        self.mutate("placement-rule.update",principal,operation,json!({"rule_id":id,"repository_id":repository,"path_prefix":path_prefix,"primary_slot_id":primary,"expected_generation":expected_generation.to_string()}),|engine,state,_|{
            engine.state.repositories.get(repository).ok_or_else(||Error::new(storage_contract::ErrorCode::RepositoryNotFound,"repository does not exist"))?;

            let actual=state.placement_rules.get(id).map_or(0,|r|r.generation);
            if actual!=expected_generation { return Err(Error::new(storage_contract::ErrorCode::GenerationMismatch,"placement rule changed; reread before updating")); }
            if state.placement_rules.get(id).is_some_and(|r|r.repository_id!=repository) { return Err(Error::new(storage_contract::ErrorCode::RequestBindingMismatch,"rule is bound to another repository")); }
            if state.placement_rules.values().any(|r|r.rule_id!=id&&r.repository_id==repository&&name_policy::path_key(&r.path_prefix)==name_policy::path_key(path_prefix)) { return Err(Error::new(storage_contract::ErrorCode::NameCollision,"an equivalent placement prefix already has a rule")); }
            engine.route(primary,engine.file_placement_settings(repository,None,path_prefix)?.1)?;
            let rule=PlacementRule{rule_id:id.into(),repository_id:repository.into(),path_prefix:path_prefix.into(),primary_slot_id:engine.disk(primary)?.slot_id.clone(),generation:actual.checked_add(1).ok_or_else(||Error::new(storage_contract::ErrorCode::ResourceLimit,"rule generation exhausted"))?};
            state.placement_rules.insert(id.into(),rule.clone());
            Ok(json!({"rule":rule}))
        })
    }
    pub fn update_protection_policy(
        &mut self,
        request: ProtectionPolicyUpdate<'_>,
    ) -> Result<OperationResult> {
        let ProtectionPolicyUpdate {
            policy_id: id,
            repository_id: repository,
            path_prefix,
            target_copies: copies,
            expected_generation,
            principal,
            operation_id: operation,
        } = request;
        validate_id(id, "policy")?;
        if !path_prefix.is_empty() {
            name_policy::validate_path(path_prefix)?;
        }
        self.mutate("protection-policy.update",principal,operation,json!({"policy_id":id,"repository_id":repository,"path_prefix":path_prefix,"target_copies":copies,"expected_generation":expected_generation.to_string()}),|engine,state,_|{
            engine.state.repositories.get(repository).ok_or_else(||Error::new(storage_contract::ErrorCode::RepositoryNotFound,"repository does not exist"))?;

            let actual=state.protection_policies.get(id).map_or(0,|p|p.generation);
            if actual!=expected_generation { return Err(Error::new(storage_contract::ErrorCode::GenerationMismatch,"protection policy changed; reread before updating")); }
            if state.protection_policies.get(id).is_some_and(|p|p.repository_id!=repository) { return Err(Error::new(storage_contract::ErrorCode::RequestBindingMismatch,"policy is bound to another repository")); }
            if state.protection_policies.get(id).is_some_and(|p|!p.inheritable||p.scope_directory_id.is_some()||p.scope_file_id.is_some()) { return Err(Error::new(storage_contract::ErrorCode::BackupConfigurationRequired,"scoped and retained settings must be changed through file or directory backup configuration")); }
            let current = engine.namespace_tree(&state.repositories[repository].main_branch_id)?;
            if state.protection_policies.values().any(|p|p.inheritable&&p.policy_id!=id&&p.repository_id==repository&&backup_paths::scope_path_in(current.entries(),p).is_some_and(|path|name_policy::path_key(path)==name_policy::path_key(path_prefix))) { return Err(Error::new(storage_contract::ErrorCode::NameCollision,"an equivalent protection prefix already has a policy")); }
            engine.route(&engine.file_placement_settings(repository,None,path_prefix)?.0,copies)?;
            let previous=state.protection_policies.get(id);
            let policy=ProtectionPolicy{policy_id:id.into(),repository_id:repository.into(),path_prefix:path_prefix.into(),target_copies:copies,inheritable:true,scope_directory_id:None,scope_file_id:None,backup_prefixes:previous.map(|policy|policy.backup_prefixes.clone()).unwrap_or_default(),generation:actual.checked_add(1).ok_or_else(||Error::new(storage_contract::ErrorCode::ResourceLimit,"policy generation exhausted"))?};
            state.protection_policies.insert(id.into(),policy.clone());
            Ok(json!({"policy":policy}))
        })
    }
}

impl crate::engine::PlanningContext<'_> {
    pub fn new_file_placement_for_file(
        &self,
        repository: &str,
        file_id: Option<&str>,
        path: &str,
    ) -> Result<(String, u32)> {
        let (primary, copies) = self.file_placement_settings(repository, file_id, path)?;
        self.route(&primary, copies)?;
        Ok((primary, copies))
    }
}

impl crate::engine::PlanningContext<'_> {
    fn file_placement_settings(
        &self,
        repository: &str,
        file_id: Option<&str>,
        path: &str,
    ) -> Result<(String, u32)> {
        let repo = self.state.repositories.get(repository).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::RepositoryNotFound,
                "repository does not exist",
            )
        })?;
        let entries = self.current_branch_entries(&repo.main_branch_id)?;
        let settings = file_policy_at(self.state, repo, entries, file_id, path);
        let copies = self.current_namespaces.map_or(settings.copies, |view| {
            view.inherited_backup_copies(repo, settings.policy, settings.primary, settings.copies)
        });
        Ok((settings.primary.to_owned(), copies))
    }
}

/// Settings selected by this repository's exact named namespace. Initial uses
/// these settings directly; Current applies its own admitted routing override.
/// This is not a route, registration, physical proof, or permission to mutate.
pub(crate) struct FilePolicyAt<'a> {
    pub(crate) primary: &'a str,
    pub(crate) copies: u32,
    policy: Option<&'a ProtectionPolicy>,
}

pub(crate) fn file_policy_at<'a>(
    state: &'a State,
    repository: &'a Repository,
    entries: &[Entry],
    file: Option<&str>,
    path: &str,
) -> FilePolicyAt<'a> {
    let primary = state.placement_rules.values()
        .filter(|r| r.repository_id == repository.repository_id && applies(&r.path_prefix, path))
        .max_by_key(|r| {
            r.path_prefix.split('/').filter(|component| !component.is_empty()).count()
        })
        .map_or(repository.primary_slot_id.as_str(), |r| r.primary_slot_id.as_str());
    let policy = backup_paths::effective_policy_in(
        state, entries, &repository.repository_id, file, path,
    );
    let copies = policy.map_or(repository.target_copies, |p| p.target_copies);
    FilePolicyAt { primary, copies, policy }
}

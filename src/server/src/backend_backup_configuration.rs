//! Typed backup read and mutation inputs; current scope and routes remain dynamic.
use data_serialization::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct ReadBackupConfiguration {
    pub repository_id: String,
    pub path: String,
}
#[derive(Clone, Debug)]
pub struct ConfigureBackups {
    pub repository_id: String,
    pub path: String,
    pub target_copies: u32,
    pub backup_prefixes: BTreeMap<String, String>,
    pub expected_generation: u64,
    pub expected_scope_id: String,
    pub operation_id: storage_contract::OperationId,
}
#[derive(Serialize)]
pub(crate) struct BackupScopeResponse {
    id: String,
    kind: storage_contract::NamespaceEntryKind,
    path: String,
}
#[derive(Serialize)]
pub(crate) struct BackupConfigurationResponse {
    scope: BackupScopeResponse,
    policy_id: Option<String>,
    effective_policy_id: Option<String>,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    effective_generation: u64,
    inherited: bool,
    target_copies: u32,
    backup_prefixes: BTreeMap<String, String>,
    route_slot_ids: Vec<String>,
    available_route_slot_ids: Vec<String>,
    backup_routes: Vec<Vec<String>>,
    backup_slot_ids: BTreeSet<String>,
    max_copies: usize,
    #[serialization(with = "storage_contract::u64_string")]
    file_count: u64,
    backup_paths: BTreeMap<String, String>,
}
impl From<storage_contract::BackupConfigurationSnapshot> for BackupConfigurationResponse {
    fn from(snapshot: storage_contract::BackupConfigurationSnapshot) -> Self {
        let storage_contract::BackupConfigurationSnapshot {
            scope,
            policy_id,
            effective_policy_id,
            generation,
            effective_generation,
            inherited,
            target_copies,
            backup_prefixes,
            route_slot_ids,
            available_route_slot_ids,
            backup_routes,
            backup_slot_ids,
            max_copies,
            file_count,
            backup_paths,
        } = snapshot;
        let storage_contract::BackupScope { id, kind, path } = scope;
        Self {
            scope: BackupScopeResponse { id, kind, path },
            policy_id,
            effective_policy_id,
            generation,
            effective_generation,
            inherited,
            target_copies,
            backup_prefixes,
            route_slot_ids,
            available_route_slot_ids,
            backup_routes,
            backup_slot_ids,
            max_copies,
            file_count,
            backup_paths,
        }
    }
}

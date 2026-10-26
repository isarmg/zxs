//! Typed policy mutations; the authenticated principal is supplied at admission.
use data_serialization::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct UpdateProtectionPolicy {
    pub policy_id: String,
    pub repository_id: String,
    pub path_prefix: String,
    pub target_copies: u32,
    pub expected_generation: u64,
    pub operation_id: storage_contract::OperationId,
}
#[derive(Clone, Debug)]
pub struct UpdatePlacementRule {
    pub rule_id: String,
    pub repository_id: String,
    pub path_prefix: String,
    pub primary_slot_id: String,
    pub expected_generation: u64,
    pub operation_id: storage_contract::OperationId,
}
#[derive(Serialize)]
pub(crate) struct PolicyResponse {
    policy_id: String,
    repository_id: String,
    path_prefix: String,
    target_copies: u32,
    inheritable: bool,
    #[serialization(skip_serializing_if = "Option::is_none")]
    scope_directory_id: Option<String>,
    #[serialization(skip_serializing_if = "Option::is_none")]
    scope_file_id: Option<String>,
    #[serialization(skip_serializing_if = "BTreeMap::is_empty")]
    backup_prefixes: BTreeMap<String, String>,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
}
impl From<storage_contract::ProtectionPolicy> for PolicyResponse {
    fn from(policy: storage_contract::ProtectionPolicy) -> Self {
        let storage_contract::ProtectionPolicy {
            policy_id,
            repository_id,
            path_prefix,
            target_copies,
            inheritable,
            scope_directory_id,
            scope_file_id,
            backup_prefixes,
            generation,
        } = policy;
        Self {
            policy_id,
            repository_id,
            path_prefix,
            target_copies,
            inheritable,
            scope_directory_id,
            scope_file_id,
            backup_prefixes,
            generation,
        }
    }
}
#[derive(Serialize)]
pub(crate) struct PlacementRuleResponse {
    rule_id: String,
    repository_id: String,
    path_prefix: String,
    primary_slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
}
impl From<storage_contract::PlacementRule> for PlacementRuleResponse {
    fn from(rule: storage_contract::PlacementRule) -> Self {
        let storage_contract::PlacementRule {
            rule_id,
            repository_id,
            path_prefix,
            primary_slot_id,
            generation,
        } = rule;
        Self {
            rule_id,
            repository_id,
            path_prefix,
            primary_slot_id,
            generation,
        }
    }
}
#[cfg(test)]
#[path = "../../../test/server/unit/backend_protection.rs"]
mod tests;

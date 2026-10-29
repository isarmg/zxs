//! Read-only retained workspace evidence budgets, not a retirement API.
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
pub struct CapacityMeasure {
    #[serialization(with = "crate::u64_string")]
    pub used: u64,
    #[serialization(with = "crate::u64_string")]
    pub limit: u64,
    #[serialization(with = "crate::u64_string")]
    pub remaining: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
pub struct WorkspaceCapacityScope {
    /// Resident records exclude admitted cold terminal manifests.
    pub records: CapacityMeasure,
    #[serialization(with = "crate::u64_string")]
    pub cold_records: u64,
    /// Includes both resident and cold manifests and lifecycle reservations.
    pub metadata_bytes: CapacityMeasure,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(rename_all = "snake_case")]
pub enum CapacityWarning {
    Ok,
    NearLimit,
    Exhausted,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
pub struct WorkspaceCapacity {
    pub global: WorkspaceCapacityScope,
    pub principal: WorkspaceCapacityScope,
    pub warning: CapacityWarning,
    /// Normal published completions and empty aborts can leave the resident working set.
    /// Permanent manifests, request evidence and recovery bytes are retained.
    pub automatic_retirement: bool,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/workspace_capacity.rs"]
mod tests;

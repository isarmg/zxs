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
    /// Resident records exclude verified cold terminal workspaces.
    pub records: CapacityMeasure,
    #[serialization(with = "crate::u64_string")]
    pub cold_records: u64,
    /// Includes both resident and cold manifests and lifecycle reservations.
    pub metadata_bytes: CapacityMeasure,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serialization(rename_all = "snake_case")]
pub enum CapacityWarning {
    Ok,
    NearLimit,
    Exhausted,
}
impl<'de> Deserialize<'de> for CapacityWarning {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        // A derived unit enum also accepts {"ok": null}; the response schema
        // admits only string tokens, not externally tagged objects.
        struct WarningVisitor;
        impl data_serialization::de::Visitor<'_> for WarningVisitor {
            type Value = CapacityWarning;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a capacity warning string")
            }

            fn visit_str<E: data_serialization::de::Error>(
                self,
                token: &str,
            ) -> Result<Self::Value, E> {
                match token {
                    "ok" => Ok(CapacityWarning::Ok),
                    "near_limit" => Ok(CapacityWarning::NearLimit),
                    "exhausted" => Ok(CapacityWarning::Exhausted),
                    _ => Err(E::unknown_variant(
                        token,
                        &["ok", "near_limit", "exhausted"],
                    )),
                }
            }
        }
        deserializer.deserialize_str(WarningVisitor)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
pub struct WorkspaceCapacity {
    pub global: WorkspaceCapacityScope,
    pub principal: WorkspaceCapacityScope,
    pub warning: CapacityWarning,
    /// Verified published or empty aborted manifests can leave the resident working set.
    pub automatic_retirement: bool,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/workspace_capacity.rs"]
mod tests;

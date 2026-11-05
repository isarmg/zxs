//! Durable review plans, not foreign-disk admission or import receipts.
use crate::{OperationId, storage_directories::DirectorySource, storage_foreign::*};
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignPlanTarget {
    pub slot_id: String,
    /// Null selects an empty disk root; a path selects an existing empty directory.
    #[serialization(deserialize_with = "required_nullable")]
    pub parent: Option<String>,
    /// Every verified history group must have exactly one distinct child name.
    pub history_names: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignPlanRequest {
    pub operation_id: OperationId,
    pub source: DirectorySource,
    pub source_review_digest: String,
    pub target: ForeignPlanTarget,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignMappedHistory {
    pub group_id: String,
    pub history_tree_id: String,
    pub destination: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignTargetMapping {
    pub identity: ForeignDiskIdentity,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    pub source_digest: String,
    pub mapping_digest: String,
    pub histories: Vec<ForeignMappedHistory>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ForeignPlanBlocker {
    PortableImportWriterUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignPlanPreview {
    pub request: ForeignPlanRequest,
    pub source: ForeignDiskPreview,
    pub target: ForeignTargetMapping,
    pub review_digest: String,
    pub execution: ForeignExecutionCapability,
    pub blocker: ForeignPlanBlocker,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignPlanSave {
    pub preview: ForeignPlanPreview,
    /// Saving authorizes only a system-disk review record, never a data mutation.
    pub acknowledge_plan_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignPlanSelection {
    pub operation_id: OperationId,
    pub review_digest: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ForeignPlanValidation {
    /// A stored review is not a reservation or evidence of current availability.
    Required,
    /// Exact source observation and target mapping matched during this request.
    MatchedAtObservation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignPlanStatus {
    pub preview: ForeignPlanPreview,
    pub validation: ForeignPlanValidation,
}

fn required_nullable<'de, T: Deserialize<'de>, D: data_serialization::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}

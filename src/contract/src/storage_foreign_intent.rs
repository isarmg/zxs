//! Saving a target-local execution intent is not importing or reserving anything.
use crate::{
    OperationId,
    storage_foreign::{ForeignDiskIdentity, ForeignExecutionCapability},
    storage_foreign_plan::{ForeignPlanPreview, ForeignPlanSelection},
};
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignIntentRequest {
    /// Scoped by the complete target registration identity, not a global reservation.
    pub operation_id: OperationId,
    pub plan: ForeignPlanSelection,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ForeignIntentEffect {
    SaveExecutionIntent,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignIntentPreview {
    pub request: ForeignIntentRequest,
    pub plan: ForeignPlanPreview,
    pub principal: String,
    pub intent_digest: String,
    pub effect: ForeignIntentEffect,
    pub execution: ForeignExecutionCapability,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignIntentSave {
    pub preview: ForeignIntentPreview,
    /// Explicitly approves only immutable intent bytes on the selected data disk.
    pub confirm_save_intent: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignIntentQuery {
    pub operation_id: OperationId,
    pub target: ForeignDiskIdentity,
    pub intent_digest: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ForeignIntentStage {
    IntentSaved,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignIntentStatus {
    pub address: ForeignIntentQuery,
    pub plan: ForeignPlanSelection,
    pub stage: ForeignIntentStage,
    pub histories: usize,
    pub temporarily_unavailable_versions: usize,
    pub execution: ForeignExecutionCapability,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/storage_foreign_intent.rs"]
mod tests;

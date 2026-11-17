//! The existing degraded outcome overlay, shared by its writer and the
//! original/visible receipt admission. The plan has already passed its full
//! candidate-bound recovery proof; this function neither verifies nor repairs it.
use crate::{OperationResult, RecoveryPlan};
use json_codec::json;
use storage_contract::{CommitState, CurrentHealth, ProtectionAtPublish};

pub(crate) fn apply(mut result: OperationResult, plan: &RecoveryPlan) -> OperationResult {
    result.commit_state = CommitState::PublishedDegraded;
    result.protection_at_publish = ProtectionAtPublish::BelowTarget;
    result.current_health = if plan.version_protection.iter().any(|v| {
        !v["slots"]
            .as_array()
            .expect("slots")
            .iter()
            .any(|s| s["state"] == "COMPLETE_VERIFIED")
    }) {
        CurrentHealth::Unavailable
    } else {
        CurrentHealth::Degraded
    };
    result.result["recovery_id"] = json!(plan.recovery_id);
    result.result["recovery_plan_digest"] = json!(plan.digest);
    result.result["isolation_generation"] = json!(plan.isolation_generation);
    result.result["outcome"] = json!("COMMITTED_DEGRADED");
    result.result["version_protection"] = json!(plan.version_protection);
    result
}

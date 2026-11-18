//! Test values follow the native purge producer's canonical fields and digest domains.
use json_codec::{Value, json};
use storage_contract::{Plan, PlanState, object_hash, object_hash_value};

pub(crate) const PLAN: &str = "plan_00000000000000000000000000000001";
pub(crate) const HISTORY: &str = "history_00000000000000000000000000000001";
pub(crate) const VERSION: &str = "ver_00000000000000000000000000000001";

pub(crate) fn plan() -> Value {
    let input = json!({"history_tree_id": HISTORY, "version_ids": [VERSION]});
    json!({
        "plan_id": PLAN, "kind": "purge", "principal": "admin", "expected_sequence": "4",
        "digest": object_hash_value("plan", &input).unwrap(), "input": input, "state": "ready",
    })
}

pub(crate) fn receipt(commit: bool, operation: &str) -> Value {
    let mut reviewed: Plan = json_codec::from_value(plan()).unwrap();
    let (kind, body, result) = if commit {
        (
            "content.purge",
            json!({"plan_id": PLAN, "digest": reviewed.digest}),
            json!({"plan_id": PLAN, "history_tree_id": HISTORY, "purged_versions": [VERSION]}),
        )
    } else {
        reviewed.state = PlanState::Cancelled;
        (
            "plan.cancel",
            json!({"plan_id": PLAN}),
            json!({"plan_id": PLAN, "state": "cancelled",
                "cancelled_plan_hash": object_hash("plan-cancellation", &reviewed).unwrap()}),
        )
    };
    let input = json!({"kind": kind, "principal": "admin", "body": body});
    json!({
        "operation_id": operation, "kind": kind, "commit_id": null, "history_tree_id": null,
        "branch_id": null, "commit_state": "PUBLISHED", "protection_at_publish": "NOT_APPLICABLE",
        "current_health": "UNKNOWN", "request_digest": object_hash_value("request", &input).unwrap(),
        "sequence": "5", "result": result,
    })
}

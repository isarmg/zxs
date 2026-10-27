//! Plan lookup and cancellation keep the persisted input opaque.
use data_serialization::Serialize;

/// HTTP cancellation fields after key admission, not an all-success domain command.
pub struct CancelPlan {
    pub plan_id: String,
    /// Operation errors follow plan ownership and history admission.
    pub operation_id: Result<storage_contract::OperationId, crate::ApiError>,
}

/// Commit fields with a deferred operation format result; no confirmation field exists on wire.
pub struct CommitPlan {
    pub plan_id: String,
    pub plan_digest: String,
    /// Propagated after plan ownership, expected digest and history admission.
    pub operation_id: Result<storage_contract::OperationId, crate::ApiError>,
}

#[derive(Serialize)]
pub(crate) struct PlanResponse {
    plan_id: String,
    kind: String,
    principal: String,
    #[serialization(with = "storage_contract::u64_string")]
    expected_sequence: u64,
    digest: String,
    input: json_codec::Value,
    state: storage_contract::PlanState,
}
impl From<storage_contract::Plan> for PlanResponse {
    fn from(plan: storage_contract::Plan) -> Self {
        let storage_contract::Plan {
            plan_id,
            kind,
            principal,
            expected_sequence,
            digest,
            input,
            state,
        } = plan;
        Self {
            plan_id,
            kind,
            principal,
            expected_sequence,
            digest,
            input,
            state,
        }
    }
}

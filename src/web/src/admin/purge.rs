//! A review can authorize only the named, already-prepared purge of exact FILE versions.
use super::response::ResponseError;
use data_serialization::Deserialize;
use json_codec::{Value, json};
use storage_contract::{HistoryTreeId, Plan, PlanState, Principal, VersionId};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
struct PurgeSelection {
    history_tree_id: HistoryTreeId,
    version_ids: Vec<VersionId>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ValidatedPlan {
    plan: Plan,
    selection: PurgeSelection,
    cancelled_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PurgeCompletion {
    kind: &'static str,
    result: Value,
}

impl PurgeCompletion {
    pub(super) fn matches(&self, kind: &str, result: &Value) -> bool {
        kind == self.kind && result == &self.result
    }
}

impl ValidatedPlan {
    pub(super) fn read(id: &str, plan: Plan) -> Result<Self, ResponseError> {
        if plan.kind != "purge" || plan.plan_id != id || storage_contract::validate_id(id, "plan").is_err()
            || plan.principal.is_empty()
        {
            return Err(ResponseError::Contract("purge plan identity"));
        }
        let selection = PurgeSelection::deserialize(&plan.input).map_err(ResponseError::Decode)?;
        // The native producer canonicalizes its explicit VersionIds before calculating the digest.
        if selection.version_ids.is_empty() || !selection.version_ids.windows(2).all(|v| v[0] < v[1])
            || !storage_contract::object_hash_value("plan", &plan.input).is_ok_and(|h| h == plan.digest)
        {
            return Err(ResponseError::Contract("purge plan versions and digest"));
        }
        let mut cancelled = plan.clone();
        cancelled.state = PlanState::Cancelled;
        let cancelled_hash = storage_contract::object_hash("plan-cancellation", &cancelled)
            .map_err(|_| ResponseError::Contract("cancelled purge plan digest"))?;
        Ok(Self { plan, selection, cancelled_hash })
    }

    pub(super) fn plan(&self) -> &Plan {
        &self.plan
    }

    pub(super) fn ready(&self) -> bool {
        self.plan.state == PlanState::Ready
    }

    pub(super) fn belongs_to(&self, principal: Option<&Principal>) -> bool {
        principal.is_some_and(|p| p.as_str() == self.plan.principal)
    }

    pub(super) fn completion(&self, commit: bool) -> PurgeCompletion {
        if commit {
            PurgeCompletion {
                kind: "content.purge",
                result: json!({
                    "plan_id": self.plan.plan_id,
                    "history_tree_id": self.selection.history_tree_id,
                    "purged_versions": self.selection.version_ids,
                }),
            }
        } else {
            PurgeCompletion {
                kind: "plan.cancel",
                result: json!({
                    "plan_id": self.plan.plan_id, "state": "cancelled",
                    "cancelled_plan_hash": self.cancelled_hash,
                }),
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/admin/purge_fixtures.rs"]
pub(super) mod fixtures;

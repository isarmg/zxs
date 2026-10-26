//! Validate the endpoint's actual success contract before releasing write fences.
//! Domain-returning endpoints are not operation receipts.
use crate::api;
use json_codec::Value;
use storage_contract::{Branch, MergeLink, OperationResult, Plan, Repository};

pub(super) struct Contract {
    method: &'static str,
    path: String,
    operation: String,
    principal: String,
    // Only non-secret identity fields needed to correlate domain responses.
    expected: Value,
}
impl Contract {
    pub(super) fn new(
        method: &'static str,
        path: &str,
        body: Option<&Value>,
        operation: &str,
        principal: &str,
    ) -> Self {
        let mut expected = json_codec::json!({});
        if let Some(body) = body {
            for key in [
                "name",
                "primary_slot_id",
                "target_copies",
                "source_commit_id",
                "source_branch_id",
                "target_branch_id",
                "target_commit_id",
            ] {
                if let Some(value) = body.get(key) {
                    expected[key] = value.clone();
                }
            }
        }
        Self {
            method,
            path: path.into(),
            operation: operation.into(),
            principal: principal.into(),
            expected,
        }
    }
    fn same(&self, value: &Value, keys: &[&str]) -> bool {
        keys.iter().all(|key| {
            self.expected
                .get(*key)
                .is_some_and(|expected| value.get(*key) == Some(expected))
        })
    }
    pub(super) fn validate(&self, value: Value) -> Result<Value, api::Error> {
        let path = self.path.as_str();
        let valid =
            if path.starts_with("/api/operations/") && self.method == "GET" {
                operation_shape(&value)
                    && path
                        == format!(
                            "/api/operations/{}",
                            api::segment(
                                value["operation_id"]
                                    .as_str()
                                    .expect("invariant: operation receipt identity was validated")
                            )
                        )
            } else if path.starts_with("/api/plans/") && self.method == "GET" {
                json_codec::from_value::<Plan>(value.clone()).is_ok()
                    && value["input"].is_object()
                    && nonempty(&value["digest"])
                    && path
                        == format!(
                            "/api/plans/{}",
                            api::segment(
                                value["plan_id"]
                                    .as_str()
                                    .expect("invariant: decoded plan identity is a string")
                            )
                        )
            } else if self.method == "GET" {
                true // Other inspect-only responses never release a mutation fence.
            } else if path == "/api/repositories" {
                json_codec::from_value::<Repository>(value.clone()).is_ok()
                    && nonempty(&value["repository_id"])
                    && nonempty(&value["history_tree_id"])
                    && nonempty(&value["main_branch_id"])
                    && self.same(&value, &["name", "primary_slot_id", "target_copies"])
            } else if path.ends_with("/branches") {
                branch_creation_shape(&value)
                    && nonempty(&value["branch_id"])
                    && nonempty(&value["history_tree_id"])
                    && self.same(&value, &["name"])
                    && value["fork_commit_id"] == self.expected["source_commit_id"]
            } else if path.ends_with("/merge-links") {
                json_codec::from_value::<MergeLink>(value.clone()).is_ok()
                    && nonempty(&value["merge_link_id"])
                    && value["operation_id"].as_str() == Some(self.operation.as_str())
                    && self.same(
                        &value,
                        &[
                            "source_branch_id",
                            "source_commit_id",
                            "target_branch_id",
                            "target_commit_id",
                        ],
                    )
                    && path
                        == format!(
                            "/api/history-trees/{}/merge-links",
                            api::segment(value["history_tree_id"].as_str().expect(
                                "invariant: decoded merge-link history identity is a string"
                            ))
                        )
            } else if let Some(kind) = [
                ("/purge-plans", "purge"),
                ("/split-plans", "split"),
                ("/historical-save-plans", "historical_save"),
            ]
            .into_iter()
            .find_map(|(suffix, kind)| path.ends_with(suffix).then_some(kind))
            {
                json_codec::from_value::<Plan>(value.clone()).is_ok()
                    && nonempty(&value["plan_id"])
                    && nonempty(&value["digest"])
                    && value["kind"] == kind
                    && value["state"] == "ready"
                    && value["principal"] == self.principal
                    && value["input"]["history_tree_id"]
                        .as_str()
                        .is_some_and(|id| {
                            path.starts_with(&format!("/api/history-trees/{}/", api::segment(id)))
                        })
            } else if path.ends_with("/commit") || path.ends_with("/cancel") {
                json_codec::from_value::<OperationResult>(value.clone()).is_ok()
                    && nonempty(&value["kind"])
                    && nonempty(&value["request_digest"])
                    && confirmed(&value, &self.operation)
            } else if path.ends_with("/smb-binding")
                || path.starts_with("/api/policies/")
                || path.starts_with("/api/primary-placement-rules/")
            {
                operation_shape(&value) && confirmed(&value, &self.operation)
            } else {
                false
            };
        if valid {
            Ok(value)
        } else {
            Err(api::Error { code: storage_contract::ErrorCode::InvalidAdminResponse, message: "The response did not confirm the requested action. Inspect the original operation before retrying.".into(), retryable: false, status: 200, request_id: None, source: None })
        }
    }
}
fn nonempty(value: &Value) -> bool {
    value.as_str().is_some_and(|s| !s.is_empty())
}
fn branch_creation_shape(value: &Value) -> bool {
    // CreateBranch returns branch_wire, not the persisted Branch. Validate and
    // remove only its public content projection before decoding the strict
    // domain record; unrelated fields must still be rejected.
    let mut domain = value.clone();
    let Some(fields) = domain.as_object_mut() else {
        return false;
    };
    let materialized = fields.remove("materialized");
    let inherited = fields.remove("inherited");
    let content_state = fields.remove("content_state");
    let Ok(branch) = json_codec::from_value::<Branch>(domain) else {
        return false;
    };
    let pending = !branch.pending_file_versions.is_empty();
    materialized.as_ref().and_then(Value::as_bool) == Some(!pending)
        && inherited.as_ref().and_then(Value::as_bool) == Some(pending)
        && content_state.as_ref().and_then(Value::as_str)
            == Some(if pending { "PENDING" } else { "LIVE" })
}
fn operation_shape(value: &Value) -> bool {
    storage_contract::client_protocol::operation_status(value).is_some()
}
fn confirmed(value: &Value, operation: &str) -> bool {
    !operation.is_empty()
        && value["operation_id"].as_str() == Some(operation)
        && matches!(
            value["commit_state"].as_str(),
            Some("PUBLISHED" | "ACKED" | "PUBLISHED_DEGRADED")
        )
}
pub(super) fn terminal(value: &Value, operation: &str) -> bool {
    use storage_contract::client_protocol::{OperationStatus, operation_status};
    !operation.is_empty()
        && value["operation_id"].as_str() == Some(operation)
        && matches!(
            operation_status(value),
            Some(
                OperationStatus::Published | OperationStatus::Degraded | OperationStatus::Rejected
            )
        )
}

#[cfg(test)]
#[path = "../../../../test/web/unit/admin/response_tests.rs"]
mod tests;

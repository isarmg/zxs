//! Validate the endpoint's actual success contract before releasing write fences.
//! Domain-returning endpoints are not operation receipts.
use json_codec::Value;
use storage_contract::{
    Branch, MergeLink, OperationResult, Plan, Repository, client_protocol::ResponseError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreviewKind {
    Purge,
    Split,
    HistoricalSave,
}
impl PreviewKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Purge => "purge",
            Self::Split => "split",
            Self::HistoricalSave => "historical_save",
        }
    }
}

/// Captured at request construction; never inferred from a URL or JSON body.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Contract {
    Inspect,
    Operation {
        id: String,
    },
    Plan {
        id: String,
    },
    Repository {
        operation: String,
        name: String,
        primary_slot_id: String,
        target_copies: u32,
    },
    Branch {
        operation: String,
        name: String,
        source_commit_id: String,
    },
    MergeLink {
        operation: String,
        history_tree_id: String,
        source_branch_id: String,
        source_commit_id: String,
        target_branch_id: String,
        target_commit_id: String,
    },
    Preview {
        kind: PreviewKind,
        history_tree_id: String,
        principal: String,
    },
    PlanReceipt {
        operation: String,
    },
    MutationReceipt {
        operation: String,
    },
}
impl Contract {
    pub(super) fn is_write(&self) -> bool {
        match self {
            Self::Inspect | Self::Operation { .. } | Self::Plan { .. } => false,
            Self::Repository { .. }
            | Self::Branch { .. }
            | Self::MergeLink { .. }
            | Self::Preview { .. }
            | Self::PlanReceipt { .. }
            | Self::MutationReceipt { .. } => true,
        }
    }

    pub(super) fn mutation_operation(&self) -> Option<&str> {
        match self {
            Self::Inspect | Self::Operation { .. } | Self::Plan { .. } | Self::Preview { .. } => {
                None
            }
            Self::Repository { operation, .. }
            | Self::Branch { operation, .. }
            | Self::MergeLink { operation, .. }
            | Self::PlanReceipt { operation }
            | Self::MutationReceipt { operation } => Some(operation),
        }
    }

    pub(super) fn validate(&self, value: Value) -> Result<Value, ResponseError> {
        let valid = match self {
            Self::Inspect => true,
            Self::Operation { id } => {
                operation_shape(&value) && value["operation_id"].as_str() == Some(id.as_str())
            }
            Self::Plan { id } => {
                json_codec::from_value::<Plan>(value.clone()).is_ok()
                    && value["input"].is_object()
                    && nonempty(&value["digest"])
                    && value["plan_id"].as_str() == Some(id.as_str())
            }
            Self::Repository {
                name,
                primary_slot_id,
                target_copies,
                ..
            } => {
                json_codec::from_value::<Repository>(value.clone()).is_ok()
                    && nonempty(&value["repository_id"])
                    && nonempty(&value["history_tree_id"])
                    && nonempty(&value["main_branch_id"])
                    && value["name"].as_str() == Some(name.as_str())
                    && value["primary_slot_id"].as_str() == Some(primary_slot_id.as_str())
                    && value["target_copies"].as_u64() == Some(u64::from(*target_copies))
            }
            Self::Branch {
                name,
                source_commit_id,
                ..
            } => {
                branch_creation_shape(&value)
                    && nonempty(&value["branch_id"])
                    && nonempty(&value["history_tree_id"])
                    && value["name"].as_str() == Some(name.as_str())
                    && value["fork_commit_id"].as_str() == Some(source_commit_id.as_str())
            }
            Self::MergeLink {
                operation,
                history_tree_id,
                source_branch_id,
                source_commit_id,
                target_branch_id,
                target_commit_id,
            } => {
                json_codec::from_value::<MergeLink>(value.clone()).is_ok()
                    && nonempty(&value["merge_link_id"])
                    && value["operation_id"].as_str() == Some(operation.as_str())
                    && value["history_tree_id"].as_str() == Some(history_tree_id.as_str())
                    && value["source_branch_id"].as_str() == Some(source_branch_id.as_str())
                    && value["source_commit_id"].as_str() == Some(source_commit_id.as_str())
                    && value["target_branch_id"].as_str() == Some(target_branch_id.as_str())
                    && value["target_commit_id"].as_str() == Some(target_commit_id.as_str())
            }
            Self::Preview {
                kind,
                history_tree_id,
                principal,
            } => {
                json_codec::from_value::<Plan>(value.clone()).is_ok()
                    && nonempty(&value["plan_id"])
                    && nonempty(&value["digest"])
                    && value["kind"] == kind.as_str()
                    && value["state"] == "ready"
                    && value["principal"].as_str() == Some(principal.as_str())
                    && value["input"]["history_tree_id"].as_str() == Some(history_tree_id.as_str())
            }
            Self::PlanReceipt { operation } => {
                json_codec::from_value::<OperationResult>(value.clone()).is_ok()
                    && nonempty(&value["kind"])
                    && nonempty(&value["request_digest"])
                    && confirmed(&value, operation)
            }
            Self::MutationReceipt { operation } => {
                operation_shape(&value) && confirmed(&value, operation)
            }
        };
        if valid {
            Ok(value)
        } else {
            Err(ResponseError::new(
                200,
                storage_contract::ErrorCode::InvalidAdminResponse,
                "The response did not confirm the requested action. Inspect the original operation before retrying.",
            ))
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

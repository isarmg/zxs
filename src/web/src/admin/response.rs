//! Validate the endpoint's actual success contract before releasing write fences.
//! Domain-returning endpoints are not operation receipts.
use json_codec::Value;
use storage_contract::{OperationResult, Plan, Repository, client_protocol::OperationStatus};

#[derive(Debug, error_definition::Error)]
pub(crate) enum ResponseError {
    #[error("invalid administration response: {0}")]
    Decode(#[source] json_codec::Error),
    #[error("administration response does not match {0}")]
    Contract(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ValidatedPlan(Plan);
impl ValidatedPlan {
    pub(super) fn plan(&self) -> &Plan {
        &self.0
    }
}

#[derive(Debug)]
pub(super) struct ValidatedAdminResponse {
    pub(super) display: String,
    pub(super) action: Action,
}
#[derive(Debug)]
pub(super) enum Action {
    Inspect,
    Plan(ValidatedPlan),
    Outcome {
        operation: String,
        status: OperationStatus,
    },
    Mutation {
        operation: String,
    },
    PlanReceipt(OperationResult),
}
fn decode<T: data_serialization::de::DeserializeOwned>(value: Value) -> Result<T, ResponseError> {
    json_codec::from_value(value).map_err(ResponseError::Decode)
}
fn require(valid: bool, context: &'static str) -> Result<(), ResponseError> {
    if valid {
        Ok(())
    } else {
        Err(ResponseError::Contract(context))
    }
}

/// Captured at request construction; never inferred from a URL or JSON body.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Contract {
    Inspect,
    Operation {
        id: String,
    },
    OperationOutcome {
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
            Self::Inspect
            | Self::Operation { .. }
            | Self::OperationOutcome { .. }
            | Self::Plan { .. } => false,
            Self::Repository { .. } | Self::PlanReceipt { .. } | Self::MutationReceipt { .. } => {
                true
            }
        }
    }

    pub(super) fn mutation_operation(&self) -> Option<&str> {
        match self {
            Self::Inspect
            | Self::Operation { .. }
            | Self::OperationOutcome { .. }
            | Self::Plan { .. } => None,
            Self::Repository { operation, .. }
            | Self::PlanReceipt { operation }
            | Self::MutationReceipt { operation } => Some(operation),
        }
    }

    pub(super) fn validate(&self, value: Value) -> Result<ValidatedAdminResponse, ResponseError> {
        let display =
            json_codec::to_string_pretty(&value).expect("invariant: JSON values serialize");
        let action = match self {
            Self::Inspect => Action::Inspect,
            Self::Operation { id } => {
                storage_contract::client_protocol::operation_observation_status(&value)
                    .ok_or(ResponseError::Contract("operation observation shape"))?;
                require(
                    value["operation_id"].as_str() == Some(id),
                    "operation identity",
                )?;
                Action::Inspect
            }
            Self::OperationOutcome { id } => {
                let status = storage_contract::client_protocol::operation_status(&value)
                    .ok_or(ResponseError::Contract("operation outcome shape"))?;
                require(
                    value["operation_id"].as_str() == Some(id),
                    "operation identity",
                )?;
                Action::Outcome {
                    operation: id.clone(),
                    status,
                }
            }
            Self::Plan { id } => {
                let plan: Plan = decode(value)?;
                require(
                    plan.input.is_object() && !plan.digest.is_empty() && plan.plan_id == *id,
                    "plan identity, input and digest",
                )?;
                Action::Plan(ValidatedPlan(plan))
            }
            Self::Repository {
                operation,
                name,
                primary_slot_id,
                target_copies,
            } => {
                let repository: Repository = decode(value)?;
                require(
                    !repository.repository_id.is_empty()
                        && !repository.history_tree_id.is_empty()
                        && !repository.main_branch_id.is_empty()
                        && repository.name == *name
                        && repository.primary_slot_id == *primary_slot_id
                        && repository.target_copies == *target_copies,
                    "repository identity and requested configuration",
                )?;
                Action::Mutation {
                    operation: operation.clone(),
                }
            }
            Self::PlanReceipt { operation } => {
                let receipt: OperationResult = decode(value)?;
                require(
                    !receipt.kind.is_empty()
                        && !receipt.request_digest.is_empty()
                        && !operation.is_empty()
                        && receipt.operation_id.to_string() == *operation
                        && matches!(
                            receipt.commit_state,
                            storage_contract::CommitState::Published
                                | storage_contract::CommitState::Acked
                                | storage_contract::CommitState::PublishedDegraded
                        ),
                    "plan operation identity and publication",
                )?;
                Action::PlanReceipt(receipt)
            }
            Self::MutationReceipt { operation } => {
                let status = storage_contract::client_protocol::operation_status(&value)
                    .ok_or(ResponseError::Contract("mutation receipt shape"))?;
                require(
                    !operation.is_empty()
                        && value["operation_id"].as_str() == Some(operation)
                        && matches!(
                            status,
                            OperationStatus::Published | OperationStatus::Degraded
                        ),
                    "mutation identity and publication",
                )?;
                Action::Mutation {
                    operation: operation.clone(),
                }
            }
        };
        Ok(ValidatedAdminResponse { display, action })
    }
}
#[cfg(test)]
#[path = "../../../../test/web/unit/admin/response_tests.rs"]
mod tests;

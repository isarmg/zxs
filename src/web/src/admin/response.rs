//! Validate the endpoint's actual success contract before releasing write fences.
//! Domain-returning endpoints are not operation receipts.
use json_codec::Value;
use storage_contract::{OperationResult, client_protocol::OperationStatus};
use super::purge::PurgeCompletion;
pub(super) use super::purge::ValidatedPlan;

#[derive(Debug, error_definition::Error)]
pub(crate) enum ResponseError {
    #[error("invalid administration response: {0}")]
    Decode(#[source] json_codec::Error),
    #[error("administration response does not match {0}")]
    Contract(&'static str),
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

/// Captured at request construction.
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
    PlanReceipt {
        operation: String,
        expected: PurgeCompletion,
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
            Self::PlanReceipt { .. } | Self::MutationReceipt { .. } => {
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
            Self::PlanReceipt { operation, .. }
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
                Action::Plan(ValidatedPlan::read(id, decode(value)?)?)
            }
            Self::PlanReceipt { operation, expected } => {
                let receipt: OperationResult = decode(value)?;
                require(
                    expected.matches(&receipt.kind, &receipt.result)
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

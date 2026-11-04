//! Request-local DTO admission before the offline lifecycle can change.
use crate::{
    ApiError,
    setup_recovery::{
        RecoveryCommitRequest, RecoveryPreviewRequest, RestoreRequest, StorageActivationRequest,
    },
};
use json_codec::Value;

// Request-local lifecycle inputs; public account setup has its own one-time lock.
pub(super) enum SetupCommand {
    Activate(StorageActivationRequest),
    CompleteActivation,
    RestoreSystem(RestoreRequest),
    RecoveryPreview(RecoveryPreviewRequest),
    RecoveryCommit(RecoveryCommitRequest),
    OfflineCheck(String),
}

impl SetupCommand {
    pub(super) fn parse(action: &str, input: Value) -> Result<Self, ApiError> {
        Ok(match action {
            "activate" => {
                Self::Activate(json_codec::from_value(input).map_err(ApiError::invalid_source)?)
            }
            "complete_activation" => {
                if !input.as_object().is_some_and(|object| object.is_empty()) {
                    return Err(ApiError::invalid(
                        "activation completion accepts an empty object",
                    ));
                }
                Self::CompleteActivation
            }
            "restore_system" => Self::RestoreSystem(
                json_codec::from_value(input).map_err(ApiError::invalid_source)?,
            ),
            "recovery_preview" => Self::RecoveryPreview(RecoveryPreviewRequest::parse(input)?),
            "recovery_commit" => Self::RecoveryCommit(
                json_codec::from_value(input).map_err(ApiError::invalid_source)?,
            ),
            "offline_check" => Self::OfflineCheck(
                input
                    .as_object()
                    .filter(|object| object.len() == 1)
                    .and_then(|object| object.get("disk_id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| ApiError::invalid("offline check accepts only disk_id"))?
                    .to_owned(),
            ),
            _ => return Err(ApiError::unsupported()),
        })
    }

    pub(super) fn resumes_storage(&self) -> bool {
        match self {
            Self::Activate(_)
            | Self::CompleteActivation
            | Self::RestoreSystem(_)
            | Self::RecoveryCommit(_) => true,
            Self::RecoveryPreview(_) | Self::OfflineCheck(_) => false,
        }
    }
}

//! Request-local DTO admission before the offline lifecycle can change.
use crate::{
    ApiError,
    setup_recovery::{
        CompleteSetupRequest, PresetSetupRequest, RecoveryCommitRequest, RecoveryPreviewRequest,
        RestoreRequest, SetupRequest,
    },
};
use json_codec::Value;

// No Debug or Serialize: several variants own administrator credentials.
pub(super) enum SetupCommand {
    Initialize(SetupRequest),
    InitializePreset(PresetSetupRequest),
    CompleteInitialization(CompleteSetupRequest),
    RestoreSystem(RestoreRequest),
    RecoveryPreview(RecoveryPreviewRequest),
    RecoveryCommit(RecoveryCommitRequest),
    OfflineCheck(String),
}

impl SetupCommand {
    pub(super) fn parse(action: &str, input: Value) -> Result<Self, ApiError> {
        Ok(match action {
            "initialize" => {
                Self::Initialize(json_codec::from_value(input).map_err(ApiError::invalid_source)?)
            }
            "initialize_preset" => Self::InitializePreset(
                json_codec::from_value(input).map_err(ApiError::invalid_source)?,
            ),
            "complete_initialization" => Self::CompleteInitialization(
                json_codec::from_value(input).map_err(ApiError::invalid_source)?,
            ),
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
            Self::Initialize(_)
            | Self::InitializePreset(_)
            | Self::CompleteInitialization(_)
            | Self::RestoreSystem(_)
            | Self::RecoveryCommit(_) => true,
            Self::RecoveryPreview(_) | Self::OfflineCheck(_) => false,
        }
    }
}

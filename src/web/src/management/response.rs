use super::Storage;
use crate::api;
use data_serialization::{Deserialize, de::IgnoredAny};
use json_codec::Value;
use web_interface::i18n::t;

#[derive(Debug, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Status {
    #[serialization(rename = "management")]
    _management: Alive,
    pub(super) authentication: Authentication,
    pub(super) storage: StorageStatus,
    #[serialization(rename = "auxiliary")]
    _auxiliary: IgnoredAny,
}

#[derive(Debug, Deserialize)]
#[serialization(rename_all = "snake_case")]
enum Alive {
    Alive,
}

#[derive(Debug, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub(super) enum Authentication {
    Ready,
    Busy,
    RecoveryRequired,
}

#[derive(Debug, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub(super) enum EmbeddedReady {
    Ready,
}

#[derive(Debug, Deserialize)]
#[serialization(untagged, deny_unknown_fields)]
pub(super) enum StorageStatus {
    Managed {
        state: Storage,
        retryable: bool,
        recovering: bool,
    },
    Embedded {
        state: EmbeddedReady,
        retryable: bool,
    },
}
impl StorageStatus {
    pub(super) fn apply(self, model: &mut super::Model) {
        let (state, retryable, recovering) = match self {
            Self::Managed {
                state,
                retryable,
                recovering,
            } => (state, retryable, recovering),
            Self::Embedded {
                state: EmbeddedReady::Ready,
                retryable,
            } => (Storage::Ready, retryable, false),
        };
        model.storage = state;
        model.retryable = retryable;
        model.recovering = recovering;
    }
}

fn invalid(source: json_codec::Error) -> api::Error {
    api::Error {
        status: 200,
        code: storage_contract::ErrorCode::InvalidResponse,
        message: t(
            "管理状态响应无效，请重新检查。",
            "The management status response is invalid. Check again.",
        ),
        retryable: false,
        request_id: None,
        source: Some(std::sync::Arc::new(api::ErrorSource::ManagementStatus(
            source,
        ))),
    }
}

pub(super) fn status(value: Value) -> Result<Status, api::Error> {
    json_codec::from_value(value).map_err(invalid)
}

pub(super) fn storage(value: Value) -> Result<StorageStatus, api::Error> {
    json_codec::from_value(value).map_err(invalid)
}

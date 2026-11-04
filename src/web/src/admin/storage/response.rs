//! Current management-only activation responses, shared with server regression tests.
use data_serialization::Deserialize;
use json_codec::Value;

#[derive(Debug, error_definition::Error)]
pub enum ResponseError {
    #[error("invalid storage management response: {0}")]
    Decode(#[source] json_codec::Error),
    #[error("storage activation response is not confirmed")]
    Unconfirmed,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Activated {
    activated: bool,
    restart_required: bool,
    recovering: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Success {
    Recovering,
    RestartRequired,
}

pub(super) fn activation(value: Value) -> Result<Success, ResponseError> {
    let Activated {
        activated,
        restart_required,
        recovering,
    } = json_codec::from_value(value).map_err(ResponseError::Decode)?;
    if !activated {
        return Err(ResponseError::Unconfirmed);
    }
    match (restart_required, recovering) {
        (false, true) => Ok(Success::Recovering),
        (true, false) => Ok(Success::RestartRequired),
        _ => Err(ResponseError::Unconfirmed),
    }
}

use data_serialization::Deserialize;
use json_codec::Value;

#[derive(Debug, error_definition::Error)]
pub(super) enum ResponseError {
    #[error("invalid setup response: {0}")]
    Decode(#[source] json_codec::Error),
    #[error("setup response does not confirm management-only initialization")]
    Unconfirmed,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Initialized {
    initialized: bool,
    storage_pending: bool,
}

pub(super) fn validate(value: Value) -> Result<(), ResponseError> {
    let Initialized {
        initialized,
        storage_pending,
    } = json_codec::from_value(value).map_err(ResponseError::Decode)?;
    if initialized && storage_pending {
        Ok(())
    } else {
        Err(ResponseError::Unconfirmed)
    }
}

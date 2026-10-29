//! Fresh diagnostic observations are separate from permanent publication outcomes.
use crate::ApiError;

#[derive(Debug)]
pub struct OperationObservation {
    pub(crate) outcome: storage_contract::OperationOutcome,
    pub(crate) health: Option<CheckedOperationHealth>,
}

#[derive(Debug, data_serialization::Serialize)]
pub(crate) struct CheckedOperationHealth {
    state: storage_contract::CurrentHealth,
    checked_at: String,
}

impl CheckedOperationHealth {
    pub(crate) fn completed(state: storage_contract::CurrentHealth) -> Result<Self, ApiError> {
        Ok(Self { state, checked_at: checked_at(std::time::SystemTime::now())? })
    }
}

fn checked_at(time: std::time::SystemTime) -> Result<String, ApiError> {
    let invalid = || {
        ApiError::new(
            ::http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::IoError,
            "system clock cannot be represented as a UTC timestamp",
        )
    };
    let duration = time
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|source| invalid().with_source(source))?;
    // Duration's u64 seconds and subsecond field always fit i128 nanoseconds.
    let nanos =
        i128::from(duration.as_secs()) * 1_000_000_000 + i128::from(duration.subsec_nanos());
    let time = date_time::OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .map_err(|source| invalid().with_source(source))?;
    Ok(time
        .format(&date_time::format_description::well_known::Rfc3339)
        .expect("invariant: nonnegative UTC time is within the RFC3339 year range"))
}


#[cfg(test)]
#[path = "../../../test/server/unit/operation_wire_tests.rs"]
mod tests;

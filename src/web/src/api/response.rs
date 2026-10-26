//! Browser adapter for the shared client protocol.
use super::Error;
use json_codec::Value;

pub(super) fn decode(status: u16, bytes: &[u8]) -> Result<Value, Error> {
    storage_contract::client_protocol::decode_response(status, bytes).map_err(from_protocol)
}

pub(super) fn from_protocol(error: storage_contract::client_protocol::ResponseError) -> Error {
    Error {
        status: error.status,
        code: error.code,
        message: error.message,
        retryable: error.retryable,
        request_id: error.request_id,
        source: error
            .source
            .map(|source| std::sync::Arc::new(super::ErrorSource::Decode(source))),
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/api/response/tests.rs"]
mod tests;

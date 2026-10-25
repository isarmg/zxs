//! Native HTTP streaming adapter for the shared client protocol.
use super::Error;
use futures::StreamExt;
use json_codec::Value;
use storage_contract::client_protocol::{self, MAX_RESPONSE_BYTES, ResponseError};

fn protocol_error(error: ResponseError) -> Error {
    Error::Api {
        status: error.status,
        code: error.code,
        message: match error.request_id {
            Some(request_id) => format!("{} (request {request_id})", error.message),
            None => error.message,
        },
        retryable: error.retryable,
        source: error.source,
    }
}

pub(super) async fn decode(response: reqwest::Response) -> Result<Value, Error> {
    let status = response.status().as_u16();
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
            return Err(protocol_error(client_protocol::response_too_large(status)));
        }
        bytes.extend_from_slice(&chunk);
    }
    client_protocol::decode_response(status, &bytes).map_err(protocol_error)
}

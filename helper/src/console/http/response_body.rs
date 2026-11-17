//! Response bytes retain the same admission slot as their blocking producer.
use data_serialization::Serialize;
use http_routing::{
    body::{Body, Bytes},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use tokio::sync::OwnedSemaphorePermit;

struct OwnedBytes {
    bytes: Bytes,
    _permit: OwnedSemaphorePermit,
}
impl AsRef<[u8]> for OwnedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

pub(super) fn owned(bytes: Bytes, permit: OwnedSemaphorePermit) -> Body {
    // Bytes clones and queued HTTP/1 frames retain the admission slot until
    // drained or disconnected, even after the Body wrapper is dropped. An
    // abandoned blocking result drops its bytes and permit together.
    Body::from(Bytes::from_owner(OwnedBytes {
        bytes,
        _permit: permit,
    }))
}

pub(super) fn json<T: Serialize>(value: T, permit: OwnedSemaphorePermit) -> Response {
    // Preserve Json's bytes, content type and serialization-failure response,
    // while carrying the job permit into the transport's queued Bytes frames.
    let (status, content_type, bytes) = match json_codec::to_vec(&value) {
        Ok(bytes) => (StatusCode::OK, "application/json", Bytes::from(bytes)),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "text/plain; charset=utf-8",
            Bytes::from(error.to_string()),
        ),
    };
    (
        status,
        [(header::CONTENT_TYPE, content_type)],
        owned(bytes, permit),
    )
        .into_response()
}

#[cfg(test)]
#[path = "../../../../test/helper/unit/console/response_body_tests.rs"]
mod tests;

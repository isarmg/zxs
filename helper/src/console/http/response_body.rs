//! A bulk response owns the same admission slot as its blocking producer.
use http_routing::body::{Body, Bytes};
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

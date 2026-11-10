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
    Body::from(Bytes::from_owner(OwnedBytes {
        bytes,
        _permit: permit,
    }))
}

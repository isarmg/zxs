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
    // Bind the permit to the actual bytes, not only the Body wrapper: HTTP
    // may drop that wrapper after polling its final frame while the frame is
    // still queued for a slow client. Bytes clones/slices retain this owner.
    // The console's plain Tokio TCP transport uses the vectored HTTP/1 queue,
    // so queued body buffers keep their owner until drained or disconnected.
    // If a request disappears during spawn_blocking, its producer still owns
    // the permit; the abandoned result drops both bytes and permit together.
    Body::from(Bytes::from_owner(OwnedBytes { bytes, _permit: permit }))
}

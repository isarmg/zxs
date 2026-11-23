//! The lowered workers emit an exact public 64-byte original-object stamp.
pub(super) fn object_body(stamp: [u8; 64]) -> [u8; 65] {
    let mut frame = [0; 65];
    frame[0] = 1;
    frame[1..].copy_from_slice(&stamp);
    frame
}

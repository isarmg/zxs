//! Only valid requests decoded in memory. No client, signing or transport.
use super::*;
use crate::smb::wire::{direct::Frame, message::Compound};

fn fields(flags: u32, count: u16, length: u64) -> Result<LockFields, LockFieldError> {
    let size = 88 + usize::from(count) * 24;
    let mut body = vec![0; size];
    body[..8].copy_from_slice(b"\xfeSMB\x40\0\x01\0");
    body[12..14].copy_from_slice(&LOCK.to_le_bytes());
    body[36] = 7;
    body[64] = 48;
    body[66..68].copy_from_slice(&count.to_le_bytes());
    body[68..72].copy_from_slice(&0x405u32.to_le_bytes()); // valid index 64
    body[72] = 3;
    body[80] = 4;
    for element in body[88..].as_chunks_mut::<24>().0 {
        element[..8].copy_from_slice(&23u64.to_le_bytes());
        element[8..16].copy_from_slice(&length.to_le_bytes());
        element[16..20].copy_from_slice(&flags.to_le_bytes());
    }
    let prefix = u32::try_from(size).unwrap().to_be_bytes();
    let data = [prefix.as_slice(), body.as_slice()].concat();
    let (frame, remainder) = Frame::split(&data).unwrap().unwrap();
    assert_eq!(remainder, []);
    Compound::parse(frame).unwrap().messages()[0].lock_fields()
}

#[test]
fn standalone_lock_fields_preserve_two_open_halves_and_exact_nonblocking_range() {
    for (flags, change) in [
        (0x11, LockChange::Shared),
        (0x12, LockChange::Exclusive),
        (0x04, LockChange::Unlock),
    ] {
        let fields = fields(flags, 1, 14).unwrap();
        assert_eq!(fields.open(), WireOpenId { persistent: 3, volatile: 4 });
        assert_eq!(fields.span(), Span::new(23, 14).unwrap());
        assert_eq!(fields.change(), change);
    }
}

#[test]
fn valid_other_lock_profiles_fail_before_a_lock_request_exists() {
    assert_eq!(fields(0x02, 1, 14), Err(LockFieldError::NotSupported));
    assert_eq!(fields(0x11, 2, 14), Err(LockFieldError::NotSupported));
    assert_eq!(fields(0x12, 1, 0), Err(LockFieldError::NotSupported));
}

#[test]
fn interval_uses_the_last_byte_without_wrap_or_adjacent_conflicts() {
    let last_byte = Span::new(u64::MAX, 1).unwrap();
    let previous_byte = Span::new(u64::MAX - 1, 1).unwrap();
    assert!(!last_byte.overlaps(previous_byte));
    assert!(last_byte.overlaps(Span::new(u64::MAX - 1, 2).unwrap()));
    assert!(Span::new(u64::MAX, 2).is_none());
    assert!(Span::new(21, 0).is_none());
}

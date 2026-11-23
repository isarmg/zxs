//! Synthetic command bodies decoded in memory. No client, signing or transport.
use super::*;
use crate::smb::wire::{direct::Frame, message::Compound};

pub(in crate::smb) fn packet(rows: &[(u64, u64, u32)]) -> Vec<u8> {
    let count = u16::try_from(rows.len()).unwrap();
    let size = 88 + rows.len() * ELEMENT;
    let mut body = vec![0; size];
    body[..8].copy_from_slice(b"\xfeSMB\x40\0\x01\0");
    body[12..14].copy_from_slice(&LOCK.to_le_bytes());
    body[36] = 7;
    body[64] = 48;
    body[66..68].copy_from_slice(&count.to_le_bytes());
    body[68..72].copy_from_slice(&0x405u32.to_le_bytes()); // valid index 64
    body[72] = 3;
    body[80] = 4;
    for (&(offset, length, flags), element) in rows.iter().zip(body[88..].as_chunks_mut::<ELEMENT>().0) {
        element[..8].copy_from_slice(&offset.to_le_bytes());
        element[8..16].copy_from_slice(&length.to_le_bytes());
        element[16..20].copy_from_slice(&flags.to_le_bytes());
        element[20..24].fill(0xa5); // ignored per MS-SMB2
    }
    let prefix = u32::try_from(size).unwrap().to_be_bytes();
    [prefix.as_slice(), body.as_slice()].concat()
}

pub(in crate::smb) fn parse(data: &[u8]) -> Result<LockFields<'_>, LockFieldError> {
    let (frame, remainder) = Frame::split(data).unwrap().unwrap();
    assert_eq!(remainder, []);
    Compound::parse(frame).unwrap().messages()[0].lock_fields()
}

#[test]
fn standalone_lock_fields_preserve_two_open_halves_and_exact_nonblocking_range() {
    for (flags, change) in [
        (0x11, LockChange::Shared),
        (0x12, LockChange::Exclusive),
        (0x04, LockChange::Unlock),
        (0x14, LockChange::Unlock),
    ] {
        let data = packet(&[(23, 14, flags)]);
        let fields = parse(&data).unwrap();
        assert_eq!(fields.open(), WireOpenId { persistent: 3, volatile: 4 });
        assert_eq!(fields.ordered().collect::<Vec<_>>(), [Ok((Span::new(23, 14).unwrap(), change))]);
    }
}

#[test]
fn valid_other_lock_profiles_fail_before_a_lock_request_exists() {
    assert_eq!(parse(&packet(&[(23, 14, 0x02)])), Err(LockFieldError::NotSupported));
    assert_eq!(parse(&packet(&[(23, 14, 0x12), (40, 4, 0x01)])), Err(LockFieldError::InvalidParameter));
    assert_eq!(parse(&packet(&[(23, 0, 0x12)])), Err(LockFieldError::NotSupported));
}

#[test]
fn multiple_entries_keep_original_order_and_exact_count() {
    let rows = [(50, 7, 0x11), (23, 14, 0x12)];
    let mut data = packet(&rows);
    let fields = parse(&data).unwrap();
    assert_eq!(fields.open(), WireOpenId { persistent: 3, volatile: 4 });
    assert_eq!(fields.ordered().collect::<Vec<_>>(), [
        Ok((Span::new(50, 7).unwrap(), LockChange::Shared)),
        Ok((Span::new(23, 14).unwrap(), LockChange::Exclusive)),
    ]);
    for declared in [1u16, 3] {
        data[70..72].copy_from_slice(&declared.to_le_bytes());
        assert_eq!(parse(&data), Err(LockFieldError::InvalidParameter));
    }
    assert_eq!(parse(&packet(&[])), Err(LockFieldError::InvalidParameter));
    let data = packet(&[(23, 14, 0x11); 17]);
    assert_eq!(parse(&data).unwrap().ordered().count(), 17);
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

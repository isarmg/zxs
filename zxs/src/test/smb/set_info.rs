//! Structural examples only; an original paid VerifiedMember is required
//! by the production-facing gate. These bytes do not represent real files.
use super::*;
use crate::smb::wire::{
    credit_fields::Cost, direct::Frame, io_fields::WireOpenId, message::{Compound, RELATED},
};

fn request(class: u8, offset: usize, input: &[u8]) -> Vec<u8> {
    assert!(offset >= FIXED_END);
    let mut p = vec![0; offset];
    p[..4].copy_from_slice(b"\xfeSMB");
    p[4] = 64;
    p[6] = 1;
    p[12..14].copy_from_slice(&SET_INFO.to_le_bytes());
    p[64] = 33;
    p[66] = INFO_FILE;
    p[67] = class;
    p[68..72].copy_from_slice(&(input.len() as u32).to_le_bytes());
    p[72..74].copy_from_slice(&(offset as u16).to_le_bytes());
    p[74..76].fill(0xcc); // reserved bytes have no meaning
    p[80..88].copy_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
    p[88..96].copy_from_slice(&0x8877_6655_4433_2211u64.to_le_bytes());
    p.extend_from_slice(input);
    p
}
fn packet(raw: &[u8]) -> Vec<u8> {
    let mut n = (raw.len() as u32).to_be_bytes().to_vec();
    n.extend_from_slice(raw);
    n
}
fn parsed(packet: &[u8]) -> Compound<'_> {
    let (frame, tail) = Frame::split(packet).unwrap().unwrap();
    assert!(tail.is_empty());
    Compound::parse(frame).unwrap()
}
fn one(raw: &[u8], max: u32) -> Result<Fields, Error> {
    let p = packet(raw);
    fields(&parsed(&p).messages()[0], max)
}

#[test]
fn disposition_is_a_boolean_on_the_original_open_not_extended_flags() {
    for (n, want) in [(0, Change::ClearDisposition), (1, Change::MarkForDelete), (0x80, Change::MarkForDelete)] {
        let raw = request(FILE_DISPOSITION, 96, &[n]);
        assert_eq!(one(&raw, 65_536).unwrap(), Fields {
            change: want, input_length: 1,
            open: OpenReference::Explicit(WireOpenId {
                persistent: 0x1122_3344_5566_7788, volatile: 0x8877_6655_4433_2211,
            }),
        });
    }
    for class in [4, 10, 11, 19, 64, 65] {
        assert_eq!(one(&request(class, 96, &[1, 0, 0, 0]), 65_536), Err(Error::NotInProfile));
    }
    let mut raw = request(FILE_DISPOSITION, 96, &[1]);
    raw[66] = 3; // INFO_SECURITY never falls into a FILE mutator
    assert_eq!(one(&raw, 65_536), Err(Error::NotInProfile));
    raw[66] = INFO_FILE;
    raw[76] = 4;
    assert_eq!(one(&raw, 65_536), Err(Error::Malformed));
}

#[test]
fn eof_is_absolute_and_cannot_use_the_tail_or_a_negative_write_sentinel() {
    for n in [0u64, 4096, i64::MAX as u64] {
        let raw = request(FILE_END_OF_FILE, 104, &n.to_le_bytes());
        assert_eq!(one(&raw, 65_536).unwrap().change, Change::EndOfFile(n));
    }
    for n in [i64::MIN, -1] {
        let raw = request(FILE_END_OF_FILE, 96, &n.to_le_bytes());
        assert_eq!(one(&raw, 65_536), Err(Error::NegativeEof));
    }
    let mut short = request(FILE_END_OF_FILE, 96, &[0; 7]);
    short.push(0); // bytes after declared buffer are not EOF bytes
    assert_eq!(one(&short, 65_536), Err(Error::InformationLength));
    let mut wide = request(FILE_DISPOSITION, 96, &[1, 0xcc, 0xcc]);
    assert_eq!(one(&wide, 2), Err(Error::ExceedsMaxTransact));
    let bytes = packet(&wide);
    let req = parsed(&bytes);
    assert!(matches!(req.messages()[0].credit_cost(), Cost::Bytes(3)));
    assert_eq!(fields(&req.messages()[0], 3).unwrap().input_length, 3);
    wide[68..72].fill(0);
    assert_eq!(one(&wide, 65_536), Err(Error::Malformed)); // no declared byte
}

#[test]
fn member_bounds_and_open_route_are_independent_of_a_later_command() {
    let mut first = request(FILE_DISPOSITION, 96, &[1]);
    first.resize(104, 0);
    first[72..74].copy_from_slice(&104u16.to_le_bytes()); // payload begins at the next member
    first[20..24].copy_from_slice(&104u32.to_le_bytes());
    let mut related = request(FILE_DISPOSITION, 96, &[0]);
    related[16..20].copy_from_slice(&RELATED.to_le_bytes());
    first.extend_from_slice(&related);
    let b = packet(&first);
    let c = parsed(&b);
    assert_eq!(fields(&c.messages()[0], 65_536), Err(Error::Malformed));
    // Literal non-sentinel halves still cannot replace the original result.
    assert_eq!(fields(&c.messages()[1], 65_536).unwrap().open, OpenReference::PreviousInCompound);

    let mut raw = request(FILE_DISPOSITION, 96, &[1]);
    raw[80..96].fill(0xff);
    assert_eq!(one(&raw, 65_536), Err(Error::NoExplicitOpen));
    for flags in [0x1000_0000u32, 0x2000_0000] {
        raw[16..20].copy_from_slice(&flags.to_le_bytes());
        assert_eq!(one(&raw, 65_536), Err(Error::NotInProfile));
    }
    raw[16..20].fill(0);
    raw[80..96].fill(1);
    raw[72..74].copy_from_slice(&95u16.to_le_bytes()); // no overlap with FILEID
    assert_eq!(one(&raw, 65_536), Err(Error::Malformed));
}

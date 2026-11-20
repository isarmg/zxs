use super::*;
use crate::{
    FileId, PoolId,
    smb::{
        backend::{Facts, Grant, Object, OpenCoordinator, OriginalRequest, ParentWrite, ReserveError, Share},
        wire::{
            direct::Frame,
            io_fields::{IoFieldError, OpenReference, WireOpenId},
            message::Compound,
        },
    },
};
use std::{cell::Cell, convert::Infallible, marker::PhantomData, num::NonZeroUsize};

const MAX_READ: u32 = 131_072;
const PERSISTENT: u64 = 0x0706_0504_0302_0100;
const VOLATILE: u64 = 0x1716_1514_1312_1110;

fn base(command: u16, structure: u16, fixed: usize, id_at: usize) -> Vec<u8> {
    let mut req = vec![0; fixed];
    req[..4].copy_from_slice(b"\xfeSMB");
    req[4..6].copy_from_slice(&64u16.to_le_bytes());
    req[12..14].copy_from_slice(&command.to_le_bytes());
    req[64..66].copy_from_slice(&structure.to_le_bytes());
    req[id_at..id_at + 8].copy_from_slice(&PERSISTENT.to_le_bytes());
    req[id_at + 8..id_at + 16].copy_from_slice(&VOLATILE.to_le_bytes());
    req
}

fn read_packet(offset: u64, length: u32, minimum: u32, flags: u8) -> Vec<u8> {
    let mut req = base(8, 49, 112, 80);
    req[67] = flags;
    req[68..72].copy_from_slice(&length.to_le_bytes());
    req[72..80].copy_from_slice(&offset.to_le_bytes());
    req[96..100].copy_from_slice(&minimum.to_le_bytes());
    req
}

fn close_packet(flags: u16) -> Vec<u8> {
    let mut req = base(6, 24, 88, 72);
    req[66..68].copy_from_slice(&flags.to_le_bytes());
    req
}

fn framed(parts: &[&[u8]]) -> Vec<u8> {
    let mut frame = (parts.iter().map(|x| x.len()).sum::<usize>() as u32).to_be_bytes().to_vec();
    for part in parts { frame.extend_from_slice(part); }
    frame
}

fn compound(frame: &[u8]) -> Compound<'_> {
    let (record, rest) = Frame::split(frame).unwrap().unwrap();
    assert!(rest.is_empty());
    Compound::parse(record).unwrap()
}

fn range(offset: u64, length: u32, minimum: u32, flags: u8) -> ReadRange {
    let frame = framed(&[&read_packet(offset, length, minimum, flags)]);
    compound(&frame).messages()[0].read_request(MAX_READ).unwrap().range
}

#[test]
fn read_fields_keep_real_credit_span_and_ignore_unused_tcp_descriptor() {
    let mut req = read_packet(17, 65_537, 7, 3);
    req[66] = 0xff; // a non-binding response padding preference
    req[104..112].fill(0xff); // ignored RemainingBytes/offset/length with CHANNEL_NONE
    let frame = framed(&[&req]);
    let batch = compound(&frame);
    let read = batch.messages()[0].read_request(MAX_READ).unwrap();
    assert_eq!(read.open, OpenReference::Explicit(WireOpenId {
        persistent: PERSISTENT, volatile: VOLATILE,
    }));
    assert_eq!(read.range.offset(), 17);
    assert_eq!(read.range.length(), 65_537);
    assert_eq!(read.range.minimum(), 7);
    assert!(read.range.unbuffered() && read.compression_requested);
    assert_eq!(batch.messages()[0].read_request(65_536), Err(IoFieldError::ExceedsMaxRead));
    assert_eq!(batch.messages()[0].close_request(), Err(IoFieldError::WrongCommand));

    req[100] = 1; // SMB Direct channel cannot be honored over TCP
    let frame = framed(&[&req]);
    assert_eq!(compound(&frame).messages()[0].read_request(MAX_READ), Err(IoFieldError::Malformed));
    req[100] = 0;
    req[67] = 0x04;
    let frame = framed(&[&req]);
    assert_eq!(compound(&frame).messages()[0].read_request(MAX_READ), Err(IoFieldError::Malformed));
}

#[test]
fn read_rejects_signed_offset_domain_overflow_without_wrapping() {
    for (offset, length) in [(u64::MAX, 0), (u64::MAX, 1), (i64::MAX as u64, 1)] {
        let frame = framed(&[&read_packet(offset, length, 0, 0)]);
        assert_eq!(compound(&frame).messages()[0].read_request(MAX_READ), Err(IoFieldError::InvalidRange));
    }
    assert_eq!(range(i64::MAX as u64 - 4, 4, 4, 0).length(), 4);
    assert_eq!(range(i64::MAX as u64, 0, 0, 0).offset(), i64::MAX as u64);
    assert_eq!(range(17, 4, 5, 0).minimum(), 5); // assess after verified READ, not during parse
}

#[test]
fn related_close_has_no_explicit_fallback_and_neither_request_reads_its_neighbor() {
    let mut read = read_packet(0, 4, 0, 0);
    read[20..24].copy_from_slice(&112u32.to_le_bytes());
    let mut close = close_packet(1);
    close[16..20].copy_from_slice(&4u32.to_le_bytes()); // related, body IDs are not authority
    close[68..72].fill(0x91); // reserved word has no meaning
    let frame = framed(&[&read, &close]);
    let batch = compound(&frame);
    assert_eq!(batch.messages()[0].read_request(MAX_READ).unwrap().open,
        OpenReference::Explicit(WireOpenId { persistent: PERSISTENT, volatile: VOLATILE }));
    let parsed = batch.messages()[1].close_request().unwrap();
    assert_eq!(parsed.open, OpenReference::PreviousInCompound);
    assert_eq!(parsed.mode, CloseMode::PostQueryAttributes);

    close[72..88].fill(0xff);
    let frame = framed(&[&read, &close]);
    assert_eq!(compound(&frame).messages()[1].close_request().unwrap(), parsed);
    close[16..20].fill(0);
    let frame = framed(&[&close]);
    assert_eq!(compound(&frame).messages()[0].close_request(), Err(IoFieldError::NoExplicitOpen));
    close[66..68].copy_from_slice(&2u16.to_le_bytes());
    let frame = framed(&[&close]);
    assert_eq!(compound(&frame).messages()[0].close_request(), Err(IoFieldError::Malformed));

    read.truncate(104);
    read[20..24].copy_from_slice(&104u32.to_le_bytes());
    let frame = framed(&[&read, &close_packet(0)]);
    assert_eq!(compound(&frame).messages()[0].read_request(MAX_READ), Err(IoFieldError::Malformed));
    let mut close = close_packet(0);
    close.truncate(80);
    close[20..24].copy_from_slice(&80u32.to_le_bytes());
    let frame = framed(&[&close, &read_packet(0, 1, 0, 0)]);
    assert_eq!(compound(&frame).messages()[0].close_request(), Err(IoFieldError::Malformed));
}

// The remainder are synthetic, in-process storage receipts, never real
// authentication/SMB traffic, backing files, or an alternate adapter.
struct Bind<H>(Facts, PhantomData<H>);
impl<H> OriginalRequest for Bind<H> {
    type Held = H;
    type Error = Infallible;
    fn recheck(&self, _held: &H) -> Result<Facts, Self::Error> { Ok(self.0) }
}

fn original<'s, H>(owner: &'s OpenCoordinator, facts: Facts, h: H, share: u32) -> Held<'s, H> {
    let op = Bind::<H>(facts, PhantomData);
    owner.reserve(&op, facts, Share::from_bits(share).unwrap()).unwrap().bind(h).unwrap()
}

fn facts(object: Object, rights: u32) -> Facts {
    Facts { object, grant: Grant::from_effective(rights).unwrap(), parent_write: ParentWrite::Granted }
}

const POOL: PoolId = PoolId([1; 16]);
const FILE: Object = Object::File(POOL, FileId([2; 16]));

struct Reader<'a> {
    called: &'a Cell<Option<(u64, u32)>>,
    storage: Result<&'static [u8], &'static str>,
}
impl CheckedRead for Reader<'_> {
    type Body = &'static [u8];
    type Error = &'static str;
    fn read_checked(&self, offset: u64, length: u32) -> Result<Self::Body, Self::Error> {
        self.called.set(Some((offset, length)));
        self.storage
    }
}

#[test]
fn read_requires_an_open_data_grant_not_list_execute_share_or_zero_length() {
    let table = OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let called = Cell::new(None);
    let body = b"";
    for (obj, access, request, room, fault) in [
        (Object::Namespace(POOL), 1, range(17, 0, 0, 0), 0, ReadFault::NotDataFile),
        (FILE, 0x20, range(17, 0, 0, 0), 0, ReadFault::AccessDenied),
        (FILE, 0x80, range(17, 4, 0, 0), 4, ReadFault::AccessDenied),
        (FILE, 1, range(17, 4, 0, 1), 4, ReadFault::UnbufferedUnavailable),
        (FILE, 1, range(17, 4, 0, 0), 3, ReadFault::ResponseSpace),
    ] {
        let h = original(&table, facts(obj, access), Reader { called: &called, storage: Ok(body) }, 7);
        assert_eq!(h.read(request, room), Err(fault));
        assert_eq!(called.get(), None);
    }
}

#[test]
fn checked_short_data_zero_length_and_real_eof_remain_distinct() {
    let table = OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let called = Cell::new(None);
    let grant = facts(FILE, 1);
    for (request, storage, result) in [
        (range(17, 4, 2, 0), Ok(&b"ab"[..]), Ok(&b"ab"[..])),
        (range(17, 4, 3, 0), Ok(&b"ab"[..]), Err(ReadFault::EndOfFile)),
        (range(17, 4, 5, 0), Ok(&b"abcd"[..]), Err(ReadFault::EndOfFile)),
        (range(17, 4, 0, 0), Ok(&b""[..]), Err(ReadFault::EndOfFile)),
        (range(17, 0, 0, 0), Ok(&b""[..]), Ok(&b""[..])),
        (range(17, 0, 0, 0), Err("revoked"), Err(ReadFault::Original("revoked"))),
        (range(17, 4, 0, 0), Err("medium offline"), Err(ReadFault::Original("medium offline"))),
        (range(17, 4, 0, 0), Ok(&b"abcde"[..]), Err(ReadFault::InconsistentLength)),
    ] {
        called.set(None);
        let h = original(&table, grant, Reader { called: &called, storage }, 7);
        assert_eq!(h.read(request, 4), result);
        assert_eq!(called.get(), Some((request.offset(), request.length())));
    }
}

struct Closer<'a> {
    owner: &'a OpenCoordinator,
    called: &'a Cell<Option<CloseMode>>,
    failure: bool,
}
impl CheckedClose for Closer<'_> {
    type Error = &'static str;
    fn close_checked(&mut self, mode: CloseMode) -> Result<Option<CloseAttributes>, Self::Error> {
        self.called.set(Some(mode));
        let rhs = facts(FILE, 2);
        let op = Bind::<()>(rhs, PhantomData);
        assert_eq!(self.owner.reserve(&op, rhs, Share::from_bits(7).unwrap()).err(),
            Some(ReserveError::SharingViolation)); // original still owns its vote
        if self.failure { return Err("unconfirmed close"); }
        Ok(Some(CloseAttributes { created: 1, accessed: 2, written: 3, changed: 4,
            allocation_size: 8, end_of_file: 5, file_attributes: 0x20 }))
    }
}

#[test]
fn explicit_close_consumes_the_hold_while_drop_never_auto_closes_or_flushes() {
    let table = OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let called = Cell::new(None);
    let open = |failure| original(&table, facts(FILE, 1),
        Closer { owner: &table, called: &called, failure }, 1);
    drop(open(false));
    assert_eq!(called.get(), None); // unexpected disconnect is not a normal close

    assert!(open(false).close(CloseMode::Plain).unwrap().is_none());
    assert_eq!(called.get(), Some(CloseMode::Plain));
    assert_eq!(open(false).close(CloseMode::PostQueryAttributes).unwrap().unwrap().end_of_file, 5);
    assert_eq!(called.get(), Some(CloseMode::PostQueryAttributes));
    assert_eq!(open(true).close(CloseMode::Plain).err(), Some("unconfirmed close"));
    drop(open(false)); // error released the old vote, without publishing success
}

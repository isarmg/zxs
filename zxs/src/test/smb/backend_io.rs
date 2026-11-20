use super::*;
use crate::{
    FileId, PoolId,
    smb::{
        backend::{Facts, Grant, Object, OpenCoordinator, OriginalRequest, ParentWrite, ReserveError, Share},
        wire::{
            direct::Frame,
            io_fields::{IoFieldError, OpenReference, WireOpenId},
            io_reply::FileReply,
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
        assert_eq!(h.read(request, room).err(), Some(fault));
        assert_eq!(called.get(), None);
    }
}

#[test]
fn checked_short_data_zero_length_and_real_eof_follow_wire_status() {
    let table = OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let called = Cell::new(None);
    let grant = facts(FILE, 1);
    for (request, storage, result) in [
        (range(17, 4, 2, 0), Ok(&b"ab"[..]), Ok(&b"ab"[..])),
        (range(17, 4, 3, 0), Ok(&b"ab"[..]), Err(ReadFault::EndOfFile)),
        (range(17, 4, 5, 0), Ok(&b"abcd"[..]), Err(ReadFault::EndOfFile)),
        (range(17, 4, 0, 0), Ok(&b""[..]), Err(ReadFault::EndOfFile)),
        (range(17, 0, 0, 0), Ok(&b""[..]), Err(ReadFault::EndOfFile)),
        (range(17, 0, 0, 0), Err("revoked"), Err(ReadFault::Original("revoked"))),
        (range(17, 4, 0, 0), Err("medium offline"), Err(ReadFault::Original("medium offline"))),
        (range(17, 4, 0, 0), Ok(&b"abcde"[..]), Err(ReadFault::InconsistentLength)),
    ] {
        called.set(None);
        let h = original(&table, grant, Reader { called: &called, storage }, 7);
        let checked = h.read(request, 4);
        match result {
            Ok(data) => assert_eq!(checked.unwrap().data(), data),
            Err(fault) => assert_eq!(checked.err(), Some(fault)),
        }
        assert_eq!(called.get(), Some((request.offset(), request.length())));
    }
}

struct Closer<'a> {
    owner: &'a OpenCoordinator,
    called: &'a Cell<Option<CloseMode>>,
    failure: bool,
    attributes_available: bool,
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
        if !self.attributes_available { return Ok(None); }
        Ok(Some(CloseAttributes { created: 1, accessed: 2, written: 3, changed: 4,
            allocation_size: 8, end_of_file: 5, file_attributes: 0x20 }))
    }
}

#[test]
fn explicit_close_consumes_the_hold_while_drop_never_auto_closes_or_flushes() {
    let table = OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let called = Cell::new(None);
    let open = |failure| original(&table, facts(FILE, 1),
        Closer { owner: &table, called: &called, failure, attributes_available: true }, 1);
    drop(open(false));
    assert_eq!(called.get(), None); // unexpected disconnect is not a normal close

    assert!(open(false).close(CloseMode::Plain).unwrap().attributes().is_none());
    assert_eq!(called.get(), Some(CloseMode::Plain));
    assert_eq!(open(false).close(CloseMode::PostQueryAttributes).unwrap().attributes().unwrap().end_of_file, 5);
    assert_eq!(called.get(), Some(CloseMode::PostQueryAttributes));
    assert_eq!(open(true).close(CloseMode::Plain).err(), Some("unconfirmed close"));
    drop(open(false)); // error released the old vote, without publishing success
}

fn write_packet(at: usize, offset: u64, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut req = base(9, 49, 112, 80);
    req[66..68].copy_from_slice(&(at as u16).to_le_bytes());
    req[68..72].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    req[72..80].copy_from_slice(&offset.to_le_bytes());
    req[108..112].copy_from_slice(&flags.to_le_bytes());
    if at >= 112 {
        req.resize(at, 0);
    }
    req.extend_from_slice(payload);
    req
}

fn flush_packet() -> Vec<u8> { base(7, 24, 88, 72) }

fn data(frame: &[u8]) -> WriteData<'_> {
    compound(frame).messages()[0].write_request(MAX_READ).unwrap().data
}

#[test]
fn write_borrows_exact_tcp_data_and_flags_not_framing_or_padding() {
    let mut req = write_packet(120, 19, 0x8000_0003, b"abc");
    req[100..108].fill(0xff); // ignored channel data under CHANNEL_NONE
    req.extend_from_slice(b"pad");
    let frame = framed(&[&req]);
    let batch = compound(&frame);
    let command = batch.messages()[0].write_request(MAX_READ).unwrap();
    assert_eq!(command.open, OpenReference::Explicit(WireOpenId {
        persistent: PERSISTENT, volatile: VOLATILE,
    }));
    assert_eq!(command.data.position(), WritePosition::At(19));
    assert_eq!(command.data.payload(), b"abc");
    assert_eq!(command.data.length(), 3);
    assert!(command.data.write_through() && command.data.unbuffered());
    assert_eq!(batch.messages()[0].write_request(2).err(), Some(IoFieldError::ExceedsMaxWrite));
    req[96] = 1;
    let frame = framed(&[&req]);
    assert_eq!(compound(&frame).messages()[0].write_request(MAX_READ).err(), Some(IoFieldError::Malformed));
}

#[test]
fn write_checks_offsets_and_preserves_symbolic_eof_without_implicit_resize() {
    for at in [111, 257] {
        let frame = framed(&[&write_packet(at, 2, 0, b"x")]);
        assert_eq!(compound(&frame).messages()[0].write_request(MAX_READ).err(), Some(IoFieldError::Malformed));
    }
    for offset in [u64::MAX - 1, i64::MAX as u64] {
        let frame = framed(&[&write_packet(112, offset, 0, b"x")]);
        assert_eq!(compound(&frame).messages()[0].write_request(MAX_READ).err(), Some(IoFieldError::InvalidRange));
    }
    let frame = framed(&[&write_packet(256, u64::MAX, 1, b"x")]);
    assert_eq!(data(&frame).position(), WritePosition::EndOfFile);
    for at in [0, 112] {
        let frame = framed(&[&write_packet(at, i64::MAX as u64, 0, b"")]);
        assert_eq!(data(&frame).position(), WritePosition::At(i64::MAX as u64));
        assert!(data(&frame).payload().is_empty());
    }
}

#[test]
fn write_cannot_borrow_from_flush_and_flush_reserved_bits_are_ignored() {
    let mut write = write_packet(112, 0, 0, b"abc");
    write.truncate(112); // no body in this command
    write[20..24].copy_from_slice(&112u32.to_le_bytes());
    let mut flush = flush_packet();
    flush[16..20].copy_from_slice(&4u32.to_le_bytes());
    flush[66..72].fill(0xff);
    flush[72..88].fill(0xff);
    let frame = framed(&[&write, &flush]);
    let batch = compound(&frame);
    assert_eq!(batch.messages()[0].write_request(MAX_READ).err(), Some(IoFieldError::Malformed));
    assert_eq!(batch.messages()[1].flush_request().unwrap().open, OpenReference::PreviousInCompound);
    // Wire extraction of PreviousInCompound is not permission after a
    // failing predecessor; the real signed dispatcher must propagate that error.
    flush[16..20].fill(0);
    flush[72..80].copy_from_slice(&PERSISTENT.to_le_bytes());
    flush[80..88].copy_from_slice(&VOLATILE.to_le_bytes());
    let frame = framed(&[&flush]);
    assert_eq!(compound(&frame).messages()[0].flush_request().unwrap().open,
        OpenReference::Explicit(WireOpenId { persistent: PERSISTENT, volatile: VOLATILE }));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WriteStep {
    CheckEof,
    Write(u64, usize),
    Flush,
}

// Synthetic receipts for source checks only; no model of Core persistence.
struct Writer<'a> {
    seen: &'a std::cell::RefCell<Vec<WriteStep>>,
    eof: u64,
    mode: WriteOpenMode,
    fail_flush: bool,
}
impl CheckedWrite for Writer<'_> {
    type Error = &'static str;
    fn writer_eof_checked(&self) -> Result<u64, Self::Error> {
        self.seen.borrow_mut().push(WriteStep::CheckEof);
        Ok(self.eof)
    }
    fn write_open_mode(&self) -> WriteOpenMode { self.mode }
    fn write_checked(&mut self, offset: u64, input: &[u8]) -> Result<(), Self::Error> {
        self.seen.borrow_mut().push(WriteStep::Write(offset, input.len()));
        if !input.is_empty() {
            self.eof = self.eof.max(offset + input.len() as u64);
        }
        Ok(())
    }
    fn flush_checked(&mut self) -> Result<(), Self::Error> {
        self.seen.borrow_mut().push(WriteStep::Flush);
        if self.fail_flush { return Err("original flush failed"); }
        Ok(())
    }
}

#[test]
fn append_and_overwrite_use_original_staged_eof_and_separate_grants() {
    use WriteStep::*;
    let table = OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let seen = std::cell::RefCell::new(Vec::new());
    let open = |rights, eof| original(&table, facts(FILE, rights),
        Writer { seen: &seen, eof, mode: WriteOpenMode::Buffered, fail_flush: false }, 7);
    let append = framed(&[&write_packet(112, u64::MAX, 0, b"ab")]);
    let mut h = open(4, 19); // append only, exact original EOF, not a snapshot
    assert_eq!(h.write(data(&append)), Ok(WriteCompletion::Applied(2)));
    assert_eq!(h.write(data(&append)), Ok(WriteCompletion::Applied(2)));
    assert_eq!(seen.borrow().as_slice(), &[CheckEof, Write(19, 2), CheckEof, Write(21, 2)]);
    for at in [18, 24] {
        let frame = framed(&[&write_packet(112, at, 0, b"x")]);
        seen.borrow_mut().clear();
        assert_eq!(h.write(data(&frame)), Err(WriteFault::AccessDenied));
        assert_eq!(seen.borrow().as_slice(), &[CheckEof]);
    }
    drop(h);
    let mut h = open(2, 19); // can overwrite, can't extend
    let old = framed(&[&write_packet(112, 17, 0, b"ab")]);
    let overlap = framed(&[&write_packet(112, 18, 0, b"ab")]);
    assert_eq!(h.write(data(&old)), Ok(WriteCompletion::Applied(2)));
    assert_eq!(h.write(data(&overlap)), Err(WriteFault::AccessDenied));
    assert_eq!(h.write(data(&append)), Err(WriteFault::AccessDenied));
    drop(h);
    let mut h = open(6, i64::MAX as u64);
    assert_eq!(h.write(data(&append)), Err(WriteFault::InvalidRange));
}

#[test]
fn reserved_mode_combinations_do_not_write_and_write_through_waits_for_core() {
    use WriteOpenMode::*;
    use WriteStep::*;
    let table = OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let seen = std::cell::RefCell::new(Vec::new());
    let open = |mode, fail_flush| original(&table, facts(FILE, 6),
        Writer { seen: &seen, eof: 4, mode, fail_flush }, 7);
    for (mode, flags, err) in [
        (Buffered, 1, WriteFault::InvalidWriteCombination),
        (BufferedWriteThrough, 1, WriteFault::InvalidWriteCombination),
        (Buffered, 2, WriteFault::UnbufferedUnavailable),
        (Buffered, 3, WriteFault::UnbufferedUnavailable),
        (Unbuffered, 0, WriteFault::UnbufferedUnavailable),
    ] {
        let wire = framed(&[&write_packet(112, 0, flags, b"a")]);
        assert_eq!(open(mode, false).write(data(&wire)), Err(err));
        assert!(seen.borrow().is_empty());
    }

    let wire = framed(&[&write_packet(112, 1, 0, b"ab")]);
    assert_eq!(open(BufferedWriteThrough, false).write(data(&wire)),
        Ok(WriteCompletion::Confirmed(2)));
    assert_eq!(seen.borrow().as_slice(), &[CheckEof, Write(1, 2), Flush]);
    seen.borrow_mut().clear();
    let mut h = open(BufferedWriteThrough, true);
    assert_eq!(h.write(data(&wire)), Err(WriteFault::Confirmation("original flush failed")));
    assert_eq!(seen.borrow().as_slice(), &[CheckEof, Write(1, 2), Flush]);
    // A confirmation failure does not silently drop or close this original.
    assert_eq!(h.flush(), Err(WriteFault::Confirmation("original flush failed")));
}

#[test]
fn empty_write_and_actual_flush_never_imply_history_or_a_new_open() {
    use WriteStep::*;
    let table = OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let seen = std::cell::RefCell::new(Vec::new());
    let open = |object, rights| original(&table, facts(object, rights),
        Writer { seen: &seen, eof: 4, mode: WriteOpenMode::Buffered, fail_flush: false }, 7);
    let wire = framed(&[&write_packet(0, 52, 0, b"")]);
    assert_eq!(open(FILE, 1).write(data(&wire)), Err(WriteFault::AccessDenied));
    assert_eq!(open(FILE, 1).flush(), Err(WriteFault::AccessDenied));
    assert_eq!(open(Object::Namespace(POOL), 2).flush(), Err(WriteFault::DirectoryFlushUnavailable));
    assert!(seen.borrow().is_empty());
    let mut h = open(FILE, 6);
    assert_eq!(h.write(data(&wire)), Ok(WriteCompletion::Applied(0)));
    assert_eq!(h.original.eof, 4);
    assert_eq!(seen.borrow().as_slice(), &[CheckEof, Write(52, 0)]);
    assert_eq!(h.flush(), Ok(()));
    assert_eq!(seen.borrow().as_slice(), &[CheckEof, Write(52, 0), Flush]);
}


#[test]
fn read_reply_uses_actual_short_count_and_borrows_the_original_payload() {
    let table = OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let called = Cell::new(None);
    let prepared = b"Hi";
    let open = original(&table, facts(FILE, 1), Reader { called: &called, storage: Ok(prepared) }, 7);
    let checked = open.read(range(4, 8, 2, 0), 8);
    let reply = FileReply::read(&checked).unwrap();
    assert_eq!((reply.command(), reply.status()), (0x08, 0));
    let (fixed, payload) = reply.parts();
    assert_eq!(fixed, &[17, 0, 80, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(payload, prepared);
    assert!(std::ptr::eq(payload.as_ptr(), prepared.as_ptr()));
    assert_eq!(called.get(), Some((4, 8)));
}

#[test]
fn read_eof_uses_error_body_for_zero_or_insufficient_data_and_not_for_core_failure() {
    let table = OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let called = Cell::new(None);
    for (range_, payload) in [
        (range(9, 4, 0, 0), &b""[..]),  // positive read, no bytes at EOF
        (range(9, 0, 0, 0), &b""[..]),  // zero request must still pass Core
        (range(9, 4, 3, 0), &b"xy"[..]), // bytes below minimum never appear in reply
    ] {
        let open = original(&table, facts(FILE, 1), Reader { called: &called, storage: Ok(payload) }, 7);
        called.set(None);
        let checked = open.read(range_, 4);
        let reply = FileReply::read(&checked).unwrap();
        assert_eq!((reply.command(), reply.status()), (0x08, 0xc000_0011));
        let (fixed, payload) = reply.parts();
        assert_eq!(fixed, &[9, 0, 0, 0, 0, 0, 0, 0]);
        assert!(payload.is_empty());
        assert_eq!(called.get(), Some((range_.offset(), range_.length())));
    }
    let open = original(&table, facts(FILE, 1),
        Reader { called: &called, storage: Err("view revoked") }, 7);
    let result = open.read(range(9, 0, 0, 0), 4);
    assert_eq!(FileReply::read(&result).err(), Some(&"view revoked"));
}

#[test]
fn optional_close_attributes_have_a_real_flag_and_fixed_layout() {
    let table = OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let called = Cell::new(None);
    let open = |attributes_available| original(&table, facts(FILE, 1),
        Closer { owner: &table, called: &called, failure: false, attributes_available }, 1);
    for (mode, available, flagged) in [
        (CloseMode::Plain, true, false),
        (CloseMode::PostQueryAttributes, false, false),
        (CloseMode::PostQueryAttributes, true, true),
    ] {
        let reply = FileReply::close(open(available).close(mode).unwrap());
        assert_eq!((reply.command(), reply.status()), (0x06, 0));
        let (fixed, payload) = reply.parts();
        assert_eq!(fixed.len(), 60);
        assert!(payload.is_empty());
        assert_eq!(&fixed[..8], &[60, 0, u8::from(flagged), 0, 0, 0, 0, 0]);
        if flagged {
            let [created, accessed, written, changed, allocation, eof] = [1u64, 2, 3, 4, 8, 5];
            for (i, value) in [created, accessed, written, changed, allocation, eof].into_iter().enumerate() {
                assert_eq!(&fixed[8 + 8*i..16 + 8*i], &value.to_le_bytes());
            }
            assert_eq!(&fixed[56..60], &[0x20, 0, 0, 0]);
        } else {
            assert_eq!(&fixed[8..], &[0; 52]);
        }
    }
}

use super::{DirectoryPage, PageStop, Pattern, PatternError, QueryObject, RecordSpace};
use crate::{
    DirectoryId, FileId, OperationId, PoolId, VolumeId, display,
    smb::{
        backend::Object,
        wire::{
            direct::Frame,
            message::Compound,
            query_fields::{DirectoryClass, FileClass, FsClass, InfoClass, QueryError, Scan},
        },
    },
};

const MAX_TX: u32 = 131_072;
const RAW_ID: [u8; 16] = [
    0x55, 1, 2, 3, 4, 5, 6, 7, 0xaa, 10, 11, 12, 13, 14, 15, 16,
];

fn fixed_request(command: u16, structure_size: u16) -> Vec<u8> {
    let mut bytes = vec![0; 64 + usize::from(structure_size) - 1];
    bytes[..4].copy_from_slice(b"\xfeSMB");
    bytes[4..6].copy_from_slice(&64u16.to_le_bytes());
    bytes[12..14].copy_from_slice(&command.to_le_bytes());
    bytes[64..66].copy_from_slice(&structure_size.to_le_bytes());
    bytes
}

fn directory(class: u8, flags: u8, output: u32, pattern: &[u8]) -> Vec<u8> {
    let mut req = fixed_request(0x0e, 33);
    req[66] = class;
    req[67] = flags;
    req[72..88].copy_from_slice(&RAW_ID);
    req[92..96].copy_from_slice(&output.to_le_bytes());
    if !pattern.is_empty() {
        req[88..90].copy_from_slice(&96u16.to_le_bytes());
        req[90..92].copy_from_slice(&(pattern.len() as u16).to_le_bytes());
        req.extend_from_slice(pattern);
    }
    req
}

fn info(kind: u8, class: u8, output: u32) -> Vec<u8> {
    let mut req = fixed_request(0x10, 41);
    req[66] = kind;
    req[67] = class;
    req[68..72].copy_from_slice(&output.to_le_bytes());
    req[88..104].copy_from_slice(&RAW_ID);
    req
}

fn utf16(name: &str) -> Vec<u8> {
    name.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn framed(messages: &[&[u8]]) -> Vec<u8> {
    let mut frame = (messages.iter().map(|m| m.len()).sum::<usize>() as u32)
        .to_be_bytes().to_vec();
    for message in messages {
        frame.extend_from_slice(message);
    }
    frame
}

fn parsed(frame: &[u8]) -> Compound<'_> {
    let (frame, rest) = Frame::split(frame).unwrap().unwrap();
    assert!(rest.is_empty());
    Compound::parse(frame).ok().unwrap()
}

#[test]
fn directory_scans_use_the_open_not_a_wire_index() {
    let mut req = directory(37, 0x13, 65_537, &utf16("Aa-é𐐀"));
    req[68..72].copy_from_slice(&u32::MAX.to_le_bytes());
    let data = framed(&[&req]);
    let batch = parsed(&data);
    let query = batch.messages()[0].query_directory(MAX_TX).unwrap();
    assert_eq!(query.class, DirectoryClass::IdBoth);
    assert_eq!(query.scan, Scan::Reopen);
    assert!(query.single);
    assert_eq!(query.wire_id, RAW_ID);
    assert_eq!(query.output_bytes, 65_537);
    assert_eq!(Pattern::from_query(&query), Ok(Some(Pattern::Exact("Aa-é𐐀".into()))));
    assert_eq!(batch.messages()[0].query_directory(65_536), Err(QueryError::ExceedsMaxTransact));

    req[67] = 0x05; // restart plus optional index request; no source offset
    let data = framed(&[&req]);
    assert_eq!(parsed(&data).messages()[0].query_directory(MAX_TX).unwrap().scan, Scan::Restart);
    req[67] = 0x08;
    let data = framed(&[&req]);
    assert_eq!(parsed(&data).messages()[0].query_directory(MAX_TX), Err(QueryError::Malformed));
}

#[test]
fn absent_pattern_accepts_only_the_spec_or_the_native_zero_placeholder() {
    for (offset, tail, valid) in [
        (0u16, &[][..], true),
        (96, &[0, 0][..], true), // Apple native empty pattern
        (96, &[0][..], false),
        (96, &[0, 1][..], false),
        (98, &[0, 0, 0, 0][..], false),
    ] {
        let mut req = directory(38, 0, 512, &[]);
        req[88..90].copy_from_slice(&offset.to_le_bytes());
        req.extend_from_slice(tail);
        let data = framed(&[&req]);
        match parsed(&data).messages()[0].query_directory(MAX_TX) {
            Ok(q) => {
                assert!(valid);
                assert_eq!(q.scan, Scan::Continue);
                assert_eq!(Pattern::from_query(&q), Ok(None));
            }
            Err(e) => {
                assert!(!valid);
                assert_eq!(e, QueryError::Malformed);
            }
        }
    }
}

#[test]
fn a_search_expression_is_not_a_path() {
    for (input, expected) in [
        ("*", Ok(Some(Pattern::All))),
        ("*.*", Ok(Some(Pattern::All))),
        (".", Ok(Some(Pattern::Current))),
        ("..", Ok(Some(Pattern::Parent))),
        ("报告<?.>", Ok(Some(Pattern::Expression("报告<?.>".into())))),
        ("a?.TXT", Ok(Some(Pattern::Expression("a?.TXT".into())))),
        ("普通", Ok(Some(Pattern::Exact("普通".into())))),
        ("..\\*", Err(PatternError::BadName)),
        ("a:b*", Err(PatternError::BadName)),
        ("a/b", Err(PatternError::BadName)),
        ("trim. ", Err(PatternError::BadName)),
        ("a\0*", Err(PatternError::BadName)),
    ] {
        let req = directory(1, 0, 512, &utf16(input));
        let data = framed(&[&req]);
        let q = parsed(&data).messages()[0].query_directory(MAX_TX).unwrap();
        assert_eq!(Pattern::from_query(&q), expected);
    }
    let req = directory(1, 0, 512, &[0x00, 0xd8]);
    let data = framed(&[&req]);
    let q = parsed(&data).messages()[0].query_directory(MAX_TX).unwrap();
    assert_eq!(Pattern::from_query(&q), Err(PatternError::BadName));
    let req = directory(1, 0, 512, &utf16(&"*".repeat(display::MAX + 1)));
    let data = framed(&[&req]);
    let q = parsed(&data).messages()[0].query_directory(MAX_TX).unwrap();
    assert_eq!(Pattern::from_query(&q), Err(PatternError::TooLong));
}

#[test]
fn directory_pattern_cannot_read_its_compound_neighbor_or_fixed_fields() {
    let mut dir = directory(12, 0, 128, &[]);
    dir[20..24].copy_from_slice(&96u32.to_le_bytes());
    dir[88..90].copy_from_slice(&96u16.to_le_bytes());
    dir[90..92].copy_from_slice(&2u16.to_le_bytes());
    let mut inf = info(1, 18, 104);
    inf[16..20].copy_from_slice(&4u32.to_le_bytes()); // related
    inf[88..104].fill(0xff);
    let data = framed(&[&dir, &inf]);
    let batch = parsed(&data);
    assert_eq!(batch.messages()[0].query_directory(MAX_TX), Err(QueryError::Malformed));
    let related = batch.messages()[1].query_info(MAX_TX).unwrap();
    assert_eq!(related.wire_id, [0xff; 16]);
    assert_eq!(related.class, InfoClass::File(FileClass::All));
    assert_eq!(batch.messages()[1].query_directory(MAX_TX), Err(QueryError::WrongCommand));

    dir[20..24].fill(0);
    dir[88..90].copy_from_slice(&88u16.to_le_bytes());
    let data = framed(&[&dir]);
    assert_eq!(parsed(&data).messages()[0].query_directory(MAX_TX), Err(QueryError::Malformed));
    let data = framed(&[&directory(12, 0, 128, &[b'x'])]);
    assert_eq!(parsed(&data).messages()[0].query_directory(MAX_TX), Err(QueryError::Malformed));
}

#[test]
fn metadata_view_keeps_requested_credit_but_does_not_read_ignored_input() {
    let mut req = info(2, 5, 128);
    req[72..76].fill(0xff); // unused input offset and reserved bits
    req[76..80].copy_from_slice(&90_001u32.to_le_bytes());
    req[80..88].fill(0xff); // irrelevant AdditionalInformation and Flags
    let data = framed(&[&req]);
    let msg = parsed(&data);
    let q = msg.messages()[0].query_info(128).unwrap();
    assert_eq!(q.class, InfoClass::Filesystem(FsClass::Attribute));
    assert_eq!(q.output_bytes, 128);
    assert_eq!(q.credit_bytes, 90_001);
    assert_eq!(q.wire_id, RAW_ID);
    assert_eq!(msg.messages()[0].query_info(127), Err(QueryError::ExceedsMaxTransact));

    let data = framed(&[&info(1, 4, 0)]); // not a successful zero-length query
    let q = parsed(&data).messages()[0].query_info(MAX_TX).unwrap();
    assert_eq!(q.class, InfoClass::File(FileClass::Basic));
    assert_eq!(q.output_bytes, 0);
    assert_eq!(q.credit_bytes, 0);
}

#[test]
fn unsupported_metadata_is_not_mapped_to_another_class_or_a_default() {
    for (kind, class, error) in [
        (1, 15, QueryError::NotSupported), // EA input is not a supported metadata path
        (1, 22, QueryError::NotSupported), // no named streams
        (2, 8, QueryError::NotSupported), // no invented FS object ID
        (3, 0, QueryError::NotSupported), // security
        (4, 0, QueryError::NotSupported), // quotas
        (2, 255, QueryError::InvalidInfoClass),
        (1, 255, QueryError::InvalidInfoClass),
        (255, 5, QueryError::Malformed),
    ] {
        let data = framed(&[&info(kind, class, 256)]);
        assert_eq!(parsed(&data).messages()[0].query_info(MAX_TX), Err(error));
    }
    for (class, expected) in [
        (60, QueryError::NotSupported),
        (79, QueryError::NotSupported),
        (4, QueryError::InvalidInfoClass), // FILE Basic, not a directory class
    ] {
        let data = framed(&[&directory(class, 0, 128, &[])]);
        assert_eq!(parsed(&data).messages()[0].query_directory(MAX_TX), Err(expected));
    }
}

#[test]
fn original_object_distinguishes_logical_root_directories_and_files() {
    let pool = PoolId([1; 16]);
    assert_eq!(QueryObject::from_original(Object::Namespace(pool)), QueryObject::LogicalRoot);
    for dir in [
        Object::OrdinaryRoot(pool, VolumeId([2; 16])),
        Object::Directory(pool, DirectoryId([3; 16])),
        Object::RecycleRoot(pool),
        Object::DeleteRound(pool, OperationId([4; 16])),
    ] {
        assert_eq!(QueryObject::from_original(dir), QueryObject::Directory);
    }
    assert_eq!(QueryObject::from_original(Object::File(pool, FileId([5; 16]))), QueryObject::File);
}

#[test]
fn sizing_accounts_for_utf16_and_inter_record_but_not_final_padding() {
    for (class, fixed) in [(1, 64), (2, 68), (3, 94), (12, 12), (37, 104), (38, 80)] {
        let frame = framed(&[&directory(class, 0, 512, &[])]);
        let q = parsed(&frame).messages()[0].query_directory(MAX_TX).unwrap();
        let mut page = DirectoryPage::start(QueryObject::LogicalRoot, &q, 512).unwrap();
        assert_eq!((page.bytes(), page.records()), (0, 0));
        assert_eq!(page.offer("𐐀甲"), Ok(RecordSpace { offset: 0, name_bytes: 6 }));
        assert_eq!(page.bytes(), fixed + 6);
        let next = (fixed + 6 + 7) & !7;
        assert_eq!(page.offer("x"), Ok(RecordSpace { offset: next, name_bytes: 2 }));
        assert_eq!((page.bytes(), page.records()), (next + fixed + 2, 2));
    }
}

#[test]
fn page_is_a_sticky_prefix_with_separate_short_buffer_and_local_limits() {
    let frame = framed(&[&directory(12, 0, 30, &[])]);
    let q = parsed(&frame).messages()[0].query_directory(MAX_TX).unwrap();
    assert_eq!(DirectoryPage::start(QueryObject::File, &q, 30).unwrap_err(), PageStop::NotDirectory);
    let mut page = DirectoryPage::start(QueryObject::Directory, &q, 512).unwrap();
    assert_eq!(page.offer("a"), Ok(RecordSpace { offset: 0, name_bytes: 2 }));
    assert_eq!(page.offer("bb"), Err(PageStop::Full)); // 16 + 12 + 4 > 30
    assert_eq!(page.offer("c"), Err(PageStop::Full)); // next shorter row would fit
    assert_eq!((page.bytes(), page.records()), (14, 1));

    let frame = framed(&[&directory(12, 0, 14, &[])]);
    let q = parsed(&frame).messages()[0].query_directory(MAX_TX).unwrap();
    let mut page = DirectoryPage::start(QueryObject::Directory, &q, 14).unwrap();
    assert_eq!(page.offer("𐐀"), Err(PageStop::FirstRecordOverflow { name_space: 2 }));
    assert_eq!(page.offer("a"), Err(PageStop::FirstRecordOverflow { name_space: 2 }));
    assert_eq!((page.bytes(), page.records()), (0, 0));
    assert_eq!(DirectoryPage::start(QueryObject::Directory, &q, 11).unwrap_err(), PageStop::ResponseSpace);
    let mut page = DirectoryPage::start(QueryObject::Directory, &q, 13).unwrap();
    assert_eq!(page.offer("a"), Err(PageStop::ResponseSpace));

    let frame = framed(&[&directory(12, 0, 11, &[])]);
    let q = parsed(&frame).messages()[0].query_directory(MAX_TX).unwrap();
    assert_eq!(DirectoryPage::start(QueryObject::Directory, &q, 1024).unwrap_err(), PageStop::ClientLengthMismatch);
}

#[test]
fn single_and_full_size_queries_both_obey_record_cap() {
    for (flags, limit) in [(2, 1), (0, 16)] {
        let frame = framed(&[&directory(3, flags, MAX_TX, &[])]);
        let q = parsed(&frame).messages()[0].query_directory(MAX_TX).unwrap();
        let mut page = DirectoryPage::start(QueryObject::LogicalRoot, &q, MAX_TX as usize).unwrap();
        for i in 0..limit {
            assert_eq!(page.offer("x"), Ok(RecordSpace { offset: i * 96, name_bytes: 2 }));
        }
        assert_eq!(page.offer("more"), Err(PageStop::Full));
        assert_eq!((page.bytes(), page.records()), (limit * 96, limit));
    }
}


mod root_pages {
    use super::*;
    use crate::smb::backend::{Facts, Grant, Held, OpenCoordinator, OriginalRequest, ParentWrite, Share,
        query::root::{Child, Fault, OriginalNamespace, RootPage, Selection, records::EncodeFault}};
    use std::{cell::Cell, io, num::NonZeroUsize, rc::Rc};

    #[derive(Default)]
    struct Observe { calls: Cell<usize>, denied: Cell<bool> }
    struct FixtureRoot { rows: Vec<(String, u16)>, observe: Rc<Observe> }

    impl OriginalNamespace for FixtureRoot {
        fn root_projection(&self) -> io::Result<Vec<Child>> {
            self.observe.calls.set(self.observe.calls.get() + 1);
            if self.observe.denied.get() { return Err(io::ErrorKind::PermissionDenied.into()); }
            Ok(self.rows.iter().map(|(name, number)| Child { name: name.clone(), number: u64::from(*number) }).collect())
        }
    }

    struct Intent(Facts);
    impl OriginalRequest for Intent {
        type Held = FixtureRoot;
        type Error = ();
        fn recheck(&self, _: &FixtureRoot) -> Result<Facts, ()> { Ok(self.0) }
    }

    fn coordinator() -> OpenCoordinator { OpenCoordinator::new(NonZeroUsize::new(16).unwrap()) }
    fn pool() -> PoolId { PoolId([1; 16]) }
    fn opened<'c>(owner: &'c OpenCoordinator, object: Object, access: u32, rows: &[(&str, u16)])
        -> (Held<'c, FixtureRoot>, Rc<Observe>)
    {
        let observe = Rc::new(Observe::default());
        let source = FixtureRoot { rows: rows.iter().map(|(n, m)| (String::from(*n), *m)).collect(),
            observe: Rc::clone(&observe) };
        let facts = Facts { object, grant: Grant::from_effective(access).unwrap(), parent_write: ParentWrite::Denied };
        let req = Intent(facts);
        let pending = owner.reserve(&req, facts, Share::from_bits(7).unwrap()).unwrap();
        (pending.bind(source).unwrap(), observe)
    }

    fn page<'a>(open: &'a mut Held<'_, FixtureRoot>, flags: u8, output: u32, room: usize, pattern: &str)
        -> Result<RootPage<'a>, Fault>
    {
        let frame = framed(&[&directory(12, flags, output, &utf16(pattern))]);
        let batch = parsed(&frame);
        let q = batch.messages()[0].query_directory(MAX_TX).unwrap();
        open.prepare_root_query(&q, room)
    }

    fn names<'p>(page: &'p RootPage<'_>, end: bool) -> Vec<(&'p str, u64, usize)> {
        let Selection::Rows { entries, exhausted, .. } = page.output() else { panic!("expected real selected rows"); };
        assert_eq!(*exhausted, end);
        entries.iter().map(|e| (e.child.name.as_str(), e.child.number, e.space.offset)).collect()
    }

    #[test]
    fn only_returned_prefix_advances_the_one_position_and_denial_is_not_end() {
        let owner = coordinator();
        let (mut open, observe) = opened(&owner, Object::Namespace(pool()), 1,
            &[("A", 11), ("bbbbbbbb", 44), ("z", 59), ("盘", 62)]);

        let first = page(&mut open, 0, 30, 4096, "*").unwrap();
        assert_eq!(names(&first, false), [("A", 11, 0)]);
        assert!(matches!(first.output(), Selection::Rows { bytes: 14, .. }));
        drop(first); // failed, cancelled or unsent reply
        let first = page(&mut open, 0, 30, 4096, "*").unwrap();
        assert_eq!(names(&first, false), [("A", 11, 0)]);
        first.written_for_fixture();

        observe.denied.set(true);
        assert!(matches!(page(&mut open, 2, 64, 4096, ""), Err(Fault::Original(e))
            if e.kind() == io::ErrorKind::PermissionDenied));
        observe.denied.set(false);
        // A continuation cannot replace the original pattern (the supplied
        // value is ignored) and starts at the first NOT returned member.
        let second = page(&mut open, 2, 64, 4096, "z").unwrap();
        assert_eq!(names(&second, false), [("bbbbbbbb", 44, 0)]);
        second.written_for_fixture();
        let last = page(&mut open, 0, 30, 4096, "").unwrap();
        assert_eq!(names(&last, true), [("z", 59, 0), ("盘", 62, 16)]);
        last.written_for_fixture();
        let end = page(&mut open, 0, 30, 4096, "").unwrap();
        assert!(matches!(end.output(), Selection::NoMatch { first_query: false }));
        assert_eq!(observe.calls.get(), 6);
    }

    #[test]
    fn search_scope_restarts_only_on_the_same_open_and_retains_exact_case() {
        let owner = coordinator();
        let (mut open, _) = opened(&owner, Object::Namespace(pool()), 1,
            &[("A.txt", 12), ("B.abc.TXT", 2), ("Z", 3), ("a.txt", 4), ("e\u{301}", 5), ("é", 6), ("𐐀档", 7)]);
        let first = page(&mut open, 0, 256, 4096, "a.txt").unwrap();
        assert_eq!(names(&first, true), [("a.txt", 4, 0)]);
        first.written_for_fixture();
        let other = page(&mut open, 0, 256, 4096, "B*").unwrap();
        assert!(matches!(other.output(), Selection::NoMatch { first_query: false }));
        drop(other);

        let restart = page(&mut open, 1, 256, 4096, "").unwrap();
        assert_eq!(names(&restart, true), [("a.txt", 4, 0)]);
        drop(restart);
        let reopen = page(&mut open, 16, 256, 4096, "<.TXT").unwrap();
        assert_eq!(names(&reopen, true), [("B.abc.TXT", 2, 0)]);
        reopen.written_for_fixture();
        let restart = page(&mut open, 1, 256, 4096, "").unwrap();
        assert_eq!(names(&restart, true), [("B.abc.TXT", 2, 0)]);
        drop(restart);

        for (pattern, name, marker) in [
            ("Z>>\"", "Z", 3),
            ("B>.abc.TXT", "B.abc.TXT", 2), // > moves past the pattern token, not the period
            ("??档", "𐐀档", 7),             // two UTF-16 units before 档
            ("e\u{301}", "e\u{301}", 5),
            ("é", "é", 6),
        ] {
            let res = page(&mut open, 16, 256, 4096, pattern).unwrap();
            assert_eq!(names(&res, true), [(name, marker, 0)]);
        }
        for pattern in [".", "..", "?.TXT", "?档"] {
            let res = page(&mut open, 16, 256, 4096, pattern).unwrap();
            assert!(matches!(res.output(), Selection::NoMatch { first_query: false }));
        }
        // Another open shares no search pattern or cursor with this one.
        let (mut another, _) = opened(&owner, Object::Namespace(pool()), 1, &[("Z", 3)]);
        let fresh = page(&mut another, 0, 256, 4096, "").unwrap();
        assert_eq!(names(&fresh, true), [("Z", 3, 0)]);
    }

    #[test]
    fn first_overflow_keeps_original_complete_name_to_retry_without_skipping() {
        let owner = coordinator();
        let (mut open, _) = opened(&owner, Object::Namespace(pool()), 1, &[("𐐀", 3), ("𐐁", 2)]);
        assert!(matches!(page(&mut open, 0, 16, 13, ""), Err(Fault::Budget(PageStop::ResponseSpace))));
        let partial = page(&mut open, 0, 14, 4096, "").unwrap();
        let Selection::FirstOverflow { child, name_space } = partial.output() else { panic!("expected overflow"); };
        assert_eq!((child.name.as_str(), child.number, *name_space), ("𐐀", 3, 2));
        partial.written_for_fixture();
        let full = page(&mut open, 0, 16, 4096, "").unwrap();
        assert_eq!(names(&full, false), [("𐐀", 3, 0)]);
        full.written_for_fixture();
        assert_eq!(names(&page(&mut open, 0, 16, 4096, "").unwrap(), true), [("𐐁", 2, 0)]);
    }

    #[test]
    fn the_full_bounded_projection_is_consumed_in_record_capped_pages() {
        let owner = coordinator();
        let (mut open, _) = opened(&owner, Object::Namespace(pool()), 1, &[]);
        open.original.rows = (0..63u16).map(|n| (format!("{n:02}"), n + 2)).collect();
        for part in 0..4u16 {
            let start = part * 16;
            let end = (start + 16).min(63);
            let p = page(&mut open, 0, 4000, 4096, "*.*").unwrap();
            let returned: Vec<u64> = names(&p, end == 63).into_iter().map(|(_, number, _)| number).collect();
            assert_eq!(returned, (start + 2..end + 2).map(u64::from).collect::<Vec<_>>());
            p.written_for_fixture();
        }
        let exhausted = page(&mut open, 0, 4000, 4096, "").unwrap();
        assert!(matches!(exhausted.output(), Selection::NoMatch { first_query: false }));
    }

    #[test]
    fn validated_projection_encodes_only_complete_directory_records() {
        for (class, fixed, next, id_at) in [
            (1, 64, 72, None), (2, 68, 72, None), (3, 94, 96, None),
            (12, 12, 16, None), (37, 104, 112, Some(96)), (38, 80, 88, Some(72)),
        ] {
            let owner = coordinator();
            let (mut open, _) = opened(&owner, Object::Namespace(pool()), 1, &[("A", 3), ("盘𐐀", 64)]);
            let frame = framed(&[&directory(class, 0, 512, &[])]);
            let parsed = parsed(&frame);
            let q = parsed.messages()[0].query_directory(MAX_TX).unwrap();

            let encoded = open.prepare_root_query(&q, 512).unwrap().encode_complete().unwrap();
            let bytes = encoded.data();
            assert_eq!(bytes.len(), next + fixed + 6); // no final alignment padding
            assert_eq!(&bytes[fixed + 2..next], &[0; 8][..next - (fixed + 2)]);
            for (at, next_offset, number, name) in [
                (0, next as u32, 3u64, &[b'A', 0][..]),
                (next, 0, 64, &[0xd8, 0x76, 0x01, 0xd8, 0, 0xdc][..]),
            ] {
                let record = &bytes[at..at + fixed + name.len()];
                assert_eq!(&record[..4], &next_offset.to_le_bytes());
                assert_eq!(&record[4..8], &[0; 4]); // never an index cursor
                assert_eq!(&record[fixed..], name); // exact UTF-16LE; no terminator
                let namesize = (name.len() as u32).to_le_bytes();
                if class == 12 {
                    assert_eq!(&record[8..12], &namesize);
                } else {
                    assert_eq!(&record[8..56], &[0; 48]); // no times/data stream for these logical folders
                    assert_eq!(&record[56..60], &[0x10, 0, 0, 0]);
                    assert_eq!(&record[60..64], &namesize);
                    let end_of_reserved = id_at.unwrap_or(fixed);
                    assert!(record[64..end_of_reserved].iter().all(|&b| b == 0));
                    if let Some(pos) = id_at {
                        assert_eq!(&record[pos..pos + 8], &number.to_le_bytes());
                    }
                }
            }
            drop(encoded); // bytes prepared but not submitted: first page unchanged
            let encoded = open.prepare_root_query(&q, 512).unwrap().encode_complete().unwrap();
            assert_eq!(encoded.data().len(), next + fixed + 6);
            encoded.written_for_fixture();
            let end = open.prepare_root_query(&q, 512).unwrap();
            assert!(matches!(end.output(), Selection::NoMatch { first_query: false }));
            assert!(matches!(end.encode_complete(), Err(EncodeFault::NotComplete)));
        }
    }

    #[test]
    fn overflow_and_impossible_namespace_numbers_do_not_encode_success_data() {
        let owner = coordinator();
        let (mut open, _) = opened(&owner, Object::Namespace(pool()), 1, &[("𐐀", 3)]);
        let residual = page(&mut open, 0, 14, 4096, "").unwrap();
        assert!(matches!(residual.encode_complete(), Err(EncodeFault::NotComplete)));
        let full = page(&mut open, 0, 16, 4096, "").unwrap().encode_complete().unwrap();
        assert_eq!(&full.data()[12..], &[1, 0xd8, 0, 0xdc]);
        drop(full);
        for rows in [&[("one", 1)][..], &[("large", 65)], &[("a", 3), ("b", 3)]] {
            let (mut bad, _) = opened(&owner, Object::Namespace(pool()), 1, rows);
            assert!(matches!(page(&mut bad, 0, 512, 4096, ""), Err(Fault::InvalidProjection)));
        }
    }

    #[test]
    fn denied_nonroot_invalid_projection_and_true_empty_remain_distinct() {
        let owner = coordinator();
        let (mut file, observe) = opened(&owner, Object::File(pool(), FileId([2; 16])), 1, &[]);
        assert!(matches!(page(&mut file, 0, 512, 4096, ""), Err(Fault::NotNamespace)));
        assert_eq!(observe.calls.get(), 0);
        let (mut no_list, observe) = opened(&owner, Object::Namespace(pool()), 0x80, &[]);
        assert!(matches!(page(&mut no_list, 0, 512, 4096, ""), Err(Fault::ListDenied)));
        assert_eq!(observe.calls.get(), 0);

        for bad in [&[("z", 3), ("a", 2)][..], &[("dup", 3), ("dup", 2)], &[("host\\leaf", 3)]] {
            let (mut root, _) = opened(&owner, Object::Namespace(pool()), 1, bad);
            assert!(matches!(page(&mut root, 0, 512, 4096, ""), Err(Fault::InvalidProjection)));
        }
        let (mut too_many, _) = opened(&owner, Object::Namespace(pool()), 1, &[]);
        too_many.original.rows = (0..64u16).map(|n| (format!("{n:02}"), n)).collect();
        assert!(matches!(page(&mut too_many, 0, 512, 4096, ""), Err(Fault::InvalidProjection)));

        let (mut root, _) = opened(&owner, Object::Namespace(pool()), 1, &[]);
        let first = page(&mut root, 0, 512, 4096, "").unwrap();
        assert!(matches!(first.output(), Selection::NoMatch { first_query: true }));
        first.written_for_fixture();
        let next = page(&mut root, 0, 512, 4096, "").unwrap();
        assert!(matches!(next.output(), Selection::NoMatch { first_query: false }));
    }
}

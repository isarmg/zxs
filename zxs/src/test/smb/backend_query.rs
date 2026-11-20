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

use super::*;
use crate::smb::wire::{direct::Frame, message::Compound};

fn w16(b: &mut [u8], at: usize, x: u16) { b[at..at+2].copy_from_slice(&x.to_le_bytes()); }
fn w32(b: &mut [u8], at: usize, x: u32) { b[at..at+4].copy_from_slice(&x.to_le_bytes()); }
fn utf(s: &str) -> Vec<u8> { s.encode_utf16().flat_map(u16::to_le_bytes).collect() }
fn header(cmd: u16, n: usize) -> Vec<u8> {
    let mut b = vec![0; n]; b[..4].copy_from_slice(b"\xfeSMB"); w16(&mut b,4,64); w16(&mut b,12,cmd); b
}
fn create(name: &[u8]) -> Vec<u8> {
    let mut p = header(5,120); w16(&mut p,64,57); w16(&mut p,108,120);
    w16(&mut p,110,u16::try_from(name.len()).unwrap());
    p.extend_from_slice(name); if name.is_empty() { p.push(0); } p
}
fn rename(name: &[u8]) -> Vec<u8> {
    let mut p = header(0x11,96); w16(&mut p,64,33); p[66]=1; p[67]=10;
    w16(&mut p,72,96); p[80..96].fill(0xff);
    let mut b=vec![0;20]; b[0]=0x41; b[1..8].fill(0x89);
    w32(&mut b,16,u32::try_from(name.len()).unwrap());
    b.extend_from_slice(name); b.resize(b.len().max(24),0xec);
    w32(&mut p,68,b.len() as u32); p.extend_from_slice(&b); p
}
fn with_first<T>(p: &[u8], inspect: impl FnOnce(&Message<'_>) -> T) -> T {
    let mut tcp=(p.len() as u32).to_be_bytes().to_vec(); tcp.extend_from_slice(p);
    let (frame, tail)=Frame::split(&tcp).unwrap().unwrap(); assert!(tail.is_empty());
    let chain=Compound::parse(frame).unwrap(); inspect(&chain.messages()[0])
}
fn parts(p: &TreeName) -> Vec<String> { p.components().map(str::to_owned).collect() }
fn open_name(p: &[u8]) -> Result<Option<Vec<String>>, NameError> {
    with_first(p, |m| Ok(CreateSpelling::from_message(m)?.name.as_ref().map(parts)))
}
fn destination(p: &[u8]) -> Result<Vec<String>, NameError> {
    with_first(p, |m| Ok(parts(&RenameSpelling::from_message(m)?.destination)))
}

#[test]
fn original_spelling_and_root_are_not_directory_or_identity_grants() {
    assert_eq!(open_name(&create(&[])).unwrap(), None);
    let name="盘\\e\u{0301}\\É\\文件😀.xlsx";
    assert_eq!(open_name(&create(&utf(name))).unwrap().unwrap(), ["盘","e\u{0301}","É","文件😀.xlsx"]);
    assert_ne!(open_name(&create(&utf("F\\A"))).unwrap(),open_name(&create(&utf("F\\a"))).unwrap());
    assert_ne!(open_name(&create(&utf("F\\é"))).unwrap(),open_name(&create(&utf("F\\e\u{0301}"))).unwrap());
    // The parser never assumes that this is the real recycle name / original delete operation.
    assert_eq!(open_name(&create(&utf("回收\\不是删除单\\plan-abcd.xlsx"))).unwrap().unwrap(),
               ["回收","不是删除单","plan-abcd.xlsx"]);
    let mut p=create(&[]); p.resize(128,0); w32(&mut p,112,128); w32(&mut p,116,16);
    p.extend_from_slice(&[0xda;16]);
    with_first(&p, |m| {
        let v=CreateSpelling::from_message(m).unwrap();
        assert!(v.name.is_none()); assert_eq!(v.context_area,&[0xda;16]);
    });
}

#[test]
fn no_host_paths_folding_streams_or_lossy_utf16() {
    for n in [r"\F\a", r"\\host\ZXS\F\a", r"F\\a", r"F\.\a", r"F\..\a", "F\\a\\",
              "F/a", "F\\a::$DATA", "F\\bad ", "F\\a.", "F\\a\0b", "F\\*"] {
        assert_eq!(open_name(&create(&utf(n))).err(),Some(NameError::InvalidName),"{n:?}");
    }
    for raw in [&[0x41][..], &[0x00,0xd8][..], &[0x00,0xdc][..], &[0x00,0xd8,0x41,0][..]] {
        assert_eq!(open_name(&create(raw)).err(),Some(NameError::InvalidName));
    }
    let wide="盘".repeat(86); // 258 UTF-8 bytes < 255 UTF-16 units
    assert_eq!(open_name(&create(&utf(&wide))).err(),Some(NameError::TooLong));
    let total="a".repeat(32_761);
    assert_eq!(open_name(&create(&utf(&total))).err(),Some(NameError::TooLong));
}

#[test]
fn rename_target_belongs_to_one_tree_and_replace_is_only_the_requested_flag() {
    for raw in ["盘\\计划.xlsx","\\盘\\计划.xlsx"] {
        let p=rename(&utf(raw));
        with_first(&p, |m| {
            let v=RenameSpelling::from_message(m).unwrap();
            assert_eq!(parts(&v.destination),["盘","计划.xlsx"]);
            assert!(v.requested_replace);
            assert_eq!(v.source_wire_id,[0xff;16]); // not resolved to any persistent business identifier
        });
    }
    let mut p=rename(&utf("b")); p[96]=0;
    assert_eq!(destination(&p).unwrap(),["b"]); // nonzero alignment padding is not a name suffix
    with_first(&p,|m| assert!(!RenameSpelling::from_message(m).unwrap().requested_replace));
    for s in ["", "\\", r"\\other\share\x", r"\??\C:\x", "盘\\..\\x"] {
        assert_eq!(destination(&rename(&utf(s))).err(),Some(NameError::InvalidName));
    }
    let mut p=rename(&utf("F\\x")); p[104]=1;
    assert_eq!(destination(&p).err(),Some(NameError::Field(FieldError::Malformed)));
    let mut p=rename(&utf("F\\x")); p[67]=65;
    assert_eq!(destination(&p).err(),Some(NameError::Field(FieldError::Unsupported)));
}

#[test]
fn range_checks_never_borrow_fixed_fields_padding_or_the_following_command() {
    let mut p=create(&utf("F\\name")); w16(&mut p,108,112);
    assert_eq!(open_name(&p).err(),Some(NameError::Field(FieldError::Malformed)));
    let mut p=create(&utf("F\\name")); w16(&mut p,108,121);
    assert_eq!(open_name(&p).err(),Some(NameError::Field(FieldError::Malformed)));
    let mut p=create(&utf("F\\name")); p.resize(160,0); w32(&mut p,112,120); w32(&mut p,116,16);
    assert_eq!(open_name(&p).err(),Some(NameError::Field(FieldError::Malformed)));
    let mut p=create(&[]); w32(&mut p,112,128);
    assert_eq!(open_name(&p).err(),Some(NameError::Field(FieldError::Malformed)));
    let mut p=create(&utf("x")); p.resize(128,0); w32(&mut p,20,128);
    w16(&mut p,108,128); p.extend_from_slice(&header(13,68));
    assert_eq!(open_name(&p).err(),Some(NameError::Field(FieldError::Malformed)));
    let mut p=rename(&utf("F\\x")); w32(&mut p,68,24);
    p.extend_from_slice(&[0,0]); // not part of the inner BufferLength
    assert_eq!(destination(&p).err(),Some(NameError::Field(FieldError::RenameInformationLength)));
    let mut p=rename(&utf("F\\x")); w16(&mut p,72,80);
    assert_eq!(destination(&p).err(),Some(NameError::Field(FieldError::Malformed)));
    let mut p=create(&utf("F")); w32(&mut p,104,0x2000);
    assert_eq!(open_name(&p).err(),Some(NameError::Field(FieldError::Unsupported)));
    let mut p=create(&utf("F")); w32(&mut p,16,0x1000_0000);
    assert_eq!(open_name(&p).err(),Some(NameError::Field(FieldError::Unsupported)));
}

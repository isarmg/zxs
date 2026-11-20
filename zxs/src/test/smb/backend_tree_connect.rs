//! Bounded TREE_CONNECT shapes and the original authenticated receipt.
use super::*;
use crate::smb::{
    connection::sessions::ordinary::tests::with_tree_member,
    wire::{direct::Frame, message::{Compound, Routing}},
};

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn packet(name: &[u8], flags: u16, offset: u16) -> Vec<u8> {
    let mut p = vec![0; usize::from(offset).max(72)];
    p[..4].copy_from_slice(b"\xfeSMB");
    p[4..6].copy_from_slice(&64u16.to_le_bytes());
    p[12..14].copy_from_slice(&3u16.to_le_bytes());
    p[64..66].copy_from_slice(&9u16.to_le_bytes());
    p[66..68].copy_from_slice(&flags.to_le_bytes());
    p[68..70].copy_from_slice(&offset.to_le_bytes());
    p[70..72].copy_from_slice(&u16::try_from(name.len()).unwrap().to_le_bytes());
    p.extend_from_slice(name);
    p
}

fn decode(payload: &[u8]) -> Result<String, Error> {
    let mut tcp = u32::try_from(payload.len()).unwrap().to_be_bytes().to_vec();
    tcp.extend_from_slice(payload);
    let (frame, remaining) = Frame::split(&tcp).unwrap().unwrap();
    assert!(remaining.is_empty());
    let message = Compound::parse(frame).unwrap();
    message.messages()[0].tree_connect_name_field().map_err(Error::Field).and_then(decode_unc)
}

#[test]
fn signed_paid_member_and_session_survive_the_unc_boundary() {
    with_tree_member(false, |member| {
        let original = std::ptr::from_ref(member.message());
        let Ok(parsed) = TreeConnectRequest::parse(member) else { panic!("original TREE_CONNECT"); };
        assert_eq!(parsed.server_spelling(), "fixture");
        assert_eq!(std::ptr::from_ref(parsed.original().message()), original);
        assert_eq!(parsed.original().ordinal(), 0);
        assert_eq!(parsed.original().message().header.message, 3);
        assert_eq!(parsed.original().message().header.session, 1);
        assert_eq!(parsed.original().message().header.routing, Routing::Sync { tree: 0 });
        parsed.original().session().recheck().unwrap(); // still only a session
    });
    with_tree_member(true, |member| {
        let original = std::ptr::from_ref(member.message());
        let Err((member, why)) = TreeConnectRequest::parse(member) else { panic!("stale session"); };
        assert_eq!(why, Error::Session(ConnectionFault::SessionState));
        assert_eq!(std::ptr::from_ref(member.message()), original);
    });
}

#[test]
fn only_zxs_is_a_share_regardless_of_the_childs_role() {
    for (path, host) in [
        (r"\\fixture\zXs", "fixture"),
        (r"\\127.0.0.1\ZXS", "127.0.0.1"),
        (r"\\2001:db8::1\ZXS", "2001:db8::1"),
        (r"\\2001-DB8-D87-FFFF-CCAA-132B-1-221B.ipv6-literal.net\ZXS",
            "2001-DB8-D87-FFFF-CCAA-132B-1-221B.ipv6-literal.net"),
        (r"\\盘.local.\ZXS", "盘.local."),
    ] {
        assert_eq!(decode(&packet(&utf16(path), 0, 72)).as_deref(), Ok(host));
    }
    for share in ["RW", "RO", "回收站", "IPC$", "ZXS$", "zxs-old"] {
        let path = format!(r"\\fixture\{share}");
        assert_eq!(decode(&packet(&utf16(&path), 0, 72)), Err(Error::UnknownShare));
        let child = format!(r"\\fixture\ZXS\{share}");
        assert_eq!(decode(&packet(&utf16(&child), 0, 72)), Err(Error::BadUnc));
    }
}

#[test]
fn invalid_unc_is_distinct_from_an_unknown_well_formed_share() {
    for path in [
        r"\fixture\ZXS", r"\\\fixture\ZXS", r"\\fixture\", r"\\.\ZXS", r"\\..\ZXS",
        r"\\ \ZXS", r"\\fixture:445\ZXS", r"\\fixture//root\ZXS", r"\\?\ZXS",
        r"\\fixture\Z*X", "\\\\fixture\\ZXS\0",
        r"\\bad host that is too long.example\ZXS", r"\\example.-bad\ZXS",
    ] {
        assert_eq!(decode(&packet(&utf16(path), 0, 72)), Err(Error::BadUnc));
    }
    let mut surrogate = utf16(r"\\fixture\ZX");
    surrogate.extend_from_slice(&0xd800u16.to_le_bytes());
    assert_eq!(decode(&packet(&surrogate, 0, 72)), Err(Error::BadUnc));
    for path in [
        format!(r"\\{}\ZXS", "h".repeat(TREE_SERVER_UNITS + 1)),
        format!(r"\\{}\ZXS", "a".repeat(64)),
        format!(r"\\h\{}", "s".repeat(TREE_SHARE_UNITS + 1)),
    ] {
        assert_eq!(decode(&packet(&utf16(&path), 0, 72)), Err(Error::BadUnc));
    }
}

#[test]
fn flags_and_offsets_are_limited_to_this_message() {
    let n = utf16(r"\\fixture\ZXS");
    // PathOffset is relative to this SMB2 header; use an in-range gap.
    assert_eq!(decode(&packet(&n, 2, 77)).as_deref(), Ok("fixture"));
    for flag in [1, 4, 5, 6] {
        assert_eq!(decode(&packet(&n, flag, 72)), Err(Error::Field(FieldError::Unsupported)));
    }
    for p in [packet(&n, 8, 72), packet(&n, 0, 71), packet(&n[..n.len() - 1], 0, 72),
              packet(&[], 0, 72), packet(&vec![0; (2 + TREE_SERVER_UNITS + 1 + TREE_SHARE_UNITS) * 2 + 2], 0, 72)] {
        assert_eq!(decode(&p), Err(Error::Field(FieldError::Malformed)));
    }

    let mut p = packet(&n, 0, 72);
    p[36] = 1;
    assert_eq!(decode(&p), Err(Error::Field(FieldError::Malformed)));
    p[36] = 0;
    for flag in [0x1000_0000u32, 0x2000_0000] {
        p[16..20].copy_from_slice(&flag.to_le_bytes());
        assert_eq!(decode(&p), Err(Error::Field(FieldError::Unsupported)));
    }
    p[16..20].fill(0);
    p[64] = 8;
    assert_eq!(decode(&p), Err(Error::Field(FieldError::Malformed)));
    p[12] = 13;
    assert_eq!(decode(&p), Err(Error::Field(FieldError::WrongCommand)));

    let mut compound = packet(&n, 0, 72);
    let next = compound.len().next_multiple_of(8);
    compound.resize(next, 0);
    compound[20..24].copy_from_slice(&u32::try_from(next).unwrap().to_le_bytes());
    compound.extend_from_slice(&packet(&n, 0, 72));
    assert_eq!(decode(&compound).as_deref(), Ok("fixture"));
    compound[70..72].copy_from_slice(&u16::try_from(next - 72 + 2).unwrap().to_le_bytes());
    assert_eq!(decode(&compound), Err(Error::Field(FieldError::Malformed)));
}

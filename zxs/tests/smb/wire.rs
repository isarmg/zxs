use crate::smb::wire::{
    Error,
    direct::{Frame, MAX_FRAME_BYTES},
    message::{Compound, MAX_COMMANDS, Routing},
    negotiate::Offer,
};

fn set16(p: &mut [u8], at: usize, n: u16) {
    p[at..at + 2].copy_from_slice(&n.to_le_bytes());
}
fn set32(p: &mut [u8], at: usize, n: u32) {
    p[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
fn header(cmd: u16, bytes: usize) -> Vec<u8> {
    let mut p = vec![0; bytes];
    p[..4].copy_from_slice(b"\xfeSMB");
    set16(&mut p, 4, 64);
    set16(&mut p, 12, cmd);
    p
}
fn tcp(p: &[u8]) -> Vec<u8> {
    let mut out = (p.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(p);
    out
}
fn envelope(tcp: &[u8]) -> Result<Compound<'_>, Error> {
    let (f, suffix) = Frame::split(tcp)?.expect("complete test frame");
    assert!(suffix.is_empty());
    Compound::parse(f)
}
fn offer(tcp: &[u8]) -> Result<Offer<'_>, Error> {
    Offer::parse(&envelope(tcp)?)
}
const HASH: &[u8] = &[1, 0, 3, 0, 1, 0, 0xd0, 0xd1, 0xd2];

fn negotiate(dialects: &[u16], contexts: &[(u16, &[u8])]) -> Vec<u8> {
    let mut p = header(0, 100);
    set16(&mut p, 64, 36);
    set16(&mut p, 66, dialects.len() as u16);
    set16(&mut p, 68, 3);
    set32(&mut p, 72, 0x44);
    p[76..92].fill(0xab);
    for d in dialects { p.extend_from_slice(&d.to_le_bytes()); }
    let ctx_at = p.len().next_multiple_of(8);
    set32(&mut p, 92, ctx_at as u32);
    set16(&mut p, 96, contexts.len() as u16);
    for &(kind, data) in contexts {
        p.resize(p.len().next_multiple_of(8), 0x93); // ignored on receipt
        p.extend_from_slice(&kind.to_le_bytes());
        p.extend_from_slice(&(data.len() as u16).to_le_bytes());
        p.extend_from_slice(&[0x42; 4]); // reserved, ignored on receipt
        p.extend_from_slice(data);
    }
    p
}

#[test]
fn frame_budget_precedes_body_and_preserves_suffix() {
    assert!(Frame::split(&[0, 0, 0]).unwrap().is_none());
    assert!(Frame::split(&[0, 0, 0, 2, 1]).unwrap().is_none());
    assert!(matches!(Frame::split(&[1, 0, 0, 2]), Err(Error::InvalidFrame)));
    assert_eq!(Frame::payload_len([0; 4]), Err(Error::InvalidFrame));
    assert_eq!(Frame::payload_len(((MAX_FRAME_BYTES + 1) as u32).to_be_bytes()), Err(Error::Limit));
    assert_eq!(Frame::payload_len((MAX_FRAME_BYTES as u32).to_be_bytes()), Ok(MAX_FRAME_BYTES));
    let a = tcp(&header(13, 68));
    let pair = [a.as_slice(), a.as_slice()].concat();
    let (frame, tail) = Frame::split(&pair).unwrap().unwrap();
    assert_eq!(tail, a.as_slice());
    assert_eq!(Compound::parse(frame).unwrap().messages()[0].raw, &a[4..]);
}

#[test]
fn headers_use_the_right_id_union_and_keep_compound_padding() {
    let mut first = header(5, 72);
    set32(&mut first, 20, 72);
    set16(&mut first, 6, 2);
    set16(&mut first, 8, 7);
    set16(&mut first, 14, 17);
    set32(&mut first, 36, 42);
    first[24..32].copy_from_slice(&0x1122u64.to_le_bytes());
    first[40..48].copy_from_slice(&0x3344u64.to_le_bytes());
    first[68..72].fill(0xa5);
    let mut last = header(6, 68);
    set32(&mut last, 16, 4);
    let input = tcp(&[first.as_slice(), last.as_slice()].concat());
    let parts = envelope(&input).unwrap();
    let m = parts.messages();
    assert_eq!(m.len(), 2);
    assert_eq!(m[0].raw, first.as_slice());
    assert_eq!(&m[0].raw[68..], &[0xa5; 4]);
    assert_eq!(m[1].raw, last.as_slice());
    let h = &m[0].header;
    assert_eq!((h.command, h.credit_charge, h.credit_request, h.channel_sequence), (5, 2, 17, 7));
    assert_eq!((h.message, h.session, h.routing), (0x1122, 0x3344, Routing::Sync { tree: 42 }));
    let mut cancel = header(12, 68);
    set32(&mut cancel, 16, 2);
    cancel[32..40].copy_from_slice(&0xabcdu64.to_le_bytes());
    let c = tcp(&cancel);
    assert_eq!(envelope(&c).unwrap().messages()[0].header.routing, Routing::AsyncCancel { id: 0xabcd });
}

#[test]
fn entire_chain_is_checked_before_any_member_is_returned() {
    let mut first = header(5, 72);
    for invalid_next in [16, 64, 69, 72, 80, u32::MAX] {
        set32(&mut first, 20, invalid_next);
        assert!(envelope(&tcp(&first)).is_err());
    }
    set32(&mut first, 20, 72);
    let mut tail = header(6, 68);
    set16(&mut tail, 4, 63);
    assert!(envelope(&tcp(&[first.as_slice(), tail.as_slice()].concat())).is_err());
    set32(&mut first, 16, 4);
    assert!(envelope(&tcp(&first)).is_err());
    let mut no = header(5, 72);
    set32(&mut no, 20, 72);
    let mut yes = no.clone();
    set32(&mut yes, 16, 4);
    let end = header(6, 68);
    assert!(envelope(&tcp(&[no.as_slice(), yes.as_slice(), end.as_slice()].concat())).is_err());
    let mut many = no.repeat(MAX_COMMANDS);
    many.extend_from_slice(&end);
    assert!(matches!(envelope(&tcp(&many)), Err(Error::Limit)));
    for magic in [b"\xfdSMB", b"\xfcSMB", b"\xffSMB"] {
        no[..4].copy_from_slice(magic);
        assert!(matches!(envelope(&tcp(&no)), Err(Error::UnsupportedEnvelope)));
    }
    for flags in [1, 2] {
        let mut bad = header(5, 68);
        set32(&mut bad, 16, flags);
        assert!(matches!(envelope(&tcp(&bad)), Err(Error::InvalidRequest)));
    }
}

#[test]
fn client_offer_retains_exact_bytes_and_does_not_grant_features() {
    let ids = [2, 0, 0x02, 0, 0x01, 0];
    let raw = negotiate(&[0x0302, 0x0311], &[(0xaffa, &[1, 2, 3]), (8, &ids), (1, HASH), (2, &ids)]);
    let wrapped = tcp(&raw);
    let o = offer(&wrapped).unwrap();
    assert_eq!(o.request, raw.as_slice());
    assert_eq!(o.request.as_ptr(), wrapped[4..].as_ptr());
    assert_eq!((o.security_mode, o.capabilities, o.client_guid), (3, 0x44, [0xab; 16]));
    assert_eq!(o.salt, &[0xd0, 0xd1, 0xd2]);
    assert!(o.signing_algorithms().unwrap().unwrap().contains(1));
    assert!(o.ciphers().unwrap().unwrap().contains(2));
    let optional = tcp(&negotiate(&[0x0311], &[(1, HASH), (0x0100, &[]), (2, &[8, 0])]));
    let o = offer(&optional).unwrap();
    assert!(o.signing_algorithms().unwrap().is_none());
    // An unprocessed optional inner payload is not an advertised server cipher.
    assert!(matches!(o.ciphers(), Err(Error::InvalidRequest)));
}

#[test]
fn negotiate_does_not_cross_declared_boundaries_or_accept_duplicate_required_contexts() {
    for contexts in [
        vec![(2, &[][..])],
        vec![(1, HASH), (1, HASH)],
        vec![(1, HASH), (8, &[]), (8, &[])],
        vec![(1, &[0, 0, 0, 0])],
        vec![(1, &[1, 0, 6, 0, 1, 0]), (0xff11, &[9; 8])],
    ] {
        assert!(matches!(offer(&tcp(&negotiate(&[0x0311], &contexts))), Err(Error::InvalidRequest)));
    }
    let mut n = negotiate(&[0x0311], &[(1, HASH)]);
    for offset in [72, 100, 105, u32::MAX] {
        set32(&mut n, 92, offset);
        assert!(matches!(offer(&tcp(&n)), Err(Error::InvalidRequest)));
    }
    let mut n = negotiate(&[0x0311], &[(1, HASH)]);
    set16(&mut n, 96, 65);
    assert!(matches!(offer(&tcp(&n)), Err(Error::Limit)));
    assert!(matches!(offer(&tcp(&negotiate(&[0x0202, 0x0302], &[]))), Err(Error::UnsupportedDialect)));
    let unknown_hash = [1, 0, 0, 0, 99, 0];
    assert!(matches!(offer(&tcp(&negotiate(&[0x0311], &[(1, &unknown_hash)]))), Err(Error::NoPreauthHashInCommon)));
    n = negotiate(&[0x0311], &[(1, HASH)]);
    set32(&mut n, 16, 8);
    assert!(matches!(offer(&tcp(&n)), Err(Error::InvalidRequest)));
}

//! Synthetic shape fixtures, not accounts, issued challenges, valid password
//! proofs, logged-on sessions or evidence of interoperability.
use super::*;
use crate::smb::wire::{
    direct::Frame, negotiate::Offer,
    preauth::{NegotiateRequestHash, tests::synthetic_exchange},
};

pub(in crate::smb::session) const NTLM: &[u8] = &[0x2b, 6, 1, 4, 1, 0x82, 0x37, 2, 2, 10];
pub(in crate::smb::session) const KRB: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x12, 1, 2, 2];
const SPNEGO: &[u8] = &[0x2b, 6, 1, 5, 5, 2];
const NF: u32 = 0x0208_0201; // Unicode + NTLM + ESS + VERSION

fn tlv(tag: u8, p: &[u8]) -> Vec<u8> {
    let mut v = vec![tag];
    match p.len() {
        0..=127 => v.push(p.len() as u8),
        128..=255 => v.extend([0x81, p.len() as u8]),
        _ => { v.push(0x82); v.extend(u16::try_from(p.len()).unwrap().to_be_bytes()); }
    }
    v.extend_from_slice(p);
    v
}
fn list(oids: &[&[u8]]) -> Vec<u8> {
    tlv(0x30, &oids.iter().flat_map(|id| tlv(6, id)).collect::<Vec<_>>())
}
fn init_with(oids: &[&[u8]], extra: &[u8]) -> Vec<u8> {
    let mut seq = tlv(0xa0, &list(oids));
    seq.extend_from_slice(extra);
    let mut body = tlv(6, SPNEGO);
    body.extend(tlv(0xa0, &tlv(0x30, &seq)));
    tlv(0x60, &body)
}
pub(in crate::smb::session) fn init(oids: &[&[u8]], optimistic: Option<&[u8]>) -> Vec<u8> {
    init_with(oids, &optimistic.map_or_else(Vec::new, |v| tlv(0xa2, &tlv(4, v))))
}
pub(in crate::smb::session) fn resp(state: Option<u8>, selected: bool, token: Option<&[u8]>, mic: Option<&[u8]>) -> Vec<u8> {
    let mut seq = Vec::new();
    if let Some(s) = state { seq.extend(tlv(0xa0, &tlv(10, &[s]))); }
    if selected { seq.extend(tlv(0xa1, &tlv(6, NTLM))); }
    if let Some(t) = token { seq.extend(tlv(0xa2, &tlv(4, t))); }
    if let Some(m) = mic { seq.extend(tlv(0xa3, &tlv(4, m))); }
    tlv(0xa1, &tlv(0x30, &seq))
}

fn p16(v: &mut [u8], at: usize, n: u16) { v[at..at+2].copy_from_slice(&n.to_le_bytes()); }
fn p32(v: &mut [u8], at: usize, n: u32) { v[at..at+4].copy_from_slice(&n.to_le_bytes()); }
fn p64(v: &mut [u8], at: usize, n: u64) { v[at..at+8].copy_from_slice(&n.to_le_bytes()); }
fn field(v: &mut Vec<u8>, at: usize, data: &[u8]) {
    let pos = v.len() as u32;
    p16(v, at, data.len() as u16);
    p16(v, at+2, data.len() as u16);
    p32(v, at+4, pos);
    v.extend_from_slice(data);
}
fn primitive(which: u32, fixed: usize, flag_at: usize, flags: u32) -> Vec<u8> {
    let mut v = vec![0; fixed];
    v[..8].copy_from_slice(b"NTLMSSP\0");
    p32(&mut v, 8, which);
    p32(&mut v, flag_at, flags);
    v
}
fn one_ntlm() -> Vec<u8> { primitive(1, 40, 12, NF) }
fn avs(mic: bool) -> Vec<u8> {
    let mut v = vec![1,0,2,0,b'N',0,2,0,2,0,b'D',0,7,0,8,0,1,2,3,4,5,6,7,8];
    if mic { v.extend([6,0,4,0,2,0,0,0]); }
    v.extend([0; 4]);
    v
}
fn two_ntlm() -> Vec<u8> {
    let mut v = primitive(2, 56, 20, NF | 0x0080_0000);
    v[24..32].fill(0x31); // arbitrary public wire marker, never an issued challenge
    field(&mut v, 40, &avs(false));
    v
}
fn three_ntlm(mic: bool) -> Vec<u8> {
    let mut v = primitive(3, if mic { 88 } else { 72 }, 60, NF);
    if mic { v[72..88].fill(0x4d); } // structurally visible, NOT a valid NTLM MIC
    let mut nt = vec![0; 44];
    nt[..16].fill(0x55); // not a valid proof
    nt[16..18].copy_from_slice(&[1,1]);
    nt.extend(avs(mic));
    nt.extend([0; 4]); // optional final reserved bytes of MS-NLMP §3.3.2
    field(&mut v, 20, &nt);
    field(&mut v, 36, b"A\0");
    v
}

// NegReq/Rsp and the client/server SMB2 headers are from the synthetic SHA
// fixtures, not from a service. GAP and suffix are outside the declared blob.
pub(in crate::smb::session) fn smb(token: &[u8], response: bool, mid: u64, sid: u64, gap: u8) -> Vec<u8> {
    let seeds = synthetic_exchange();
    let (idx, fixed, fields) = if response { (3,72,68) } else { (2,88,76) };
    let mut v = seeds[idx][..fixed].to_vec();
    p64(&mut v, 24, mid);
    p64(&mut v, 40, sid);
    p16(&mut v, fields, (fixed + 2) as u16);
    p16(&mut v, fields+2, u16::try_from(token.len()).unwrap());
    v.extend([gap,0x77]);
    v.extend_from_slice(token);
    v.push(0xee);
    v
}
pub(in crate::smb::session) fn framed(p: &[u8]) -> Vec<u8> {
    let mut v = (p.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(p);
    v
}
pub(in crate::smb::session) fn compound(frame: &[u8]) -> Compound<'_> {
    let (f, rest) = Frame::split(frame).unwrap().unwrap();
    assert!(rest.is_empty());
    Compound::parse(f).unwrap()
}
fn conn() -> ConnectionHash {
    let [req, rsp, ..] = synthetic_exchange();
    let buffer = framed(&req);
    let c = compound(&buffer);
    NegotiateRequestHash::received(&Offer::parse(&c).unwrap()).unwrap().reply(&rsp).unwrap()
}
fn first(blob: &[u8]) -> Result<First> {
    let bytes = framed(&smb(blob, false, 1, 0, 0));
    begin(&conn(), &compound(&bytes))
}
pub(in crate::smb::session) fn seen(blob: &[u8]) -> HaveNegotiate {
    let First::Negotiate(n) = first(blob).unwrap() else { panic!("expected original Type1") };
    n
}
fn select(blob: &[u8]) -> NeedSelection {
    let First::Select(n) = first(blob).unwrap() else { panic!("expected selection") };
    n
}
fn challenged(wrapped: bool) -> AwaitAuthenticate {
    let n = one_ntlm();
    let c = two_ntlm();
    let (init_token, challenge) = if wrapped {
        (init(&[NTLM], Some(&n)), resp(Some(1), true, Some(&c), None))
    } else { (n, c) };
    seen(&init_token).challenged(&smb(&challenge, true, 1, 42, 0)).unwrap()
}

#[test]
fn exact_der_list_and_first_vs_later_wrappers() {
    let fixed = [0x60,0x1c,6,6,0x2b,6,1,5,5,2,0xa0,0x12,0x30,0x10,0xa0,0x0e,
                 0x30,0x0c,6,10,0x2b,6,1,4,1,0x82,0x37,2,2,10];
    let got = der::initiator_initial(&fixed).unwrap();
    assert!(got.preferred && got.optimistic.is_none());
    assert_eq!(got.list, &fixed[16..]); // 30..., never the a0... outer list tag
    assert_eq!(init(&[NTLM], None).as_slice(), &fixed[..]);
    assert!(der::subsequent(&fixed).is_err());
    assert!(der::initiator_initial(&resp(None, false, None, None)).is_err());
    // Bounded future explicit field (high tag [31]) is ignored; it is not a mechanism.
    let extended = init_with(&[NTLM], &[0xbf,0x1f,2,5,0]);
    assert!(der::initiator_initial(&extended).unwrap().preferred);
    // Init2 negHints is ignored even when supplied by an initiator; no Type1 or identity.
    assert!(matches!(first(&init_with(&[NTLM], &[0xa3,2,0x30,0])), Ok(First::Select(_))));
    // DER named-bit string with one set bit, not a padded 32-bit integer.
    assert!(der::initiator_initial(&init_with(&[NTLM], &[0xa1,4,3,2,7,0x80])).is_ok());
    assert!(der::initiator_initial(&init_with(&[NTLM], &[0xa1,4,3,2,0,0x80])).is_err());
}

#[test]
fn malformed_der_cannot_turn_into_direct_ntlm_or_select_another_mechanism() {
    let good = init(&[NTLM], None);
    let mut nonminimal = vec![0x60,0x81,good[1]];
    nonminimal.extend_from_slice(&good[2..]);
    let mut tail = good;
    tail.extend(one_ntlm());
    for bad in [nonminimal, tail, vec![0x60,0x80,0,0], init(&[],None),
                init(&[SPNEGO,NTLM],None), init(&[KRB],Some(&one_ntlm())),
                init(&[&[0x80,1],NTLM],None), init_with(&[NTLM], &[0xa3,2,0x30,1]),
                init_with(&[NTLM], &[0xa2,2,4,0,0xa2,2,4,0]),
                init_with(&[NTLM], &[0xa2,2,4,0,0xa1,3,3,1,0])] {
        assert!(first(&bad).is_err());
    }
    let many: Vec<&[u8]> = std::iter::repeat_n(NTLM,33).collect();
    assert!(matches!(first(&init(&many,None)), Err(Failure::Limit)));
}

#[test]
fn spnego_preferred_and_direct_are_distinct_unverified_profiles() {
    for wrapped in [false,true] {
        let t3 = three_ntlm(true);
        let list_mic = [0x6d;16]; // not valid; no authenticity is asserted
        let blob = if wrapped { resp(None,false,Some(&t3),Some(&list_mic)) } else { t3.clone() };
        let buffer = framed(&smb(&blob,false,2,42,0x33));
        let end = challenged(wrapped).received(&compound(&buffer)).unwrap();
        assert_eq!(end.type1.bytes(), one_ntlm().as_slice());
        assert_eq!(end.type2.bytes(), two_ntlm().as_slice());
        assert_eq!(end.type3.raw, t3.as_slice());
        assert_eq!(end.type3.user_utf16, b"A\0");
        assert!(end.type3.domain_utf16.is_empty() && end.type3.mic_indicated);
        assert_eq!(&end.type3.nt_response[..16], &[0x55;16]);
        match &end.layer {
            Layer::Direct => assert!(!wrapped && end.list_mic.is_none()),
            Layer::Spnego { list: l, require_list_mic } => {
                assert!(wrapped && *require_list_mic);
                assert_eq!(l.bytes(), list(&[NTLM]).as_slice());
                assert_eq!(end.list_mic, Some(&list_mic[..]));
            }
        }
        // Same security field, different SMB2 gap: transcript changes, not NTLM bytes.
        let other_buf = framed(&smb(&blob,false,2,42,0x44));
        let other = challenged(wrapped).received(&compound(&other_buf)).unwrap();
        assert_ne!(end.request.as_bytes(), other.request.as_bytes());
    }
    let t3 = three_ntlm(false);
    let final_blob = resp(None,false,Some(&t3),None);
    let final_buf = framed(&smb(&final_blob,false,2,42,0));
    let end = challenged(true).received(&compound(&final_buf)).unwrap();
    assert!(matches!(end.layer, Layer::Spnego { require_list_mic: false, .. }));
    let claimed = resp(None,false,Some(&three_ntlm(true)),None);
    let claimed_buf = framed(&smb(&claimed,false,2,42,0));
    assert!(challenged(true).received(&compound(&claimed_buf)).is_err());
}

#[test]
fn other_optimistic_mechanism_is_not_ntlm_and_its_mic_cannot_be_reused() {
    let mut extra = tlv(0xa2,&tlv(4,b"other mechanism"));
    extra.extend(tlv(0xa3,&tlv(4,b"not an NTLM MIC")));
    let initiator = init_with(&[KRB,NTLM],&extra);
    let selection = |state| smb(&resp(Some(state),true,None,None),true,1,42,0);
    assert!(select(&initiator).selected(&selection(1)).is_err());
    let initial_ntlm = framed(&smb(&resp(None,false,Some(&one_ntlm()),None),false,2,42,0));
    let later = || select(&initiator).selected(&selection(3)).unwrap()
        .received(&compound(&initial_ntlm)).unwrap();
    let challenge = smb(&resp(Some(1),false,Some(&two_ntlm()),None),true,2,42,0);
    let finish = |mic: Option<&[u8]>| framed(&smb(
        &resp(None,false,Some(&three_ntlm(false)),mic),false,3,42,0));
    let missing = finish(None);
    assert!(later().challenged(&challenge).unwrap().received(&compound(&missing)).is_err());
    let done = finish(Some(&[0x7f;16]));
    let end = later().challenged(&challenge).unwrap().received(&compound(&done)).unwrap();
    let Layer::Spnego { list: l, require_list_mic } = &end.layer else { panic!("wrong layer") };
    assert!(*require_list_mic);
    assert_eq!(l.bytes(), list(&[KRB,NTLM]).as_slice());
    // A selected-mech field may appear only in the first server reply.
    let repeated = smb(&resp(Some(1),true,Some(&two_ntlm()),None),true,2,42,0);
    assert!(later().challenged(&repeated).is_err());
}

#[test]
fn wrong_role_or_session_never_falls_back_or_completes() {
    let wrapped1 = init(&[NTLM], Some(&one_ntlm()));
    for bad in [smb(&two_ntlm(),true,1,42,0),
                smb(&resp(Some(1),true,Some(&two_ntlm()),None),true,7,42,0),
                smb(&resp(Some(1),true,Some(&two_ntlm()),None),true,1,0,0),
                smb(&resp(Some(0),true,Some(&two_ntlm()),None),true,1,42,0)] {
        assert!(seen(&wrapped1).challenged(&bad).is_err());
    }
    let raw3 = three_ntlm(false);
    let der3 = resp(None,false,Some(&raw3),None);
    for (wrapped, token, sid) in [(true,raw3.as_slice(),42),(false,der3.as_slice(),42),
                                  (true,der3.as_slice(),43),(false,raw3.as_slice(),43)] {
        let buf = framed(&smb(token,false,2,sid,0));
        assert!(challenged(wrapped).received(&compound(&buf)).is_err());
    }
    let odd_mic = resp(None,false,Some(&raw3),Some(&[0;15]));
    let buf = framed(&smb(&odd_mic,false,2,42,0));
    assert!(challenged(true).received(&compound(&buf)).is_err());
    assert!(first(&two_ntlm()).is_err());
    assert!(first(&three_ntlm(false)).is_err());
    let first_direct = one_ntlm();
    let wrong_more = smb(&resp(Some(1),true,Some(&two_ntlm()),None),true,1,42,0);
    assert!(seen(&first_direct).challenged(&wrong_more).is_err());
}

#[test]
fn descriptors_are_message_relative_flag_scoped_and_cannot_alias_the_mic() {
    let mut n = one_ntlm();
    n[16..32].fill(0xff);
    assert!(ntlm::negotiate(&n).is_ok()); // absent supplied-name flags: ignore whole descriptors
    p32(&mut n,12,NF | 0x1000);
    assert!(ntlm::negotiate(&n).is_err());
    let mut c = two_ntlm();
    c[12..20].fill(0xff);
    assert!(ntlm::challenge(&c).is_ok()); // target name not requested
    p32(&mut c,20,NF | 0x0080_0004);
    assert!(ntlm::challenge(&c).is_err());
    let mut a = three_ntlm(true);
    a[22..24].fill(0xff); // MaxLen ignored
    a[52..60].fill(0xff); // no key-exchange flag: ignore whole descriptor
    assert!(ntlm::authenticate(&a).is_ok());
    p32(&mut a,40,72); // nonempty Unicode name points inside the claimed MIC
    assert!(ntlm::authenticate(&a).is_err());
    let mut a = three_ntlm(false);
    p32(&mut a,40,72); // overlaps the NT response
    assert!(ntlm::authenticate(&a).is_err());
    for (at, n) in [(20,24),(36,0),(36,1)] {
        let mut a = three_ntlm(false);
        p16(&mut a,at,n); // NTLMv1-sized, anonymous or odd Unicode length
        assert!(ntlm::authenticate(&a).is_err());
    }
    let mut anon = three_ntlm(false);
    p32(&mut anon,60,NF|0x0800);
    assert!(ntlm::authenticate(&anon).is_err());
    let mut bad_eol = three_ntlm(false);
    // Last byte of the optional reserved trailer (two username bytes follow).
    let last_reserved = bad_eol.len()-3;
    bad_eol[last_reserved] = 1;
    assert!(ntlm::authenticate(&bad_eol).is_err());
}

#[test]
fn preferred_without_an_optimistic_token_starts_with_a_real_selection() {
    let client = init(&[NTLM],None);
    let missing_oid = smb(&resp(Some(1),false,None,None),true,1,42,0);
    assert!(select(&client).selected(&missing_oid).is_err());
    let selected = smb(&resp(Some(1),true,None,None),true,1,42,0);
    let raw_request = framed(&smb(&one_ntlm(),false,2,42,0));
    assert!(select(&client).selected(&selected).unwrap().received(&compound(&raw_request)).is_err());
    let req = framed(&smb(&resp(None,false,Some(&one_ntlm()),None),false,2,42,0));
    let phase = select(&client).selected(&selected).unwrap().received(&compound(&req)).unwrap();
    let more = smb(&resp(None,false,Some(&two_ntlm()),None),true,2,42,0);
    let final_msg = framed(&smb(&resp(None,false,Some(&three_ntlm(false)),None),false,3,42,0));
    let end = phase.challenged(&more).unwrap().received(&compound(&final_msg)).unwrap();
    assert!(matches!(end.layer, Layer::Spnego { require_list_mic: false, .. }));
}

#[test]
fn target_info_has_a_bounded_typed_terminator_not_just_a_version_marker() {
    let mut c = primitive(2,56,20,NF | 0x0080_0000);
    field(&mut c,40,&[0,0,0,0]); // no names or server timestamp
    assert!(ntlm::challenge(&c).is_err());
    let mut auth = three_ntlm(true);
    let start = 88 + 44; // original AVs in this synthetic Type3
    p16(&mut auth,start+6,1); // duplicate NetBIOS computer-name ID
    assert!(ntlm::authenticate(&auth).is_err());
    let mut only_marker = three_ntlm(false);
    p16(&mut only_marker,20,48); // declared NT field ends inside its first AV value
    assert!(ntlm::authenticate(&only_marker).is_err());
}

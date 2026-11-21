//! End-to-end source fixtures, not runtime/interoperability evidence. The
//! previous setup fixtures perform the real original strict handshake using
//! only public synthetic material. SHA-512 -> SP800-108 HMAC-SHA256 -> AES-CMAC
//! tags below were produced independently (Python hashlib/hmac/cryptography).
use super::*;
use crate::smb::connection::{
    sessions::{challenge::tests::{challenged,provider},finish::tests::publish_at,tests::connected_as},
    setup::tests::{frame,parsed},
};
const TREE:&str=concat!(
    "fe534d4240000100000000000300010008000000000000000300000000000000",
    "00000000000000002a0000000000000000000000000000000000000000000000",
    "0900000048001a005c005c0066006900780074007500720065005c005a0058005300");
const FIRST:[&str;2]=["701a0a44733d60ecbdcbf53ef90b7db6","7eb7b4f5bb09abc023fadedf8212097a"];
const RELATED_TAGS:[&str;2]=["d807b5f5cd1a9f3be39be9b0672071d8","42ca15102a790c5bd3c6291e0d38ab4d"];
const SHORT:[&str;2]=["1e7cfeec389124f8024b4281c594fe9b","4c1516b67b02f3dcab9466f6d6baaff7"];
const TWO:[&str;2]=["3329de80d9c2f536ee317c5c26a3778c","d29136853d91d8e365a43ab9e6d47e45"];

fn hex(s:&str)->Vec<u8> {s.as_bytes().chunks_exact(2)
    .map(|p|u8::from_str_radix(std::str::from_utf8(p).unwrap(),16).unwrap()).collect()}
fn packet(cmd:u16,mid:u64,sid:u64,related:bool,next:usize,tag:&str)->Vec<u8> {
    let mut p=hex(TREE);p[12..14].copy_from_slice(&cmd.to_le_bytes());
    p[24..32].copy_from_slice(&mid.to_le_bytes());p[40..48].copy_from_slice(&sid.to_le_bytes());
    if related {p[16]|=4;}
    match cmd {
        3=>{},
        8=>{p.truncate(64);p.resize(113,0);p[64..66].copy_from_slice(&49u16.to_le_bytes());
            p[68..72].copy_from_slice(&65_537u32.to_le_bytes());
            p[80..88].copy_from_slice(&5u64.to_le_bytes());p[88..96].copy_from_slice(&7u64.to_le_bytes());}
        13=>{p.truncate(64);p.extend_from_slice(&[4,0,0,0]);}
        _=>unreachable!(),
    }
    if next!=0 {assert!(p.len()<=next);p.resize(next,0xac);p[20..24].copy_from_slice(&(next as u32).to_le_bytes());}
    p[48..64].copy_from_slice(&hex(tag));p
}
fn pair(first:Vec<u8>,last:Vec<u8>)->Vec<u8> {let mut p=first;p.extend(last);frame(&p)}
fn related()->Vec<u8> {
    pair(packet(3,3,1,false,104,RELATED_TAGS[0]),packet(13,4,u64::MAX,true,0,RELATED_TAGS[1]))
}

#[test]
fn default_and_explicit_cmac_both_require_first_signed_request_on_its_published_session() {
    for (i,explicit) in [false,true].into_iter().enumerate() {
        for unsigned in [false,true] {
            let srv=provider();let mut c=connected_as(&srv,explicit).unwrap();
            assert_eq!(publish_at(&mut c,1),1);
            // Only the provider retains its verifier secret; an idle live
            // session keeps derived session keys but not a password equivalent.
            assert_eq!(srv.run.verifier.as_ref().unwrap().fixture_revision_holders(),1);
            let mut q=packet(3,3,1,false,0,FIRST[i]);if unsigned {q[16]=0;}
            let bytes=frame(&q);let raw=parsed(&bytes);let paid=c.receive(&raw).unwrap();
            match paid.preflight(&mut c) {
                Admission::Ready(mut batch) if !unsigned=>{
                    let Some(Step::Verified(v))=batch.next().unwrap() else {panic!("no original first member")};
                    assert_eq!(v.ordinal(),0);assert!(std::ptr::eq(v.message(),&raw.messages()[0]));
                    v.session().recheck().unwrap();
                    assert!(matches!(v.rust_session_witness(),Err(Fault::NoVerifier)));
                    assert!(batch.next().unwrap().is_none());
                }
                Admission::TransportGone if unsigned=>{}, // mandatory for this actual active-session TCON
                _=>panic!("missing context must not disable signing; an explicit context cannot excuse a missing flag"),
            }
            if unsigned {assert!(matches!(c.alive(),Err(Fault::Closed)));assert!(c.sessions.is_empty());}
        }
    }
}

#[test]
fn submission_and_the_original_transport_are_both_required_even_when_sid_is_known() {
    let srv=provider();let mut c=connected_as(&srv,false).unwrap();let mut other=connected_as(&srv,false).unwrap();
    let (id,_)=challenged(&mut c,false);assert_eq!(id,1); // valid-looking SID, no final was submitted
    let bytes=frame(&packet(3,3,id,false,0,FIRST[0]));let raw=parsed(&bytes);
    let paid=c.receive(&raw).unwrap();
    let Admission::AnotherConnection(paid)=paid.preflight(&mut other) else {panic!("transport receipt transferred")};
    assert!(other.sessions.is_empty());
    let Admission::Refused(r)=paid.preflight(&mut c) else {panic!("in-progress not Valid")};
    assert_eq!(r.failure(),(0,session::Fault::UnknownSession));assert!(std::ptr::eq(r.original.frame,&raw));
    let paid=other.receive(&raw).unwrap();
    let Admission::Refused(missing)=paid.preflight(&mut other) else {panic!("known foreign SID granted")};
    assert_eq!(missing.failure(),(0,session::Fault::UnknownSession));
}

#[test]
fn whole_compound_mac_precedes_every_member_and_covers_the_original_padding_and_related_id() {
    for damaged in [None,Some((4+103,0)),Some((4+104+48,1)),Some((4+104+40,1))] {
        let srv=provider();let mut c=connected_as(&srv,false).unwrap();assert_eq!(publish_at(&mut c,1),1);
        let mut bytes=related();if let Some((at,_))=damaged {bytes[at]^=1;}
        let raw=parsed(&bytes);let paid=c.receive(&raw).unwrap();
        match paid.preflight(&mut c) {
            Admission::Ready(mut batch) if damaged.is_none()=>{
                let Some(Step::Verified(v))=batch.next().unwrap() else {panic!("first")};
                assert_eq!(v.ordinal(),0);assert_eq!(v.message().header.session,1);
                let Some(Step::Verified(v))=batch.next().unwrap() else {panic!("second")};
                assert_eq!(v.ordinal(),1);assert_eq!(v.message().header.session,u64::MAX);
                assert_eq!(v.session().reg.claim.id.get(),1); // wire was never patched
                assert!(batch.next().unwrap().is_none());
            }
            Admission::Refused(r)=>{
                let (_,ordinal)=damaged.unwrap();
                assert_eq!(r.failure(),(ordinal,session::Fault::BadSignature));
                assert_eq!(r.original.debit.entries.len(),2); // no dispatchable prefix, no recycled MID
            }
            _=>panic!("unexpected admission"),
        }
    }
}

#[test]
fn revocation_blocks_the_current_borrow_and_later_steps_not_just_new_batches() {
    let srv=provider();let mut c=connected_as(&srv,false).unwrap();assert_eq!(publish_at(&mut c,1),1);
    let bytes=related();let raw=parsed(&bytes);let paid=c.receive(&raw).unwrap();
    let Admission::Ready(mut batch)=paid.preflight(&mut c) else {panic!("signed batch")};
    let Some(Step::Verified(first))=batch.next().unwrap() else {panic!("original")};
    first.session().recheck().unwrap();srv.run.verifier.as_ref().unwrap().fixture_revoke();
    assert!(first.session().recheck().is_err());assert!(batch.next().is_err());assert!(batch.next().is_err());
}

#[test]
fn insufficient_payload_credit_is_member_error_not_replay_or_an_io_ticket() {
    for chain in [false,true] {
        let srv=provider();let mut c=connected_as(&srv,false).unwrap();assert_eq!(publish_at(&mut c,1),1);
        let last=if chain {packet(13,4,u64::MAX,true,0,RELATED_TAGS[1])} else {packet(13,4,1,false,0,SHORT[1])};
        let bytes=pair(packet(8,3,1,false,120,SHORT[0]),last);let raw=parsed(&bytes);
        let paid=c.receive(&raw).unwrap();let Admission::Ready(mut batch)=paid.preflight(&mut c) else {panic!("valid MACs")};
        assert!(matches!(batch.next().unwrap(),Some(Step::InvalidCharge{ordinal:0})));
        if chain {assert!(matches!(batch.next().unwrap(),Some(Step::AfterFailedCredit{ordinal:1})));}
        else {let Some(Step::Verified(v))=batch.next().unwrap() else {panic!("unrelated command was poisoned")};
            assert_eq!(v.ordinal(),1);assert_eq!(v.message().header.message,4);}
        assert!(batch.next().unwrap().is_none());drop(batch);
        assert!(matches!(c.alive(),Err(Fault::Closed))); // the future emitter did not settle either paid MID
    }
}

#[test]
fn two_unrelated_members_use_their_distinct_real_sessions_on_one_owner() {
    let srv=provider();let mut c=connected_as(&srv,false).unwrap();
    assert_eq!(publish_at(&mut c,1),1);assert_eq!(publish_at(&mut c,3),2);
    let bytes=pair(packet(3,5,1,false,104,TWO[0]),packet(13,6,2,false,0,TWO[1]));
    let raw=parsed(&bytes);let paid=c.receive(&raw).unwrap();
    let Admission::Ready(mut batch)=paid.preflight(&mut c) else {panic!("two published sessions, two keys")};
    for (idx,id) in [1,2].into_iter().enumerate() {
        let Some(Step::Verified(v))=batch.next().unwrap() else {panic!("wrong scope")};
        assert_eq!(v.ordinal(),idx);assert_eq!(v.session().reg.claim.id.get(),id);
        assert!(std::ptr::eq(v.message(),&raw.messages()[idx]));v.session().recheck().unwrap();
    }
    assert!(batch.next().unwrap().is_none());
}

/// Borrow the real first TREE_CONNECT for a source-only backend fixture.
/// The callback neither constructs a member nor can retain it past the batch.
pub(in crate::smb) fn with_tree_member(revoke: bool, check: impl FnOnce(VerifiedMember<'_, '_>)) {
    let srv = provider();
    let mut connection = connected_as(&srv, false).unwrap();
    assert_eq!(publish_at(&mut connection, 1), 1);
    let bytes = frame(&packet(3, 3, 1, false, 0, FIRST[0]));
    let raw = parsed(&bytes);
    let paid = connection.receive(&raw).unwrap();
    let Admission::Ready(mut batch) = paid.preflight(&mut connection) else { panic!("signed fixture"); };
    let Some(Step::Verified(member)) = batch.next().unwrap() else { panic!("first TREE_CONNECT"); };
    if revoke { srv.run.verifier.as_ref().unwrap().fixture_revoke(); }
    check(member);
}

#[test]
fn naked_related_tree_terminates_its_original_live_connection_not_other_sids_or_transports() {
    let srv=provider();let mut c=connected_as(&srv,false).unwrap();let mut other=connected_as(&srv,false).unwrap();
    assert_eq!(publish_at(&mut c,1),1);assert_eq!(publish_at(&mut other,1),2);
    use crate::smb::connection::transport::DirectStream;
    c.stream=Some(DirectStream::Fixture(Box::new(std::io::Cursor::new(Vec::<u8>::new()))));
    other.stream=Some(DirectStream::Fixture(Box::new(std::io::Cursor::new(Vec::<u8>::new()))));
    let mut q=packet(3,3,1,false,0,FIRST[0]);q[16]=0;
    let foreign=frame(&q);let fr=parsed(&foreign);let paid=other.receive(&fr).unwrap();
    let Admission::Refused(denied)=paid.preflight(&mut other) else {panic!("foreign SID is not an original live grant")};
    assert!(other.alive().is_ok());assert!(c.alive().is_ok());assert_eq!(other.sessions.len(),1);
    assert_eq!(denied.failure().0,0);
    let first=packet(3,3,1,false,104,RELATED_TAGS[0]);
    let mut naked=packet(3,4,2,true,0,"00000000000000000000000000000000");naked[16]=4;
    let bytes=pair(first,naked);let original=parsed(&bytes);let paid=c.receive(&original).unwrap();
    assert!(matches!(paid.preflight(&mut c),Admission::TransportGone));
    assert!(matches!(c.alive(),Err(Fault::Closed)));assert!(c.sessions.is_empty());
    assert!(other.alive().is_ok());assert_eq!(other.sessions.len(),1);
    assert_eq!(srv.run.index.lock().unwrap().live.len(),1);
    assert!(c.stream.is_none());assert!(other.stream.is_some());
    drop(denied); // own pending unpaid response cannot affect another transport
}

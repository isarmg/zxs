//! Independently fixed public fixture for the *new* ZXSLOCAL scope only.
//! No account/password, loader, real FD or service. The invented ResponseKeyNT
//! and challenge are fixture bytes; reference proofs use Python stdlib HMAC.
use super::*;
use super::super::{Issued,MechanismProof,ProtectedRevision,Rejected,Result,Saved};
use crate::smb::session::envelope::tests as frame;
use std::{sync::Arc,time::{Duration,Instant}};

const RECORD:&str=concat!(
    "5a58532d525553542d534d422d563200010b000000000000010101010101010101010101010101010202",
    "020202020202020202020202020203030303030303030303030303030303040404040404040404040404",
    "040404040505050505050505050505050505050506060606060606060606060606060606030303030303",
    "030303030303030303030808080808080808080808080808080800000000000000017769726566697874",
    "757265000000000000000000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000606162636465666768696a6b6c6d6e6f",
);
const TYPE1:&str="4e544c4d535350000100000015820822000000000000000000000000000000000a00654a0000000f";
const TYPE2:&str=concat!(
    "4e544c4d5353500002000000060006003800000015828a22111213141516171800000000000000002e00",
    "2e003e0000000a00654a0000000f42004f0058000100060042004f005800020010005a00580053004c00",
    "4f00430041004c00070008004433221100bcda0100000000",
);
const TYPE3:&str=concat!(
    "4e544c4d53535000030000000000000000000000a600a6005800000010001000fe000000160016000e01",
    "00000e000e0024010000000000000000000015828a220a00654a0000000fe47e4480064bf3197854737e",
    "0ee9cae2117cef32015791cce9baa2d940e1e4fd01010000000000004433221100bcda01414243444546",
    "4748000000000100060042004f005800020010005a00580053004c004f00430041004c00070008004433",
    "221100bcda0106000400020000000900280063006900660073002f006600690078007400750072006500",
    "2e0069006e00760061006c00690064000a00100000000000000000000000000000000000000000000000",
    "00005a00580053004c004f00430041004c00570069007200650046006900780074007500720065004600",
    "490058005400550052004500",
);
const SESSION:&str="a698b415669d1adf02c4866ac90507f6";
const TIME:u64=0x01da_bc00_1122_3344;

fn hex(s:&str)->Vec<u8>{s.as_bytes().chunks_exact(2).map(|x|
    u8::from_str_radix(std::str::from_utf8(x).unwrap(),16).unwrap()).collect()}
fn filled()->Zeroizing<[u8;RECORD_BYTES]>{
    // This reference hex is wholly public. Production never allocates this
    // Vec or accepts any unqualified filename through the record codec.
    Zeroizing::new(hex(RECORD).try_into().unwrap())
}
fn ascii(text:&str)->[u8;64] {
    let mut out=[0;64];out[..text.len()].copy_from_slice(text.as_bytes());out
}
fn expected()->Coordinates{
    Coordinates{pool:[1;16],init:[2;16],setup:[3;16],instance:[4;16],binding:[5;16],
        account:[6;16],credential:[3;16],record:[8;16],generation:NonZeroU64::MIN,
        login_len:11,login:ascii("wirefixture")}
}
fn utf(s:&str)->Box<[u8]>{s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>().into_boxed_slice()}

fn proof_from_fixture(decoded:DecodedRecord,want:&Coordinates)->Result<MechanismProof>{
    // Local synthetic oracle, NOT a production qualification check.
    if decoded.coordinates()!=want {return Err(Rejected);}
    let DecodedRecord{fields,key}=decoded;
    let who=std::str::from_utf8(&fields.login[..usize::from(fields.login_len)]).map_err(|_|Rejected)?;
    let revision=Arc::new(ProtectedRevision{user:utf(who),domain:utf(DOMAIN),spn:utf("cifs/fixture.invalid"),
        epoch:fields.generation,response_key:key,source:RevisionSource::Fixture});
    let (one,two,three)=(hex(TYPE1),hex(TYPE2),hex(TYPE3));
    let sid=NonZeroU64::new(42).unwrap();
    let issued=Issued{start:frame::seen(&one),revision,assigned:sid,expected_type2:Saved::from_wire(&two).unwrap(),
        expires:Instant::now()+Duration::from_secs(30)};
    let next=issued.sent(&frame::smb(&two,true,1,sid.get(),0))?;
    let original=frame::framed(&frame::smb(&three,false,2,sid.get(),0));
    next.received(&frame::compound(&original),TIME)
}

#[test]
fn decoded_fixed_record_can_feed_only_the_existing_strict_fixture_verifier() {
    let want=expected();let decoded=DecodedRecord::decode(filled()).unwrap();
    assert!(decoded.coordinates()==&want);
    let proven=proof_from_fixture(decoded,&want).unwrap();
    assert_eq!(&proven.exported[..],hex(SESSION).as_slice());

    // A positive parse and the same public coordinates are not authentication.
    let mut wrong=filled();wrong[224]^=1;
    assert!(proof_from_fixture(DecodedRecord::decode(wrong).unwrap(),&want).is_err());
}

#[test]
fn fields_are_checked_exactly_and_carry_no_authority_or_default_account() {
    let mut v1=filled();v1[14]=b'1'; // exact retired magic, not a second decoder
    assert!(DecodedRecord::decode(v1).is_err());
    for at in [0,15,16,18,23,160,171,223] {
        let mut b=filled();b[at]^=0xff;assert!(DecodedRecord::decode(b).is_err());
    }
    for n in [0,65,255] {let mut b=filled();b[17]=n;assert!(DecodedRecord::decode(b).is_err());}
    for name in ["guest","anonymous","root","nobody","WireFixture",".wirefixture","wire fixture"] {
        let mut b=filled();b[17]=name.len() as u8;b[160..224].copy_from_slice(&ascii(name));
        assert!(DecodedRecord::decode(b).is_err());
    }
    for at in (24..152).step_by(16) {
        let mut b=filled();b[at..at+16].fill(0);assert!(DecodedRecord::decode(b).is_err());
    }
    for g in [0,i64::MAX as u64+1,u64::MAX] {
        let mut b=filled();b[152..160].copy_from_slice(&g.to_be_bytes());assert!(DecodedRecord::decode(b).is_err());
    }
    let mut b=filled();b[40..56].fill(3);assert!(DecodedRecord::decode(b).is_err()); // INIT != Setup
    for op in [2,7] {
        let mut b=filled();b[120..136].fill(op);assert!(DecodedRecord::decode(b).is_err()); // generation 1
    }
    for op in [2,3] {
        let mut b=filled();b[152..160].copy_from_slice(&2u64.to_be_bytes());b[120..136].fill(op);
        assert!(DecodedRecord::decode(b).is_err()); // later generation cannot repeat INIT/Setup
    }
    let mut b=filled();b[224..240].fill(0);assert!(DecodedRecord::decode(b).is_err());
    let b=hex(RECORD); // direct snapshots must be exactly sized before calling the codec
    assert!(<&[u8;RECORD_BYTES]>::try_from(&b[..239]).is_err());
    let mut b=b;b.push(0);assert!(<&[u8;RECORD_BYTES]>::try_from(b.as_slice()).is_err());
}

#[test]
fn different_but_well_formed_coordinates_never_equal_the_same_original_record() {
    let want=expected();
    for at in [24,40,56,72,88,104,120,136,160] {
        let mut b=filled();
        match at {
            56|120=>{b[56]=0x71;b[120]=0x71;}, // a different valid original Setup and gen-1 op
            160=>b[at]=b'b',
            _=>b[at]=0x71,
        }
        let record=DecodedRecord::decode(b).unwrap();
        assert!(record.coordinates()!=&want);assert!(proof_from_fixture(record,&want).is_err());
    }
    let mut b=filled();b[120..136].fill(9);b[152..160].copy_from_slice(&i64::MAX.to_be_bytes());
    let later=DecodedRecord::decode(b).unwrap();
    assert_eq!(later.coordinates().generation.get(),i64::MAX as u64);
    assert!(later.coordinates()!=&want);
    assert!(proof_from_fixture(later,&want).is_err());
}

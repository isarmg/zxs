//! Fixed public bytes only, no account, vault or inherited descriptor.
use super::*;

const WIRE_HEX:&str=concat!(
    "5a58532d525553542d534d422d454e562d320021212121212121212121212121212121212121212121212121",
    "212121212121212222222222222222222222222222222222222222222222222222222222222222000003e800",
    "0003e800000000000102030000000002030405000b2f66697874757265732f73008c5a58532d525553542d53",
    "4d422d4932001111111111111111111111111111111112121212121212121212121212121212161616161616",
    "1616161616161616161617171717171717171717171717171717131313131313131313131313131313130b66",
    "6978747572652e6c616e0133333333333333333333333333333333020004446f63730006e59b9ee694b601aa",
    "aa0cab0000000919191919191919191919191919191919131313131313131313131313131313131a1a1a1a1a",
    "1a1a1a1a1a1a1a1a1a1a1a00000000000004040000000000000505ffffffffffffffff000003e60000000000",
    "0000010000007b010500000000000515000000292929292929292929292929e8030000000000000000000105",
    "66696c65733d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3de0f2cdbc7c70f7",
    "598ecedc376bb0625a5ef2794eef793d5c281a4e000df89ebb0000000000000021000000000000002c623e93",
    "83933d2eb6e177c67a1c60b6f9531ed52972cb0aeedb75ca973459a3ef0102030405060708",
);
const SHA:&str="25bc05cf564dc1496f02da0055a24da482a1e86f94a9fdb6a4bc401c297a72af";

fn bytes(s:&str)->Vec<u8>{
    assert_eq!(s.len()%2,0);
    (0..s.len()).step_by(2).map(|i|u8::from_str_radix(&s[i..i+2],16).unwrap()).collect()
}
fn request(v:&[u8])->Vec<u8>{
    let mut p=START_MAGIC.to_vec();p.extend_from_slice(&[0x21;32]);
    p.extend_from_slice(&crate::hash::digest(v).unwrap());p
}
#[test]
fn fixed_public_start_and_envelope(){
    let v=bytes(WIRE_HEX);assert_eq!(v.len(),521);
    let q=request(&v);assert_eq!(&q[48..],bytes(SHA).as_slice());
    let start=Start::decode(&q).unwrap();
    let e=start.verify(&v).unwrap();
    let ready=start.prepared_frame();
    assert_eq!(&ready[..16],b"ZXS-RUST/PREP02\0");assert_eq!(&ready[16..],&q[16..]);
    assert!(Start::decode(&ready).is_err());
    assert_eq!(e.release_sha,[0x22;32]);
    assert_eq!((e.uid,e.gid,e.sdev,e.sino),(1000,1000,0x10203,0x02030405));
    assert_eq!(e.system,"/fixtures/s");assert_eq!(e.plan.len(),140);assert_eq!(e.listener_cookie,0xaaaa0cab00000009);
    assert_eq!((e.account,e.credential,e.record),([0x19;16],[0x13;16],[0x1a;16]));
    assert_eq!((e.vault.dev,e.vault.ino,e.vault.ctime_s,e.vault.ctime_ns,e.vault.mtime_s,e.vault.mtime_ns),
        (0x404,0x505,-1,998,1,123));
    assert_eq!(e.generation,1);assert_eq!(e.login,"files");
}
#[test]
fn cannot_replace_a_well_formed_envelope_or_inherited_nonce(){
    let v=bytes(WIRE_HEX);let q=request(&v);
    assert!(Start::decode(&q[..START_BYTES-1]).is_err());
    let mut long=q.clone();long.push(0);assert!(Start::decode(&long).is_err());
    let mut changed=v.clone();changed[51]^=1;
    assert!(Start::decode(&q).unwrap().verify(&changed).is_err());
    assert!(Start::decode(&request(&changed)).unwrap().verify(&changed).is_err());
    let mut q=q;q[16]^=1;assert!(Start::decode(&q).unwrap().verify(&v).is_err());
}
#[test]
fn sizes_and_coordinates_still_need_validation_with_a_matching_sha(){
    let raw=bytes(WIRE_HEX);
    for cut in [0,18,108,120,261,262,270,318,334,346,358,359,386,394,400,401,432,465,480,520]{
        let v=&raw[..cut];assert!(Start::decode(&request(v)).unwrap().verify(v).is_err());
    }
    for (at,width) in [(19,32),(51,32),(83,4),(87,4),(99,8),(263,8),(271,16),(287,16),(303,16),(327,8),(387,8),(401,32),(433,32),(473,8),(481,32),(513,8)]{
        let mut v=raw.clone();v[at..at+width].fill(0);
        assert!(Start::decode(&request(&v)).unwrap().verify(&v).is_err());
    }
    let mut long=raw.clone();long.push(0);assert!(Start::decode(&request(&long)).unwrap().verify(&long).is_err());
    let mut reserved=raw.clone();reserved[83..87].copy_from_slice(&65534u32.to_be_bytes());
    assert!(Start::decode(&request(&reserved)).unwrap().verify(&reserved).is_err());
    let mut none=raw.clone();none[262]=0;
    assert!(Start::decode(&request(&none)).unwrap().verify(&none).is_err());
    let mut extra=raw.clone();extra[262]=2;extra.splice(271..271,7u64.to_be_bytes());
    assert!(Start::decode(&request(&extra)).unwrap().verify(&extra).is_err());
    let mut v=raw.clone();v[343..347].copy_from_slice(&1_000_000_000u32.to_be_bytes());
    assert!(Start::decode(&request(&v)).unwrap().verify(&v).is_err());
    let mut v=raw.clone();v[387]=0x80;assert!(Start::decode(&request(&v)).unwrap().verify(&v).is_err());
    let mut v=raw;v[110]=b'/';assert!(Start::decode(&request(&v)).unwrap().verify(&v).is_err());
}

//! Fixed public bytes only, no account, vault or inherited descriptor.
use super::*;

const WIRE_HEX:&str=concat!(
    "5a58532d525553542d534d422d454e562d32002121212121212121212121212121212121212121212121212121",
    "2121212121212222222222222222222222222222222222222222222222222222222222222222000003e8000003",
    "e800000000000102030000000002030405000b2f66697874757265732f7300935a58532d525553542d534d422d",
    "493100111111111111111111111111111111111212121212121212121212121212121216161616161616161616",
    "16161616161617171717171717171717171717171717131313131313131313131313131313130b666978747572",
    "652e6c616e010418c00002110133333333333333333333333333333333020004446f63730006e59b9ee694b601",
    "aaaa0cab0000000919191919191919191919191919191919131313131313131313131313131313131a1a1a1a1a",
    "1a1a1a1a1a1a1a1a1a1a1a00000000000004040000000000000505ffffffffffffffff000003e6000000000000",
    "00010000007b010500000000000515000000292929292929292929292929e80300000000000000000001056669",
    "6c65733d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d0b0ee614d62726aa95da",
    "ec83c0e5bc1a5f8b4ec23ef3f2c593250d380898504c0000000000000021000000000000002c623e9383933d2e",
    "b6e177c67a1c60b6f9531ed52972cb0aeedb75ca973459a3ef0102030405060708",
);
const SHA:&str="d556a3484949a5562dabf91cf38b1c848617a13a9a6e4ea8f43fb588d11251a2";

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
    let v=bytes(WIRE_HEX);assert_eq!(v.len(),528);
    let q=request(&v);assert_eq!(&q[48..],bytes(SHA).as_slice());
    let start=Start::decode(&q).unwrap();
    let e=start.verify(&v).unwrap();
    let ready=start.prepared_frame();
    assert_eq!(&ready[..16],b"ZXS-RUST/PREP02\0");assert_eq!(&ready[16..],&q[16..]);
    assert!(Start::decode(&ready).is_err());
    assert_eq!(e.release_sha,[0x22;32]);
    assert_eq!((e.uid,e.gid,e.sdev,e.sino),(1000,1000,0x10203,0x02030405));
    assert_eq!(e.system,"/fixtures/s");assert_eq!(e.plan.len(),147);assert_eq!(e.cookies(),&[0xaaaa0cab00000009]);
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
    for cut in [0,18,108,120,268,269,277,325,341,353,365,366,393,401,407,408,439,472,487,527]{
        let v=&raw[..cut];assert!(Start::decode(&request(v)).unwrap().verify(v).is_err());
    }
    for (at,width) in [(19,32),(51,32),(83,4),(87,4),(99,8),(270,8),(278,16),(294,16),(310,16),(334,8),(394,8),(408,32),(440,32),(480,8),(488,32),(520,8)]{
        let mut v=raw.clone();v[at..at+width].fill(0);
        assert!(Start::decode(&request(&v)).unwrap().verify(&v).is_err());
    }
    let mut long=raw.clone();long.push(0);assert!(Start::decode(&request(&long)).unwrap().verify(&long).is_err());
    let mut reserved=raw.clone();reserved[83..87].copy_from_slice(&65534u32.to_be_bytes());
    assert!(Start::decode(&request(&reserved)).unwrap().verify(&reserved).is_err());
    let mut dup=raw.clone();dup[269]=2;dup.splice(278..278,raw[270..278].iter().copied());
    assert!(Start::decode(&request(&dup)).unwrap().verify(&dup).is_err());
    let mut v=raw.clone();v[350..354].copy_from_slice(&1_000_000_000u32.to_be_bytes());
    assert!(Start::decode(&request(&v)).unwrap().verify(&v).is_err());
    let mut v=raw.clone();v[394]=0x80;assert!(Start::decode(&request(&v)).unwrap().verify(&v).is_err());
    let mut v=raw;v[110]=b'/';assert!(Start::decode(&request(&v)).unwrap().verify(&v).is_err());
}

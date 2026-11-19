use super::*;
use std::{fs,io::{Seek,SeekFrom,Write}};

const VECTORS:&str=include_str!("../vectors/portable-volume-claim-v1.txt");
fn field(name:&str)->Vec<u8>{
    let hex=VECTORS.lines().filter_map(|s|s.split_once('=')).find(|(k,_)|*k==name).unwrap().1;
    assert_eq!(hex.len()%2,0);
    (0..hex.len()/2).map(|i|u8::from_str_radix(&hex[2*i..2*i+2],16).unwrap()).collect()
}
fn seq(first:u8)->[u8;16]{std::array::from_fn(|i|first+i as u8)}

#[test]
fn four_external_declarations_select_only_their_exact_logical_claim_rows(){
    let init=crate::hash::digest(&field("source_input_init")).unwrap();
    let add=crate::hash::digest(&field("source_input_add")).unwrap();
    assert_eq!(init.as_slice(),field("source_digest_init"));assert_eq!(add.as_slice(),field("source_digest_add"));
    let pool=PoolId(seq(0x01));
    for (key,role,volume,op,digest) in [("initial_s",Role::System,0x21,0x81,init),("initial_r",Role::Recycle,0x31,0x81,init),
        ("initial_f",Role::File,0x41,0x81,init),("later_f",Role::File,0x51,0x91,add)]{
        assert_eq!(Declaration::decode(&field(key)).unwrap(),Declaration{role,pool,volume:VolumeId(seq(volume)),
            claimed_by:OperationId(seq(op)),source_digest:digest});
    }
    // Later F did not inherit the original init claim. No latest frontier,
    // checked payload hash or self-asserted role stands in for this record.
    let f=Declaration::decode(&field("initial_f")).unwrap();let mut swapped=field("initial_f");
    for offset in [22,23,39,55,71,102]{
        swapped[offset]^=if offset==22{2}else{1};
        assert_ne!(Declaration::decode(&swapped).unwrap(),f);
        swapped=field("initial_f");
    }
    assert_ne!(Declaration::decode(&field("later_f")).unwrap(),f);
}

#[test]
fn exact_structure_rejects_partial_unknown_or_unissued_ids_but_cannot_detect_a_clone(){
    let original=field("initial_r");
    for count in 0..original.len(){assert!(Declaration::decode(&original[..count]).is_err());}
    let mut tail=original.clone();tail.push(0);assert!(Declaration::decode(&tail).is_err());
    for off in [0,21,22]{let mut invalid=original.clone();invalid[off]=b'X';assert!(Declaration::decode(&invalid).is_err());}
    for off in [23,39,55]{let mut invalid=original.clone();invalid[off..off+16].fill(0);assert!(Declaration::decode(&invalid).is_err());}
    let mut unknown=original.clone();unknown[71..].fill(0);
    // No digest default. Structural parsing may report any 32 declared
    // bytes, including zero; only an independently confirmed source can compare.
    assert_ne!(Declaration::decode(&unknown).unwrap(),Declaration::decode(&original).unwrap());
    assert_eq!(Declaration::decode(&original.clone()).unwrap(),Declaration::decode(&original).unwrap());
}

#[test]
fn a_held_regular_yields_only_a_declaration_without_changing_its_seek_position(){
    let tmp=crate::test_support::PrivateTemp::under(&std::env::temp_dir());
    let root_name=ROOT_NAME.to_str().unwrap();
    assert_eq!(root_name,".zxs-new-ffffffffffffffffffffffffffffffff-0");assert_eq!(root_name.len(),43);
    // The plain business directory "VOLUME" remains unrelated.
    tmp.child("VOLUME");let p=tmp.0.join(root_name);
    let mut f=fs::OpenOptions::new().create_new(true).read(true).write(true).open(&p).unwrap();
    let bytes=field("initial_s");f.write_all(&bytes).unwrap();f.sync_all().unwrap();f.seek(SeekFrom::Start(19)).unwrap();
    let got=Declaration::read(&f).unwrap();assert_eq!(got,Declaration::decode(&bytes).unwrap());assert_eq!(f.stream_position().unwrap(),19);
    fs::write(tmp.0.join("untrusted-copy"),&bytes).unwrap();
    assert_eq!(Declaration::read(&File::open(tmp.0.join("untrusted-copy")).unwrap()).unwrap(),got);
    assert!(Declaration::read(&File::open(tmp.0.join("VOLUME")).unwrap()).is_err());
    f.set_len(102).unwrap();assert!(Declaration::read(&f).is_err());
    f.set_len(104).unwrap();assert!(Declaration::read(&f).is_err());
}

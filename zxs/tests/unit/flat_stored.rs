use super::*;
use std::{fs,io::{Seek,SeekFrom,Write}};

const HEX:&str="5a58532d504f525441424c452d312f52414e47452d494e44455800010101010101010101010101010101010202020202020202020202020202020203030303030303030303030303030303040404040404040404040404040404040000000000000007000000000000000600000002000000030505050505050505050505050505050500000104000000000000000806060606060606060606060606060606000000010000000000000003000000000000000200000001000000000000000200000000000000040000000000000000000000000000000000000006000000020000000000000001";
const SHA:&str="ef7ad059d1e6e875c2ef1292c8731d1096a77bd5c5b4e8bcbbe55e130b86d7b1";
fn hex(raw:&str)->Vec<u8>{(0..raw.len()/2).map(|i|u8::from_str_radix(&raw[i*2..i*2+2],16).unwrap()).collect()}
fn key()->VersionKey{VersionKey{pool:PoolId([1;16]),file:FileId([2;16]),node:NodeId([3;16]),version:VersionId([4;16]),representation:7}}
fn fixture()->(Descriptor,Material,Material,Index){
    let key=key();
    let m=Material::declared(key.file,OperationId([5;16]),260,8).unwrap();
    let n=Material::declared(key.file,OperationId([6;16]),1,3).unwrap();
    let mut b=Build::new(key.file).unwrap();
    b.add(Leaf::Data(m),2,2).unwrap();b.add(Leaf::Zero,0,2).unwrap();b.add(Leaf::Data(n),1,2).unwrap();
    let d=Descriptor::declared(key,6,231,hex(SHA).try_into().unwrap()).unwrap();
    (d,m,n,b.finish(6).unwrap())
}

#[test]
fn independently_spelled_wire_is_canonical_and_binds_the_exact_chosen_representation(){
    let (d,_,_,i)=fixture();let raw=hex(HEX);
    let (actual,bytes)=encode(d.key,&i).unwrap();assert_eq!(actual.index_sha().as_slice(),hex(SHA));
    assert_eq!((actual,bytes),(d,raw.clone()));
    assert_eq!(d.decode(&raw).unwrap(),i);
    for k in [VersionKey{pool:PoolId([8;16]),..d.key},VersionKey{file:FileId([9;16]),..d.key},
        VersionKey{node:NodeId([10;16]),..d.key},VersionKey{version:VersionId([11;16]),..d.key},
        VersionKey{representation:8,..d.key}] {assert!((Descriptor{key:k,..d}).decode(&raw).is_err());}
    let mut altered=raw.clone();altered[FIXED]^=0x40;
    assert!(d.decode(&altered).is_err()); // original SYSTEM digest, not caller's new hash
    assert!(d.decode(&raw[..raw.len()-1]).is_err());
    assert!(encode(VersionKey{file:FileId([9;16]),..d.key},&i).is_err());
    let empty=Index::full(Material::declared(d.key.file,OperationId([7;16]),1000,0).unwrap()).unwrap();
    let(er,eb)=encode(d.key,&empty).unwrap();assert_eq!(eb.len(),FIXED);assert_eq!(er.decode(&eb).unwrap(),empty);
}

#[test]
fn even_rehashed_noncanonical_orders_aliases_offsets_and_unused_materials_are_rejected(){
    let (original,_,_,_)=fixture();let rows=FIXED+2*MATERIAL;
    fn bad(d:Descriptor,change:impl FnOnce(&mut Vec<u8>)){
        let mut b=hex(HEX);change(&mut b);
        let candidate=Descriptor{digest:IndexDigest(hash::digest(&b).unwrap()),index_bytes:b.len() as u64,..d};
        assert!(candidate.decode(&b).is_err()); // fabricated descriptor still cannot legitimize malformed bytes
    }
    bad(original,|b|b[rows+8..rows+12].copy_from_slice(&2u32.to_be_bytes())); // out of first-use order
    bad(original,|b|b[rows+ROW+12..rows+2*ROW].copy_from_slice(&1u64.to_be_bytes())); // zero with an offset
    bad(original,|b|b[rows+2*ROW..rows+2*ROW+8].copy_from_slice(&5u64.to_be_bytes())); // not full logical coverage
    bad(original,|b|b[rows..rows+8].fill(0)); // empty row
    bad(original,|b|b[rows+12..rows+ROW].copy_from_slice(&u64::MAX.to_be_bytes()));
    bad(original,|b|{b[rows+ROW+8..rows+ROW+12].copy_from_slice(&1u32.to_be_bytes());
        b[rows+ROW+12..rows+2*ROW].copy_from_slice(&4u64.to_be_bytes());}); // must have merged first two
    bad(original,|b|b[rows+2*ROW+8..rows+2*ROW+12].copy_from_slice(&1u32.to_be_bytes())); // second descriptor never used
    bad(original,|b|b.copy_within(FIXED..FIXED+MATERIAL,FIXED+MATERIAL)); // duplicate original identity
    bad(original,|b|b.copy_within(FIXED..FIXED+20,FIXED+MATERIAL)); // same identity, two extents
    bad(original,|b|b[FIXED-8..FIXED-4].copy_from_slice(&u32::MAX.to_be_bytes()));
    bad(original,|b|b.push(0));
}

#[test]
fn reopens_one_plain_table_without_ancestor_files_or_implicit_custody_from_a_clone(){
    let(d,m,n,expected)=fixture();let tmp=crate::test_support::PrivateTemp::under(&std::env::temp_dir());
    let store=|name:&str,b:&[u8]|{
        let path=tmp.0.join(name);let mut f=fs::OpenOptions::new().create_new(true).write(true).open(&path).unwrap();
        f.write_all(b).unwrap();f.sync_all().unwrap();drop(f);path
    };
    let a=File::open(store("original-dense-A",b"abcdefgh")).unwrap();
    let b=File::open(store("original-dense-B",b"xyz")).unwrap();
    let path=store("selected-D-index",&hex(HEX));File::open(&tmp.0).unwrap().sync_all().unwrap();
    let mut index_file=File::open(&path).unwrap();index_file.seek(SeekFrom::Start(20)).unwrap();
    let loaded=d.read(&index_file).unwrap();assert_eq!(loaded,expected);assert_eq!(index_file.stream_position().unwrap(),20);
    let mut out=Vec::new();loaded.visit(1,4,|source,off,nbytes|{
        let target=match source{Leaf::Data(x) if x==m=>&a,Leaf::Data(x) if x==n=>&b,
            Leaf::Zero=>{out.resize(out.len()+nbytes as usize,0);return Ok(())},_=>panic!("unissued terminal")};
        let mut picked=vec![0;nbytes as usize];target.read_exact_at(&mut picked,off)?;out.extend_from_slice(&picked);Ok(())
    }).unwrap();assert_eq!(out,b"d\0\0y");
    // A copied table produces the same logical plan, not a new right to
    // adopt/delete that inode, select a NODE, or bypass payload proof.
    let copy=store("copy",&hex(HEX));assert_eq!(d.read(&File::open(copy).unwrap()).unwrap(),expected);
    let mut changed=hex(HEX);changed[FIXED]^=0x40;
    fs::write(&path,&changed).unwrap();assert!(d.read(&index_file).is_err());
    assert!(d.read(&File::open(&tmp.0).unwrap()).is_err());
}

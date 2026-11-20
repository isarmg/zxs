//! Four direct work maps on one actual append file. The fixture exercises
//! physical index selection and terminal coverage, not SQL publication.
use super::*;
use crate::test_support::PrivateTemp;
use std::{fs::OpenOptions,io::Write};

const POOL:PoolId=PoolId([1;16]);
const FILE:FileId=FileId([2;16]);
fn id(b:u8)->OperationId{OperationId([b;16])}
fn key(round:u8,save:u8,revision:u64)->WorkKey{WorkKey{pool:POOL,file:FILE,round:id(round),save:id(save),revision}}
fn m(save:u8,sz:u64)->Material{Material::declared(FILE,id(save),0,sz).unwrap()}
fn bytes(i:&Index)->Vec<u8>{
    let mut b=Vec::new();
    i.visit(0,i.len(),|leaf,at,n|{
        match leaf{
            Leaf::Zero=>b.resize(b.len()+n as usize,0),
            Leaf::Data(m)=>{
                let actual:&[u8]=match (m.issuer.0[0],m.size){(3,5)=>b"ABCDE",(4,2)=>b"xy",(6,1)=>b"z",(7,1)=>b"W",_=>return Err(invalid("unknown synthetic material"))};
                b.extend_from_slice(&actual[at as usize..(at+n) as usize]);
            }
        }
        Ok(())
    }).unwrap();b
}

#[test]
fn cold_direct_work_maps_keep_each_frozen_view_without_a_full_or_parent_table(){
    let temp=PrivateTemp::under(&std::env::temp_dir());let p=temp.0.join("one-check-table");
    let mut checks=OpenOptions::new().create_new(true).read(true).write(true).open(&p).unwrap();
    checks.write_all(b"prefix").unwrap();
    let first=Index::full(m(3,5)).unwrap();
    let next=first.edit(1,Some((m(4,2),0,2)),5).unwrap();
    let second=next.edit(0,None,4).unwrap().edit(0,None,6).unwrap();
    let third=second.edit(4,Some((m(6,1),0,1)),6).unwrap();
    let fourth=third.edit(2,Some((m(7,1),0,1)),6).unwrap();
    let chosen=[(key(3,3,1),&first,b"ABCDE".as_slice()),(key(4,5,2),&second,b"AxyD\0\0"),
        (key(6,6,1),&third,b"AxyDz\0"),(key(7,7,1),&fourth,b"AxWDz\0")];
    let mut written=Vec::new();let mut end=6;
    for (k,original,expected) in chosen{
        let (d,wire)=encode_work(k,original).unwrap();checks.write_all(&wire).unwrap();
        assert_eq!(d.logical_bytes(),expected.len() as u64);
        written.push((WorkDescriptor::declared(k,d.logical_bytes(),d.index_bytes(),d.index_sha()).unwrap(),end,expected));
        end+=d.index_bytes();
    }
    checks.sync_all().unwrap();drop(checks);
    let cold=HeldBytes::separate_format_fixture(File::open(&p).unwrap()).unwrap();
    for &(original,at,expected) in &written{
        let selected=original.read_selected(&cold,at,at+original.index_bytes()).unwrap();
        assert_eq!(bytes(&selected),expected);
        assert!(original.read_selected(&cold,at,at+original.index_bytes()-1).is_err());
    }
    let (d,at,_)=written[3];
    assert_eq!(d.key(),key(7,7,1));
    let wrong=WorkDescriptor::declared(key(8,8,1),d.logical_bytes(),d.index_bytes(),d.index_sha()).unwrap();
    assert!(wrong.read_selected(&cold,at,end).is_err());
    let formal=VersionKey{pool:POOL,file:FILE,node:NodeId(id(7).0),version:VersionId(id(7).0),representation:1};
    let (_,wire)=encode_work(d.key(),&fourth).unwrap();
    assert!(Descriptor::declared(formal,d.logical_bytes(),d.index_bytes(),d.index_sha()).unwrap().decode(&wire).is_err());
    assert!(encode_work(key(7,7,2),&fourth).is_err());
    let extra=Index::full(Material::declared(FILE,id(8),1,1).unwrap()).unwrap();
    assert!(encode_work(key(8,8,1),&extra).is_err());
}

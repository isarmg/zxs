//! Nonzero locations in the same physical data and checksum files. These
//! are source-level observations, with deliberately synthetic issuance.
use super::*;
use crate::{FileId,OperationId,test_support::PrivateTemp};
use std::{fs,io::Write,path::Path};

fn new(p:&Path)->File{
    fs::OpenOptions::new().read(true).write(true).create_new(true).open(p).unwrap()
}
fn issuer(tmp:&PrivateTemp,name:&str,identity:u8,data:&[u8])->(Ref,Vec<u8>){
    let m=Material::declared(FileId([2;16]),OperationId([identity;16]),0,data.len() as u64).unwrap();
    let mut f=new(&tmp.0.join(format!("{name}.input")));f.write_all(data).unwrap();
    let t=tmp.0.join(format!("{name}.table"));let out=new(&t);
    let declared=write_candidate(PoolId([1;16]),m,&f,&out).unwrap();
    (declared,fs::read(t).unwrap())
}

#[test]
fn two_real_appended_windows_share_one_pair_without_promoting_a_physical_tail(){
    let tmp=PrivateTemp::under(&std::env::temp_dir());
    let (before,b)=issuer(&tmp,"old",3,b"A");
    let (after,a)=issuer(&tmp,"new",4,b"bc");
    let raw_path=tmp.0.join("round");let checks_path=tmp.0.join("checks");
    let raw=new(&raw_path);let checks=new(&checks_path);
    let start=Ends{raw:(1u64<<33)+7,checks:11};
    let old_end=Ends{raw:start.raw+1,checks:start.checks+b.len() as u64};
    let all=Ends{raw:old_end.raw+2,checks:old_end.checks+a.len() as u64};
    raw.write_all_at(b"A",start.raw).unwrap();checks.write_all_at(&b,start.checks).unwrap();
    raw.sync_all().unwrap();checks.sync_all().unwrap();
    let held_raw=HeldBytes::separate_format_fixture(File::open(&raw_path).unwrap()).unwrap().checked_prefix().unwrap();
    let held_checks=HeldBytes::separate_format_fixture(File::open(&checks_path).unwrap()).unwrap().checked_prefix().unwrap();
    let old=Segment::open(before,&held_raw,&held_checks,start,old_end).unwrap();

    raw.write_all_at(b"bc",old_end.raw).unwrap();checks.write_all_at(&a,old_end.checks).unwrap();
    raw.sync_all().unwrap();checks.sync_all().unwrap();
    let cold_raw=HeldBytes::separate_format_fixture(File::open(&raw_path).unwrap()).unwrap();
    let cold_checks=HeldBytes::separate_format_fixture(File::open(&checks_path).unwrap()).unwrap();
    assert!(Segment::open(after,&cold_raw,&cold_checks,old_end,old_end).is_err());
    assert!(Segment::open(after,&cold_raw,&cold_checks,old_end,Ends{raw:all.raw,checks:old_end.checks}).is_err());
    assert!(Segment::open(after,&cold_raw,&cold_checks,old_end,Ends{raw:old_end.raw,checks:all.checks}).is_err());
    let selected=Segment::open(after,&cold_raw,&cold_checks,old_end,all).unwrap();
    let mut out=Vec::new();let mut buffers=Buffers::new(32,2).unwrap();
    old.range(&held_raw,&held_checks,0,1,&mut buffers,&mut out).unwrap();
    assert!(old.range(&held_raw,&held_checks,1,1,&mut buffers,&mut out).is_err());
    assert!(old.range(&cold_raw,&cold_checks,0,1,&mut buffers,&mut out).is_err());
    selected.range(&cold_raw,&cold_checks,0,2,&mut buffers,&mut out).unwrap();
    assert_eq!(out,b"Abc");
}

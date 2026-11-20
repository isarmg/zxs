//! The actual two-link DATA/checks pair is reopened for old, newest
//! confirmed and unconfirmed append cuts; no fake DB or network response.
use super::*;
use crate::{hash,test_support::PrivateTemp};
use std::{fs::{File,OpenOptions},os::unix::fs::{FileExt,OpenOptionsExt}};

const POOL:PoolId=PoolId([1;16]);
const FILE:FileId=FileId([2;16]);
const ROUND:OperationId=OperationId([3;16]);
const NEXT:OperationId=OperationId([4;16]);
const PENDING:OperationId=OperationId([5;16]);

fn open(p:&std::path::Path)->File {
    OpenOptions::new().read(true).write(true).create(true).mode(0o600).open(p).unwrap()
}
fn add(dir:&HeldDir,raw:&File,checks:&File,round:OperationId,id:OperationId,rev:u64,from:End,at:u64,
       data:&[u8],logical:u64,prior:Option<&Index>)->(Record,Index){
    let key=Key{pool:POOL,file:FILE,round,save:id,revision:rev};
    let material=Material::declared(FILE,id,0,data.len() as u64).unwrap();
    let input=open(&dir.path.join("incoming-test"));input.set_len(0).unwrap();input.write_all_at(data,0).unwrap();input.sync_all().unwrap();
    let rr=super::super::blocks::walk(&input,POOL,material,
        |p,b|raw.write_all_at(b,from.data+p),|p,b|checks.write_all_at(b,from.checks+p)).unwrap();
    let view=match prior {
        Some(p)=>p.edit(at,Some((material,0,data.len() as u64)),logical).unwrap(),
        None=>Index::full(material).unwrap(),
    };
    let (m,wire)=super::super::stored::encode_work(key.codec(),&view).unwrap();
    let pos=from.checks+rr.table_len().unwrap();checks.write_all_at(&wire,pos).unwrap();
    raw.sync_all().unwrap();checks.sync_all().unwrap();dir.file.sync_all().unwrap();
    let end=End{data:from.data+data.len() as u64,checks:pos+m.index_bytes()};
    (Record::edit(key,from,end,logical,hash::digest(data).unwrap(),rr.directory_sha(),
        Map{at:pos,bytes:m.index_bytes(),sha:m.index_sha()}).unwrap(),view)
}

#[test]
#[ignore="real private Linux ordinary files; source-only, no live data or SMB service"]
fn selected_prefix_does_not_select_or_read_a_physically_present_unconfirmed_tail() {
    let temp=PrivateTemp::under(&std::env::temp_dir());let dir=HeldDir::open(&temp.child("work")).unwrap();
    let n=names(ROUND);
    let path=|i:usize|dir.path.join(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(n[i].to_bytes()));
    let raw=open(&path(0));std::fs::hard_link(path(0),path(1)).unwrap();
    let checks=open(&path(2));std::fs::hard_link(path(2),path(3)).unwrap();
    let (first,idx)=add(&dir,&raw,&checks,ROUND,ROUND,1,End::ZERO,0,b"ABCDE",5,None);
    let early=Selected::open(&dir,first,first.end()).unwrap();
    let (next,idx)=add(&dir,&raw,&checks,ROUND,NEXT,2,first.end(),1,b"xy",5,Some(&idx));
    let (pending,_)=add(&dir,&raw,&checks,ROUND,PENDING,3,next.end(),2,b"z",5,Some(&idx));
    let old=Selected::open(&dir,next,pending.end()).unwrap();
    let r=|need:&[Need]|Ok(need.iter().map(|n|Origin{record:if n.issuer==ROUND{first}else{next},physical:pending.end()}).collect());
    let mut actual=Vec::new();early.range_with(&dir,0,5,r,||Ok(()),&mut actual).unwrap();assert_eq!(actual,b"ABCDE");
    actual.clear();old.range_with(&dir,0,5,r,||Ok(()),&mut actual).unwrap();assert_eq!(actual,b"AxyDE");
    assert!(Selected::open(&dir,next,next.end()).is_err()); // observed tail needs its own reservation
    assert!(Selected::open(&dir,pending,next.end()).is_err());
    assert!(old.pair.segment(pending,old.record.end()).is_err());
    assert!(old.range_with(&dir,0,5,|_|Ok(vec![Origin{record:pending,physical:pending.end()}]),||Ok(()),Vec::new()).is_err());

    let exact=Confirmation::read_back(&dir,pending,LiveStamp::read(&raw).unwrap(),LiveStamp::read(&checks).unwrap()).unwrap();
    let end=pending.end();raw.write_all_at(b"k",end.data).unwrap();raw.sync_all().unwrap();
    assert!(exact.require(&dir,pending).is_err()); // no unknown tail before confirming
    actual.clear();old.range_with(&dir,0,5,r,||Ok(()),&mut actual).unwrap();assert_eq!(actual,b"AxyDE");
}

#[test]
#[ignore="actual private two-round ordinary files, not executed in static source review"]
fn a_used_older_round_is_rechecked_after_final_sink_without_materializing_a_full_version(){
    use std::os::unix::ffi::OsStrExt;
    let temp=PrivateTemp::under(&std::env::temp_dir());let dir=HeldDir::open(&temp.child("old-source")).unwrap();
    let pair=|k|{
        let n=names(k);
        let leaf=|i:usize|dir.path.join(std::ffi::OsStr::from_bytes(n[i].to_bytes()));
        let data=open(&leaf(0));std::fs::hard_link(leaf(0),leaf(1)).unwrap();
        let checks=open(&leaf(2));std::fs::hard_link(leaf(2),leaf(3)).unwrap();
        (data,checks)
    };
    let (data,checks)=pair(ROUND);
    let (first,index)=add(&dir,&data,&checks,ROUND,ROUND,1,End::ZERO,0,b"ABCDE",5,None);
    let (more,table)=pair(NEXT);
    let (second,_)=add(&dir,&more,&table,NEXT,NEXT,1,End::ZERO,1,b"xy",5,Some(&index));
    let select=|needs:&[Need]|Ok(needs.iter().map(|n|if n.issuer==ROUND{
        Origin{record:first,physical:first.end()}
    }else{Origin{record:second,physical:second.end()}}).collect());
    let held=Selected::open(&dir,second,second.end()).unwrap();let mut output=Vec::new();
    held.range_with(&dir,0,5,select,||Ok(()),&mut output).unwrap();assert_eq!(output,b"AxyDE");
    struct Flush<'a>{output:&'a mut Vec<u8>,old:&'a File}
    impl Write for Flush<'_> {
        fn write(&mut self,b:&[u8])->io::Result<usize>{self.output.extend_from_slice(b);Ok(b.len())}
        fn flush(&mut self)->io::Result<()>{self.old.set_len(6)?;self.old.sync_all()}
    }
    output.clear();
    assert!(held.range_with(&dir,0,5,select,||Ok(()),Flush{output:&mut output,old:&data}).is_err());
    assert_eq!(output,b"AxyDE"); // bytes remain private in a real network adapter
}

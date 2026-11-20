//! These source fixtures use a real securely named two-link regular file.
//! They exercise local storage only: no SQL admission or SMB response is simulated.
use super::*;
use crate::{fs::SealedExtent,hash,test_support::PrivateTemp};
fn window<'d>(d:&'d HeldDir,at:u64,end:u64,first:bool)->io::Result<WorkPair<'d>>{
    WorkPair::issued_window(d,c"body".to_owned(),c"pin-body".to_owned(),at,end,first)
}
fn stamp(bytes:&[u8],at:u64)->SealedExtent{SealedExtent::original(at,bytes.len(),hash::digest(bytes).unwrap()).unwrap()}
fn read(v:&HeldBytes,d:&HeldDir,e:SealedExtent)->io::Result<Vec<u8>>{
    v.require_name(d,c"body")?;v.require_name(d,c"pin-body")?;
    if e.end()>v.stamp().len{return Err(invalid("outside selected revision"));}
    let mut scratch=vec![0;(e.end()-e.start()) as usize];
    e.checked(v.file(),&mut scratch)?;
    v.require_name(d,c"body")?;v.require_name(d,c"pin-body")?;Ok(scratch)
}
#[test]
fn original_short_block_and_appended_qualified_windows_share_one_inode_without_whole_file_locks(){
    let temp=PrivateTemp::under(&std::env::temp_dir());let d=HeldDir::open(&temp.child("file")).unwrap();d.exclusive().unwrap();
    let n=crate::DRAFT_CHUNK as u64;let mut first=window(&d,0,n+1,true).unwrap();
    let full=vec![0x11;n as usize];first.put(0,&full).unwrap();first.put(n,b"A").unwrap();first.finish().unwrap();drop(first);
    let first=WitnessRead::at(&d).unwrap().open(c"body",c"pin-body",n+1).unwrap();
    let strict=WitnessRead::at(&d).unwrap().open(c"body",c"pin-body",n+1).unwrap(); // headers/formal material remain exact
    let pin=first.checked_prefix().unwrap();let tail=stamp(b"A",n);
    let old_id=pin.stamp();assert_eq!(read(&pin,&d,tail).unwrap(),b"A");
    // A different physical fragment may overlay this same logical position.
    // The old view still authenticates exactly its original single-byte tail.
    let mut next=window(&d,n+1,n+4,false).unwrap();
    assert!(next.put(n,b"BBB").is_err());next.put(n+1,b"B").unwrap();drop(next); // SQL's exact reserved physical offset survives
    assert!(window(&d,n+3,n+4,false).is_err()); // never fill a hole/adopt observed EOF
    let mut retry=window(&d,n+1,n+4,false).unwrap();
    assert!(retry.put(n+1,b"XBB").is_err());retry.put(n+1,b"BBB").unwrap();retry.finish().unwrap();
    assert!(strict.unchanged().is_err());assert_eq!(read(&pin,&d,tail).unwrap(),b"A");
    assert!(read(&pin,&d,stamp(b"BBB",n+1)).is_err()); // an old handle cannot read at its new EOF
    assert!(retry.stamp().same(old_id));drop(retry);
    let later=WitnessRead::at(&d).unwrap().open(c"body",c"pin-body",n+4).unwrap().checked_prefix().unwrap();
    assert_eq!(read(&later,&d,stamp(b"BBB",n+1)).unwrap(),b"BBB");
    let mut last=window(&d,n+4,n+7,false).unwrap();last.put(n+4,b"CCC").unwrap();last.finish().unwrap();drop(last);
    assert_eq!(read(&pin,&d,tail).unwrap(),b"A");assert_eq!(read(&later,&d,stamp(b"BBB",n+1)).unwrap(),b"BBB");
    let newest=WitnessRead::at(&d).unwrap().open(c"body",c"pin-body",n+7).unwrap().checked_prefix().unwrap();
    assert_eq!(read(&newest,&d,stamp(b"CCC",n+4)).unwrap(),b"CCC");assert!(newest.stamp().same(old_id));
    assert_eq!(d.names().unwrap().len(),2); // two hard links, exactly ONE regular data inode
    // Damage within an old sealed extent is not excused by the same inode
    // or by a legitimate append. No output is delivered before its digest.
    let bad=std::fs::OpenOptions::new().write(true).open(d.path.join("body")).unwrap();bad.write_all_at(b"D",n).unwrap();
    assert!(read(&pin,&d,tail).is_err());assert_eq!(read(&later,&d,stamp(b"BBB",n+1)).unwrap(),b"BBB");
    std::fs::rename(d.path.join("pin-body"),d.path.join("away")).unwrap();assert!(read(&later,&d,stamp(b"BBB",n+1)).is_err());
    assert!(window(&d,n+7,n+8,false).is_err());assert!(!d.path.join("pin-body").exists());
}
#[test]
fn bounded_nonzero_seals_do_not_truncate_large_native_positions(){
    assert!(SealedExtent::original(0,0,[0;32]).is_err());
    assert!(SealedExtent::original(i64::MAX as u64,1,[0;32]).is_err());
    assert!(SealedExtent::original(0,crate::DRAFT_CHUNK+1,[0;32]).is_err());
    let temp=PrivateTemp::under(&std::env::temp_dir());let d=HeldDir::open(&temp.child("wide")).unwrap();
    let at=(1u64<<33)+7;
    // Sparse fixture represents an independently issued, already-present
    // prefix only. No hypothetical full 8GiB checksum/read/write occurs here.
    let mut initial=window(&d,0,0,true).unwrap();initial.put(0,b"").unwrap();initial.finish().unwrap();
    initial.file.set_len(at).unwrap();drop(initial);
    let mut next=window(&d,at,at+3,false).unwrap();next.put(at,b"new").unwrap();next.finish().unwrap();
    let leaf=WitnessRead::at(&d).unwrap().open(c"body",c"pin-body",at+3).unwrap().checked_prefix().unwrap();
    assert_eq!(read(&leaf,&d,stamp(b"new",at)).unwrap(),b"new");assert_eq!(next.stamp().len,at+3);
}

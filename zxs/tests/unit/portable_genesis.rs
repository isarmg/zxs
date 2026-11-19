use super::*;
use crate::{fs::HeldDir, test_support::PrivateTemp};
use std::{fs as osfs, io::Write, os::unix::fs::{MetadataExt, OpenOptionsExt}};

const VECTOR: &str = include_str!("../vectors/portable-genesis-v1.txt");
fn hex(key: &str) -> Vec<u8> {
    let s = VECTOR.lines().filter_map(|l| l.split_once('=')).find(|(k,_)| *k == key).unwrap().1;
    assert_eq!(s.len()%2,0);
    (0..s.len()).step_by(2).map(|i|u8::from_str_radix(&s[i..i+2],16).unwrap()).collect()
}
fn signed(mut b: Vec<u8>) -> Vec<u8> {
    let g=hash::digest(&b).unwrap(); b.extend_from_slice(&g); b
}
#[test]
fn the_four_unopened_vector_paths_derive_their_own_marks_and_exact_zero_cut() {
    let bytes=hex("genesis"); let body=hex("body");
    assert_eq!((body.len(),bytes.len()),(183,215));
    assert_eq!(signed(body),bytes);
    let g=Genesis::only(&bytes).unwrap();
    assert_eq!(g.digest.as_slice(),hex("source_digest")); assert_eq!(g.uid,1001);
    assert_eq!(g.approval_ref,std::array::from_fn(|i|0xb1+i as u8));
    assert_eq!(g.rows[3].path.as_os_str().as_bytes(),b"/m/\xffvol");
    for (row,key) in g.rows.iter().zip(["volume_s","volume_r","volume_f0","volume_f1"]) {
        assert_eq!(g.mark(row),Declaration::decode(&hex(key)).unwrap());
    }
    g.require_zero(&hex("zero_floor")).unwrap();
    for i in [0,24,25,56,57,64,65,72,73,104] {
        let mut floor=hex("zero_floor"); floor[i]^=1; assert!(g.require_zero(&floor).is_err());
    }
    assert!(g.require_zero(&hex("untrusted_nonzero_floor")).is_err());
    let mut unknown=bytes.clone();unknown.extend_from_slice(&hex("unknown_tail"));
    assert!(Genesis::only(&unknown).is_err());
    for n in 0..bytes.len(){assert!(Genesis::only(&bytes[..n]).is_err());}
}
#[test]
fn even_rehashed_malformed_original_selections_are_not_portable_genesis() {
    let mut variations=Vec::new();
    for (s,n) in [(23,16),(39,16),(55,16),(71,4),(75,2),(78,16),(94,1)]{
        let mut b=hex("body");b[s..s+n].fill(0);variations.push(b);
    }
    let mut b=hex("body");b[71..75].fill(255);variations.push(b);
    let mut b=hex("body");b[104..120].copy_from_slice(&hex("body")[78..94]);variations.push(b);
    let mut b=hex("body");b[103]=2;variations.push(b);
    let mut b=hex("body");b[176..183].copy_from_slice(b"/m/aaaa");variations.push(b);
    let mut b=hex("body");b[176..183].copy_from_slice(b"/m/bin/");variations.push(b);
    let mut b=hex("body");b[176..183].copy_from_slice(b"/m/./yo");variations.push(b);
    let mut b=hex("body");b[176..183].copy_from_slice(b"/m/v\0ol");variations.push(b);
    let mut b=hex("body");b.splice(176..183,b"/m/sys/y".iter().copied());b[174..176].copy_from_slice(&8u16.to_be_bytes());variations.push(b);
    for b in variations {assert!(Genesis::only(&signed(b)).is_err());}
    let mut ok=hex("body");ok[176..183].copy_from_slice(b"/m/sysx");
    assert!(Genesis::only(&signed(ok)).is_ok()); // not a child of /m/sys
}

fn create(p:&Path,bytes:&[u8]){
    let mut f=osfs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(p).unwrap();
    f.write_all(bytes).unwrap();f.sync_all().unwrap();
}
fn mark(row:u8,id:[u8;16],g:&[u8;32])->Vec<u8>{
    let mut b=b"ZXS-PORTABLE-1/VOLUME\0".to_vec();b.push(row);
    b.extend([1;16]);b.extend(id);b.extend([2;16]);b.extend_from_slice(g);b
}
#[test]
#[ignore="use a protected disposable directory owned by a non-root service uid; no particular filesystem format"]
fn selected_system_owns_the_lifetime_lock_and_no_remote_path_is_needed_for_its_empty_cut() {
    let uid=fs::local_uid();assert_ne!(uid,0);
    let t=PrivateTemp::under(&std::env::temp_dir());let s=t.child("sys");let f=t.child("files");
    let r=t.0.join("offline-r");
    let system=HeldDir::open(&s).unwrap();let file_root=HeldDir::open(&f).unwrap();
    let (sl,fl)=(ComponentLimit::held(&system).unwrap().encoded(),ComponentLimit::held(&file_root).unwrap().encoded());
    let mut b=b"ZXS-PORTABLE-1/GENESIS\0".to_vec();
    for v in [1,2,3]{b.extend([v;16]);}b.extend(uid.to_be_bytes());b.extend(3u16.to_be_bytes());
    for (i,(path,limit)) in [(&s,sl),(&r,sl),(&f,fl)].into_iter().enumerate(){
        let p=path.as_os_str().as_bytes();b.push(i as u8);b.extend([11+i as u8;16]);b.push(limit);
        b.extend(u16::try_from(p.len()).unwrap().to_be_bytes());b.extend_from_slice(p);
    }
    let g=hash::digest(&b).unwrap();let image=signed(b);
    let mn=super::super::portable_claim::ROOT_NAME.to_str().unwrap();
    let floor=[b"ZXS-PORTABLE-1/CONFIRMED\0".as_slice(),g.as_slice(),&[0;16],g.as_slice()].concat();
    let reg=s.join("REGISTRATION");let pending=s.join("INITIALIZING");let lower=s.join("CONFIRMED");
    create(&pending,&image);create(&lower,&floor);create(&s.join(mn),&mark(0,[11;16],&g));
    create(&f.join(mn),&mark(2,[13;16],&g));
    assert!(opened::Empty::open(&s).is_err()); // complete bytes in the pending name are not READY
    osfs::rename(&pending,&reg).unwrap();
    let original=osfs::File::open(&reg).unwrap();original.try_lock().unwrap();
    assert!(opened::Empty::open(&s).is_err());original.unlock().unwrap();
    let z=opened::Empty::open(&s).unwrap();
    assert!(original.try_lock().is_err());
    let info=z.observe(VolumeId([13;16])).unwrap();
    assert_eq!(info.id,VolumeId([13;16]));assert_eq!(info.role,Role::File);
    assert_eq!(info.entity.inode,osfs::metadata(&f).unwrap().ino());
    assert!(z.observe(VolumeId([12;16])).is_err()); // registered but offline
    assert!(z.observe(VolumeId([99;16])).is_err());
    // All three real roots now exist on this same mount. R has its
    // EXACT role/claim marker and geometry, but cannot be a dedicated R.
    assert_eq!(t.child("offline-r"),r);create(&r.join(mn),&mark(1,[12;16],&g));
    assert!(HeldDir::open(&r).unwrap().same_mount(&system).unwrap());
    assert!(z.observe(VolumeId([12;16])).is_err());
    assert_eq!(z.observe(VolumeId([13;16])).unwrap().role,Role::File);
    assert_eq!(z.observe(VolumeId([11;16])).unwrap().role,Role::System);
    // Wrong row in F cannot borrow its former S; the true S observation
    // remains available without F or R. This test does not certify disk separation.
    osfs::write(f.join(mn),mark(0,[11;16],&g)).unwrap();
    assert!(z.observe(VolumeId([13;16])).is_err());
    assert_eq!(z.observe(VolumeId([11;16])).unwrap().role,Role::System);
    // Replacing CONFIRMED with an equal fresh inode is not a continuation
    // of this held empty view. No repair / adoption of a higher frontier.
    osfs::rename(&lower,t.0.join("prior-floor")).unwrap();create(&lower,&floor);
    assert!(z.observe(VolumeId([11;16])).is_err());
    drop(z);original.try_lock().unwrap();original.unlock().unwrap();
    assert_eq!(osfs::read(&reg).unwrap(),image);
    // A copied second name / old family / unknown tail cannot be treated
    // as this empty registration. No writes restore a chosen frontier.
    osfs::hard_link(&reg,&pending).unwrap();assert!(opened::Empty::open(&s).is_err());osfs::remove_file(&pending).unwrap();
    osfs::OpenOptions::new().append(true).open(&reg).unwrap().write_all(&[0xde,0xad]).unwrap();
    assert!(opened::Empty::open(&s).is_err());assert_eq!(osfs::read(&lower).unwrap(),floor);
}

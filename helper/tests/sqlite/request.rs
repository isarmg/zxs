// Fixed public values are deliberately independent of the encoder. No medium or user identity.
use super::*;
use std::{ffi::OsString,os::unix::ffi::OsStringExt};
fn paths()->Vec<PathBuf>{[b"/srv/zxs-s".as_slice(),b"/mnt/zxs-r",b"/mnt/f-a",b"/mnt/f-\xff"].into_iter()
    .map(|b|PathBuf::from(OsString::from_vec(b.to_vec()))).collect()}
#[test]
fn four_roots_match_published_sqlite_request_and_preserve_non_utf8(){
    let r=Request::new([1;16],[2;16],[3;16],1000,paths()).unwrap();
    assert_eq!(r.sha(),[0x5f,0x78,0x94,0x41,0x27,0x99,0xad,0x53,0xef,0x6a,0xb1,0xfe,0x00,0x98,0x16,0x78,
        0x55,0x06,0xd3,0x89,0xde,0x2c,0x1e,0xdd,0x9b,0x02,0xe7,0x9b,0xe6,0xab,0xa1,0x1a]);
    let mut wrong=paths();wrong.swap(2,3);assert!(Request::new([1;16],[2;16],[3;16],1000,wrong).is_err());
    let mut nested=paths();nested[3]=nested[2].join("sub");assert!(Request::new([1;16],[2;16],[3;16],1000,nested).is_err());
    let mut non_utf=paths();non_utf.swap(0,3);
    assert_eq!(Request::new([1;16],[2;16],[3;16],1000,non_utf).err().unwrap().kind(),io::ErrorKind::Unsupported);
    assert_ne!(Request::new([1;16],[2;16],[4;16],1000,paths()).unwrap().sha(),r.sha());
}
#[test]
fn both_exact_original_names_including_terminator_must_fit_real_vfs(){
    let r=Request::new([1;16],[2;16],[3;16],1000,paths()).unwrap();
    let n=b"/srv/zxs-s/SYSTEM.sqlite3-journal\0".len();
    assert_eq!(r.database(n).unwrap(),Path::new("/srv/zxs-s/SYSTEM.sqlite3"));
    assert!(r.database(n-1).is_err());
}

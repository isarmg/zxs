use super::*;
use crate::system::reader::zxs_sql_template;
#[test]
fn blob_number_ranges_include_both_ends_and_reject_ambiguous_originals(){
    // Use the actual pinned SQLite engine and a deliberately minimal
    // index. This fixture can represent a pre-existing collision which
    // the full CREATE path cannot issue. No new production table.
    let mut ptr=std::ptr::null_mut();
    check(unsafe{zxs_sql_template(c"CREATE TABLE root_entry(file_id BLOB PRIMARY KEY) STRICT, WITHOUT ROWID".as_ptr(),&mut ptr)}).unwrap();
    assert!(!ptr.is_null());let db=Connection(ptr);
    let id=|hi:u64,lo:u64|{let mut b=[0;16];b[..8].copy_from_slice(&hi.to_be_bytes());b[8..].copy_from_slice(&lo.to_be_bytes());FileId(b)};
    let insert=|x:FileId|{let q=db.prepare(c"INSERT INTO main.root_entry VALUES(?1)").unwrap();
        q.bind_bytes(1,&x.0).unwrap();q.done().unwrap();};
    let mut p=[0x81;16];p[8..].fill(0x42);let pool=PoolId(p);
    let occupied=0xfedcba9876543210;let a=id(occupied,0);let b=id(occupied,u64::MAX);
    let fresh=id(occupied+1,3);let small=id(255,8);let last=id(u64::MAX,7);
    insert(a);assert!(original(&db,pool,b).is_err());
    assert_eq!(original(&db,pool,a).unwrap().words(),(0x8181818181818181,occupied,0x4242424242424242));
    assert_eq!(choose(&db,&[small,last,b,b,b,b,b,fresh]).unwrap(),fresh);
    assert!(choose(&db,&[b;8]).is_err());
    insert(b);assert!(original(&db,pool,a).is_err());assert!(original(&db,pool,b).is_err());
    assert!(original(&db,pool,fresh).is_err());
    insert(small);insert(last);assert!(original(&db,pool,small).is_err());assert!(original(&db,pool,last).is_err());
}

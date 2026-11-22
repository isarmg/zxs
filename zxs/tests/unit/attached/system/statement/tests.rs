use super::*;

fn memory()->Connection{
    let mut p=ptr::null_mut();
    check(unsafe{super::super::zxs_sql_template(c"CREATE TABLE fixture(data BLOB);".as_ptr(),&mut p)}).unwrap();
    assert!(!p.is_null());
    Connection(p)
}

#[test]
fn sqlite_variable_row_keeps_exact_types_bytes_and_bounds(){
    let db=memory();
    let mut q=db.prepare(c"SELECT X'6162',X'000063',NULL,'ab',X'',17").unwrap();
    assert_eq!(q.row().err().unwrap().to_string(),"no live SQLite result row");
    assert_eq!(q.step(),100);
    let copied={
        let row=q.row().unwrap();
        let first=row.blob(0,2).unwrap();
        let second=row.blob(1,3).unwrap();
        assert!(row.is_null(2));
        assert!(!row.is_null(4));
        assert_eq!(row.byte(5).unwrap(),17);
        assert_eq!(first,b"ab");
        assert_eq!(second,b"\0\0c");
        for column in [2,3,5]{
            assert_eq!(row.blob(column,20).unwrap_err().to_string(),"unexpected SQLite blob storage class");
        }
        for (column,max) in [(4,20),(1,2)]{
            assert_eq!(row.blob(column,max).unwrap_err().to_string(),"missing or oversized SQLite blob");
        }
        row.array::<2>(0).unwrap()
    };
    assert!(q.blob_eq(0,2,b"ab").unwrap());
    assert!(!q.blob_eq(0,2,b"ba").unwrap());
    q.done().unwrap();
    assert_eq!(q.row().err().unwrap().kind(),io::ErrorKind::InvalidInput);
    assert_eq!(copied,*b"ab");
    let mut empty=db.prepare(c"SELECT X'6162' WHERE 0").unwrap();
    assert_eq!(empty.step(),101);
    assert!(empty.row().is_err());
}

#[test]
fn sqlite_row_reborrow_tracks_each_live_result_without_copies(){
    let db=memory();
    let mut q=db.prepare(c"WITH RECURSIVE rows(n) AS (VALUES (1) UNION ALL SELECT n+1 FROM rows WHERE n<4096)
            SELECT CAST(printf('%04x',n) AS BLOB),n FROM rows").unwrap();
    for n in 1..=4096{
        assert_eq!(q.step(),100);
        assert_eq!(q.integer(1).unwrap(),n);
        let row=q.row().unwrap();
        assert_eq!(row.blob(0,4).unwrap(),format!("{n:04x}").as_bytes());
    }
    assert_eq!(q.step(),101);
    assert!(q.row().is_err());
}

use super::*;
#[test]
fn management_navigation_budget_interrupts_only_the_original_cut(){
    let ddl=std::ffi::CString::new(crate::system::SCHEMA).unwrap();let mut raw=std::ptr::null_mut();
    check(unsafe{super::super::super::zxs_sql_template(ddl.as_ptr(),&mut raw)}).unwrap();
    assert!(!raw.is_null());let db=Connection(raw);
    let e=navigation_cut(&db,1,||{
        let q=db.prepare(c"WITH RECURSIVE nums(i) AS
                (SELECT 1 UNION ALL SELECT i+1 FROM nums WHERE i<10000) SELECT count(*) FROM nums")?;
        if !work_record::start_one(&q)?{return Err(invalid("missing probe count"));}
        q.done()
    }).unwrap_err();
    assert_eq!(e.kind(),io::ErrorKind::WouldBlock);
    let q=db.prepare(c"WITH RECURSIVE nums(i) AS
                (SELECT 1 UNION ALL SELECT i+1 FROM nums WHERE i<10000) SELECT count(*) FROM nums").unwrap();
    assert!(work_record::start_one(&q).unwrap());assert_eq!(q.integer(0).unwrap(),10000);q.done().unwrap();
}

use super::*;

#[test]
fn file_label_page_and_idempotent_exact_key() {
    // Use the actual production label table, a minimal parent key, and the
    // original SQLite implementation; no files, server, or port is involved.
    let text = crate::system::SCHEMA.split("CREATE TABLE file_label (").nth(1)
        .unwrap().split_once(") STRICT, WITHOUT ROWID;").unwrap().0;
    let ddl = std::ffi::CString::new(format!("PRAGMA foreign_keys=ON;
        CREATE TABLE root_entry(file_id BLOB PRIMARY KEY) STRICT, WITHOUT ROWID;
        CREATE TABLE file_label ({text}) STRICT, WITHOUT ROWID;
        INSERT INTO root_entry VALUES (x'11111111111111111111111111111111');")).unwrap();
    let mut raw = std::ptr::null_mut();
    super::super::check(unsafe {
        crate::system::reader::zxs_sql_template(ddl.as_ptr(), &mut raw)
    }).unwrap();
    assert!(!raw.is_null());
    let db = Connection(raw);
    let file = FileId([0x11; 16]);
    for at in 0..35 {
        let label = Label::parse(&format!("Sketch {at:02}")).unwrap();
        set(&db, file, &label, true).unwrap();
        set(&db, file, &label, true).unwrap();
    }
    let first = list(&db, file, None).unwrap();
    assert_eq!(first.names.len(), 32);
    assert_eq!(first.next.as_ref().unwrap().text(), "Sketch 31");
    let last = list(&db, file, first.next.as_ref()).unwrap();
    assert_eq!(last.names.iter().map(Label::text).collect::<Vec<_>>(),
        ["Sketch 32", "Sketch 33", "Sketch 34"]);
    assert!(last.next.is_none());
    let label = Label::parse("Sketch 31").unwrap();
    assert!(contains(&db, file, &label).unwrap());
    assert!(!contains(&db, file, &Label::parse("sketch 31").unwrap()).unwrap());
    set(&db, file, &label, false).unwrap();
    set(&db, file, &label, false).unwrap();
    assert!(!contains(&db, file, &label).unwrap());
    assert!(set(&db, FileId([0x22;16]), &label, true).is_err());
}

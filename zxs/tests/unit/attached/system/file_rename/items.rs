#[test]
fn ordinary_rename_recovery_page_does_not_fill_with_aborted_history(){
    use super::super::{check,zxs_sql_template};
    let mut raw=std::ptr::null_mut();
    // SAFETY: fixed test SQL and an initialized output; no actual pool is opened.
    check(unsafe{zxs_sql_template(c"
        CREATE TABLE operation(op_id BLOB PRIMARY KEY,kind INTEGER,state INTEGER) STRICT, WITHOUT ROWID;
        CREATE TABLE file_rename(rename_op BLOB PRIMARY KEY,file_id BLOB) STRICT, WITHOUT ROWID;
        CREATE TABLE file_rename_abort(rename_op BLOB PRIMARY KEY) STRICT, WITHOUT ROWID;
    ".as_ptr(),&mut raw)}).unwrap();
    let db=Connection(raw);
    for i in 1..=20{
        let op=[i;16];let file=[i+64;16];
        let add=db.prepare(c"INSERT INTO operation VALUES (?1,?2,?3)").unwrap();
        add.bind_bytes(1,&op).unwrap();
        add.bind_integer(2,if i==19{16}else{14}).unwrap();
        add.bind_integer(3,i64::from(i==18)).unwrap();
        add.done().unwrap();
        let rename=db.prepare(c"INSERT INTO file_rename VALUES (?1,?2)").unwrap();
        rename.bind_bytes(1,&op).unwrap();
        rename.bind_bytes(2,&file).unwrap();
        rename.done().unwrap();
        if i<=16{
            let abort=db.prepare(c"INSERT INTO file_rename_abort VALUES (?1)").unwrap();
            abort.bind_bytes(1,&op).unwrap();
            abort.done().unwrap();
        }
    }
    let first=(OperationId([17;16]),FileId([81;16]));
    let second=(OperationId([20;16]),FileId([84;16]));
    assert_eq!(recovery_page(&db,OperationId([0;16])).unwrap(),[first,second]);
    assert_eq!(recovery_page(&db,first.0).unwrap(),[second]);
}

pub(super) fn recover_for_test(src:&InitialReader,op:OperationId,file:FileId,
    allowed:impl Fn(&Connection,&Catalog,ShareId,VolumeId)->io::Result<()>)->io::Result<()>{
    recover(src,op,file,&allowed,&||Ok(()))
}

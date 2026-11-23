//! The real pair receipt reader/writer over inert in-memory fixture rows.
use super::*;
use super::super::super::{check,zxs_sql_template};
use std::ptr;

fn memory()->Connection{
    // The actual v20 graph separately checks live source/target ACL and
    // original placement. This fixture exercises paired selection/proofs.
    let ddl=c"CREATE TABLE operation(op_id BLOB PRIMARY KEY,kind INTEGER,state INTEGER,allowed INTEGER);
        CREATE TABLE file_replace(replace_op BLOB PRIMARY KEY,save_op BLOB UNIQUE,recycle_op BLOB UNIQUE,state INTEGER);
        CREATE TABLE file_transfer_source(transfer_op BLOB PRIMARY KEY,guard BLOB,target_guard BLOB,status INTEGER);
        CREATE TABLE file_transfer_void(transfer_op BLOB PRIMARY KEY,setting_op BLOB,auth_op BLOB,reason INTEGER);
        CREATE TRIGGER pending_pair_only BEFORE UPDATE OF state ON operation
          WHEN EXISTS(SELECT 1 FROM file_replace p WHERE (save_op=OLD.op_id OR recycle_op=OLD.op_id) AND p.state<>1)
               OR OLD.allowed<>1
          BEGIN SELECT RAISE(ABORT,'refused original'); END;
        CREATE TRIGGER select_pair AFTER UPDATE OF state ON file_replace WHEN NEW.state=1
          BEGIN
            UPDATE operation SET state=1 WHERE op_id=NEW.save_op AND kind=10 AND state=0;
            SELECT CASE WHEN changes()<>1 THEN RAISE(ABORT,'SAVE missing') END;
            UPDATE operation SET state=1 WHERE op_id=NEW.recycle_op AND kind=12 AND state=0;
            SELECT CASE WHEN changes()<>1 THEN RAISE(ABORT,'recycle missing') END;
          END;";
    let mut raw=ptr::null_mut();
    // SAFETY: one fixed NUL-terminated DDL string and writable pointer out;
    // success returns one owned in-memory SQLite connection.
    check(unsafe{zxs_sql_template(ddl.as_ptr(),&mut raw)}).unwrap();
    assert!(!raw.is_null());
    Connection(raw)
}

fn op(n:u8)->OperationId{OperationId([n;16])}
fn provision(db:&Connection,allow_recycle:bool)->Pair{
    let pair=Pair{replacement:op(1),save:op(2),recycle:op(3)};
    for (id,kind,allowed) in [(pair.save,10,true),(pair.recycle,12,allow_recycle)]{
        let q=db.prepare(c"INSERT INTO operation VALUES(?1,?2,0,?3)").unwrap();
        q.bind_bytes(1,&id.0).unwrap();q.bind_integer(2,kind).unwrap();
        q.bind_integer(3,i64::from(u8::from(allowed))).unwrap();q.done().unwrap();
    }
    pair.reserve(db).unwrap();
    pair
}

#[test]
fn original_pair_is_selected_once_with_exact_retirement(){
    let db=memory();let pair=provision(&db,true);
    assert_eq!(pair.state(&db).unwrap(),Some(State::Pending));
    assert!(db.fixed(c"UPDATE operation SET state=1 WHERE kind=10").is_err());
    assert!(db.fixed(c"UPDATE operation SET state=1 WHERE kind=12").is_err());
    assert!(Pair::require_unpaired(&db,pair.save).is_err());
    assert!(Pair::require_unpaired(&db,pair.recycle).is_err());
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    pair.confirm(&db,Some(&[8]),&[9]).unwrap();
    db.fixed(c"COMMIT").unwrap();
    assert_eq!(pair.state(&db).unwrap(),Some(State::Selected));
    assert!(pair.confirm(&db,Some(&[8]),&[9]).is_err());
}

#[test]
fn final_refusal_rolls_back_both_and_exact_void_does_not_select_either(){
    let db=memory();let pair=provision(&db,false);
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    assert!(pair.confirm(&db,Some(&[8]),&[9]).is_err());
    db.fixed(c"ROLLBACK").unwrap();
    assert_eq!(pair.state(&db).unwrap(),Some(State::Pending));
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    pair.release(&db,op(10),op(11),2).unwrap();
    db.fixed(c"COMMIT").unwrap();
    assert_eq!(pair.state(&db).unwrap(),Some(State::Voided));
    assert!(db.fixed(c"UPDATE operation SET state=1").is_err());
}

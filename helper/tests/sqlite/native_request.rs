//! Pure source fixtures against the exact candidate schema, no native
//! authority. SQL acceptability alone never proves an SMB credential.
use super::*;
use rusqlite::{Connection, TransactionBehavior};

fn vector(name: &str) -> Vec<u8> {
    let v = include_str!("native-request-v7.txt").lines().filter_map(|s| s.split_once('='))
        .find(|(k, _)| *k == name).expect("public vector").1;
    assert_eq!(v.len() % 2, 0);
    v.as_bytes().chunks_exact(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap()).collect()
}
fn id(name: &str) -> Id { vector(name).try_into().unwrap() }
fn bound(is_reset: bool) -> Binding {
    Binding::expected(&vector(if is_reset { "smb_reset_bound" } else { "setup_bound" }).try_into().unwrap(),
        &id("pool"), &id("instance"), &[0x08;16], &[if is_reset { 0xa0 } else { 0x90 };32]).unwrap()
}
fn birth() -> Birth {
    Birth::new(id("pool"), id("init_op"), id("setup_op"), id("offline_actor"), id("approval_ref"),
        bound(false), &vector("web_login_hex"), &vector("smb_login_hex")).unwrap()
}
fn reset(op: Id) -> Reset {
    Reset::smb(id("pool"), id("init_op"), id("setup_op"), op, id("offline_actor"), id("reset_approval"),
        id("result_smb_id"), id("setup_op"), 1, bound(true), &vector("smb_login_hex")).unwrap()
}
fn con() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    conn.execute_batch(include_str!("../../src/sqlite_install/schema.sql")).unwrap();
    let pool = id("pool"); let init = id("init_op"); let actor = id("offline_actor"); let approval = id("approval_ref");
    // The dummy INIT here only supplies the SQL foreign key context.
    // It is not a disk selection or a full original INIT request.
    let tx = conn.unchecked_transaction().unwrap();
    tx.execute("INSERT INTO main.operation VALUES(?1,1,?2,zeroblob(32),0)",
        params![&init[..], &actor[..]]).unwrap();
    tx.execute("INSERT INTO main.pool_birth VALUES(1,?1,?2,?3,1001,3)",
        params![&pool[..], &init[..], &approval[..]]).unwrap();
    for (i,path) in [b"/s",b"/r",b"/f"].into_iter().enumerate() {
        tx.execute("INSERT INTO main.volume_claim VALUES(?1,?2,?2,?3,?4,255,?5)",
            params![&init[..],i as i64, &[80+i as u8;16][..], &path[..], &[100+i as u8;32][..]]).unwrap();
    }
    tx.execute("UPDATE main.operation SET state=1 WHERE op_id=?1", [&init[..]]).unwrap();
    tx.commit().unwrap();
    conn
}

#[test]
fn exact_public_scope_encodings_match_both_full_bound_digest_examples() {
    let a = birth();
    let b = reset(id("smb_reset_op"));
    assert_eq!(a.binding.sha.as_slice(), vector("setup_bound_sha"));
    assert_eq!(b.binding.sha.as_slice(), vector("smb_reset_bound_sha"));
    assert_ne!(a.binding.sha, b.binding.sha);
    assert_eq!(a.binding.sid.as_slice(), vector("smb_sid"));
    assert_eq!(a.sha().as_slice(), vector("request_sha"));
    assert_eq!(b.sha().as_slice(), vector("smb_reset_sha"));
    let web=web_reset::Reset::new(id("pool"),id("init_op"),id("setup_op"),id("web_reset_op"),id("offline_actor"),
        id("reset_approval"),id("result_web_id"),id("setup_op"),1).unwrap();
    assert_eq!(web.sha().as_slice(),vector("web_reset_sha"));
    assert!(Reset::smb(id("pool"), id("init_op"), id("setup_op"), id("smb_reset_op"), id("offline_actor"),
        id("reset_approval"), id("result_smb_id"), id("setup_op"), i64::MAX, bound(true), &vector("smb_login_hex")).is_err());
}

fn active(tx: &Transaction<'_>, role: i64) -> i64 {
    tx.query_row("SELECT COALESCE((SELECT generation FROM main.active_credential WHERE role=?1),0)", [role],
        |r| r.get(0)).unwrap()
}
#[test]
fn reservation_and_conditional_confirmation_never_confuse_pending_with_an_original_receipt() {
    let mut db = con();
    let first = birth();
    let ids = Ids::checked(id("result_web_id"), id("result_smb_id")).unwrap();
    let verifier = super::super::web_secret::PreparedWeb::synthetic(vector("web_salt").try_into().unwrap(),
        vector("web_verifier").try_into().unwrap());
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    assert_eq!(first.original(&tx).unwrap(), Original::Absent);
    assert!(activate(&tx, &first.scope, first.scope.op, &first.binding, &first.sha(), ids.smb,
        &first.smb, 1, None).is_err());
    first.reserve(&tx, ids, &verifier).unwrap();
    assert_eq!(first.original(&tx).unwrap(), Original::Pending(ids));
    assert_eq!((active(&tx,0),active(&tx,1)), (0,0));
    assert!(first.reserve(&tx, ids, &verifier).is_err());
    tx.commit().unwrap();

    // The test supplies the last SQL predicate deliberately, without a native
    // TDB proof. An actual caller must not reach this merely from a pending row.
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    let wrong = Binding { sha: bound(true).sha, ..first.binding };
    assert!(activate(&tx, &first.scope, first.scope.op, &wrong, &first.sha(), ids.smb,
        &first.smb, 1, None).is_err());
    assert_eq!(first.original(&tx).unwrap(), Original::Pending(ids));
    activate(&tx, &first.scope, first.scope.op, &first.binding, &first.sha(), ids.smb, &first.smb, 1, None).unwrap();
    assert_eq!(first.original(&tx).unwrap(), Original::Committed(ids));
    assert_eq!((active(&tx,0),active(&tx,1)), (1,1));
    tx.commit().unwrap();

    let r = reset(id("smb_reset_op"));
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    r.reserve(&tx).unwrap();
    assert_eq!(r.original(&tx).unwrap(), Original::Pending(2));
    assert_eq!((active(&tx,0),active(&tx,1)), (1,0));
    let contender = reset([0xb9;16]);
    assert!(contender.reserve(&tx).is_err());
    tx.commit().unwrap();
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    assert!(activate(&tx, &r.scope, r.setup, &r.binding, &r.sha(), r.account, &r.login, 2, None).is_err());
    activate(&tx, &r.scope, r.setup, &r.binding, &r.sha(), r.account, &r.login, 2, Some(r.previous)).unwrap();
    assert_eq!(r.original(&tx).unwrap(), Original::Committed(2));
    assert_eq!((active(&tx,0),active(&tx,1)), (1,2));
    assert!(activate(&tx, &r.scope, r.setup, &r.binding, &r.sha(), r.account, &r.login, 2, Some(r.previous)).is_err());
    let old_web: i64 = tx.query_row("SELECT generation FROM main.web_password WHERE slot=1", [], |r|r.get(0)).unwrap();
    assert_eq!(old_web,1);
    let tables: i64 = tx.query_row("SELECT count(*) FROM sqlite_schema WHERE name='smb_password'", [], |r|r.get(0)).unwrap();
    assert_eq!(tables,0);
    tx.commit().unwrap();
}

#[test]
fn later_resets_follow_the_exact_committed_predecessor_not_a_name_or_generation_alone() {
    let mut db = con();
    let first = birth();
    let ids = Ids::checked(id("result_web_id"), id("result_smb_id")).unwrap();
    let verifier = super::super::web_secret::PreparedWeb::synthetic(vector("web_salt").try_into().unwrap(),
        vector("web_verifier").try_into().unwrap());
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    first.reserve(&tx, ids, &verifier).unwrap();
    activate(&tx, &first.scope, first.scope.op, &first.binding, &first.sha(), ids.smb, &first.smb, 1, None).unwrap();
    let r = reset(id("smb_reset_op"));
    r.reserve(&tx).unwrap();
    let later = |previous| Reset::smb(id("pool"), id("init_op"), id("setup_op"), [0xc3;16],
        id("offline_actor"), id("reset_approval"), ids.smb, previous, 2, bound(true),
        &vector("smb_login_hex")).unwrap();
    let next = later(r.scope.op);
    assert!(next.original(&tx).is_err()); // the recorded predecessor is still pending
    activate(&tx, &r.scope, r.setup, &r.binding, &r.sha(), r.account, &r.login, 2, Some(r.previous)).unwrap();
    assert!(later([0x92;16]).original(&tx).is_err());
    assert_eq!(next.original(&tx).unwrap(), Original::Absent);
    next.reserve(&tx).unwrap();
    assert_eq!(next.original(&tx).unwrap(), Original::Pending(3));
    assert_eq!(r.original(&tx).unwrap(), Original::Committed(2)); // history remains available
    assert_eq!((active(&tx,0),active(&tx,1)), (1,0)); // no SMB fallback to that history
    tx.commit().unwrap();
}

#[test]
fn only_web_reset_changes_web_and_never_creates_a_native_pending_step() {
    let mut db=con();
    let first=birth();
    let ids=Ids::checked(id("result_web_id"),id("result_smb_id")).unwrap();
    let verifier=super::super::web_secret::PreparedWeb::synthetic([2;16],[3;32]);
    let new=super::super::web_secret::PreparedWeb::synthetic([5;16],[6;32]);
    let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    first.reserve(&tx,ids,&verifier).unwrap();
    activate(&tx,&first.scope,first.scope.op,&first.binding,&first.sha(),ids.smb,&first.smb,1,None).unwrap();
    let reset=|op|web_reset::Reset::new(id("pool"),id("init_op"),id("setup_op"),op,id("offline_actor"),
        id("reset_approval"),ids.web,id("setup_op"),1).unwrap();
    let r=reset(id("web_reset_op"));
    assert_eq!(r.original(&tx).unwrap(),Original::Absent);
    assert_eq!(r.update(&tx,&new).unwrap(),2);
    assert_eq!(r.original(&tx).unwrap(),Original::Committed(2));
    assert_eq!((active(&tx,0),active(&tx,1)),(2,1));
    assert!(r.update(&tx,&verifier).is_err());
    assert!(reset([0x66;16]).eligible(&tx).is_err());
    tx.commit().unwrap();
}

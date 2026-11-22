//! Shared account fixture using a real private SQLite file. Never derives an
//! SMB credential or creates a vault; that proof belongs to a separate role.
use super::*;
use crate::system::{reader::smb::SmbSession,shared_account::{PublicEpoch,SharedScope}};

const SETUP: [u8;16]=[0x44;16];
const INSTANCE: [u8;16]=[0x33;16];
const BIND: [u8;16]=[0x66;16];
const ACCOUNT: [u8;16]=[0x55;16];
const APPROVAL: [u8;16]=[0x77;16];
const FIRST: [u8;16]=[0x71;16];
const NEW_OP: [u8;16]=[0x78;16];
const NEW_RECORD: [u8;16]=[0x79;16];
const NEW_APPROVAL: [u8;16]=[0x7a;16];
const USER:&str="first.admin";
const OLD_PASS:&str="correct horse battery staple"; // independent public fixture
const NEW_PASS:&str="synthetic shared rotation fixture"; // synthetic public fixture only
const NEW_TAG:&str="f4a5f283ce9072614dea2ca3c7a38d78312a46fef3d0aedaaa2797da558928e1"; // bundled reference C

fn scope(db:&Builder)->SharedScope<'static>{
    let q=db.0.prepare(c"SELECT b.pool_id,b.init_op,o.actor_id,o.request_sha FROM main.pool_birth b
        JOIN main.operation o ON o.op_id=b.init_op WHERE b.singleton=1 AND o.kind=1 AND o.state=1").unwrap();
    assert_eq!(q.step(),100);
    let mut sid=[0x22;28];
    sid[..12].copy_from_slice(b"\x01\x05\x00\x00\x00\x00\x00\x05\x15\x00\x00\x00");
    sid[24..].copy_from_slice(&1000u32.to_le_bytes());
    let s=SharedScope{pool:q.array(0).unwrap(),init:q.array(1).unwrap(),original_actor:q.array(2).unwrap(),
        init_sha:q.array(3).unwrap(),setup:SETUP,instance:INSTANCE,bind:BIND,account:ACCOUNT,
        plan_sha:[0x61;32],login:USER.as_bytes(),sid};
    q.done().unwrap();s
}
pub(super) fn rows(db:&Builder)->[String;5]{
    let s=scope(db);let keys=s.role_keys().unwrap();
    let op=hex(&SETUP);let a=hex(&APPROVAL);let inst=hex(&INSTANCE);let plan=hex(&s.plan_sha);
    let account=hex(&ACCOUNT);let login=hex(USER.as_bytes());let web=hex(&keys.web);let smb=hex(&keys.smb);
    let sid=hex(&s.sid);let bind=hex(&BIND);let record=hex(&FIRST);let actor=hex(&s.original_actor);
    let sha=hex(&s.setup_request(APPROVAL,FIRST).unwrap());
    let fields=include_str!("../sqlite/first-account-v1.txt"); // old KDF vector, not old account or format
    let get=|key|fields.lines().filter_map(|l|l.split_once('=')).find(|(k,_)|*k==key).unwrap().1;
    let salt=get("web_salt");let tag=get("web_verifier");
    [format!("INSERT INTO account_pair VALUES(1,X'{op}',X'{a}',X'{inst}',X'{plan}',X'{account}',X'{login}',X'{web}',X'{smb}',X'{sid}',X'{bind}');"),
     format!("INSERT INTO web_password VALUES(1,X'{op}',1,X'{salt}',X'{tag}');"),
     format!("INSERT INTO smb_rust_revision VALUES(X'{op}',X'{record}');"),
     format!("INSERT INTO operation VALUES(X'{op}',2,X'{actor}',X'{sha}',0);"),
     format!("UPDATE operation SET state=1 WHERE op_id=X'{op}' AND kind=2 AND state=0;")]
}
fn stage_rotation(db:&Builder){
    let s=scope(db);
    let old=PublicEpoch{op:SETUP,generation:1,record:FIRST};
    let new=PublicEpoch{op:NEW_OP,generation:2,record:NEW_RECORD};
    let req=hex(&s.reset_request(NEW_APPROVAL,old,new).unwrap());let plan=hex(&s.plan_sha);
    let (op,prior,ap,record,actor)=(hex(&NEW_OP),hex(&SETUP),hex(&NEW_APPROVAL),hex(&NEW_RECORD),hex(&s.original_actor));
    let salt=hex(&[b'R';16]);
    db.exec("PRAGMA foreign_keys=ON; PRAGMA synchronous=EXTRA; BEGIN IMMEDIATE;");
    db.exec(&format!("INSERT INTO credential_reset VALUES(X'{op}',X'{prior}',2,X'{ap}',X'{plan}');"));
    db.exec(&format!("UPDATE web_password SET credential_op=X'{op}',generation=2,salt=X'{salt}',verifier=X'{NEW_TAG}'
         WHERE slot=1 AND credential_op=X'{prior}' AND generation=1;"));
    db.exec(&format!("INSERT INTO smb_rust_revision VALUES(X'{op}',X'{record}');"));
    db.exec(&format!("INSERT INTO operation VALUES(X'{op}',9,X'{actor}',X'{req}',0); COMMIT;"));
}
fn finish_rotation(db:&Builder){
    db.exec(&format!("BEGIN IMMEDIATE; UPDATE operation SET state=1 WHERE op_id=X'{}' AND kind=9 AND state=0; COMMIT;",
        hex(&NEW_OP)));
}
pub(super) fn fixture_rotate_account(db:&Builder){
    stage_rotation(db);finish_rotation(db)
}

#[test]
#[ignore="non-root, private original S and real Argon2; test-only SMB authority does not authenticate a client"]
fn one_credential_pending_revokes_both_original_role_witnesses_and_reopens_exactly_once(){
    let t=PrivateTemp::under(&std::env::temp_dir());
    let(s,p)=record(&t,InitState::Committed,"");
    let db=Builder::existing(&s.join("SYSTEM.sqlite3"));
    let [a,w,r,op,on]=rows(&db);
    db.exec("PRAGMA foreign_keys=ON; PRAGMA synchronous=EXTRA; BEGIN IMMEDIATE;");
    db.exec(&a);db.exec(&w);assert!(db.try_exec(&op).is_err()); // no record
    db.exec(&r);db.exec(&op);db.exec("COMMIT;");
    let online=InitialReader::open(&s,p).unwrap();
    let login=||online.authenticate_web(USER,OLD_PASS);
    let slot=||SmbSession::fixture_authenticated(&online,USER);
    assert!(login().is_err());assert!(slot().is_err());
    db.exec("BEGIN IMMEDIATE;");db.exec(&on);db.exec("COMMIT;");
    let admin=login().unwrap();let smb=slot().unwrap();
    let keys=scope(&db).role_keys().unwrap();assert_eq!(admin.id().unwrap().0,keys.web);
    assert_ne!(keys.web,keys.smb);
    for wrong in ["","guest","anonymous","files.smb","FIRST.ADMIN","nobody"]{
        assert!(online.authenticate_web(wrong,OLD_PASS).is_err());
    }
    let share=admin.publish_root(OperationId([0x81;16]),crate::VolumeId([0x13;16]),"documents").unwrap();
    admin.set_share_mode(OperationId([0x83;16]),share.share,share.op,account::share_mode::Mode::ReadOnly).unwrap();
    assert!(smb.connect(share.share).is_ok());
    stage_rotation(&db);
    assert!(login().is_err());assert!(slot().is_err());
    assert!(admin.id().is_err());assert!(smb.connect(share.share).is_err());
    assert!(online.authenticate_web(USER,NEW_PASS).is_err());
    finish_rotation(&db);
    assert!(admin.id().is_err());assert!(smb.connect(share.share).is_err());
    assert!(login().is_err());
    let renewed=online.authenticate_web(USER,NEW_PASS).unwrap();assert_eq!(renewed.id().unwrap().0,keys.web);
    assert!(slot().unwrap().connect(share.share).is_ok());
    assert!(db.try_exec("SELECT * FROM main.smb_password").is_err());
}

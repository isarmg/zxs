//! Two fixed local protocol slots; these fixtures do not authenticate SMB.
use super::*;
use crate::identity::AccountId;
use crate::system::reader::account::{setup_receipt,setup_request};
const V:&str=include_str!("../sqlite/first-account-v1.txt");
fn field(k:&str)->&'static str{
    V.lines().find_map(|line|line.split_once('=').filter(|(n,_)|*n==k).map(|(_,v)|v)).unwrap()
}
fn sid()->[u8;28]{[1,5,0,0,0,0,0,5,21,0,0,0,1,0,0,0,2,0,0,0,3,0,0,0,232,3,0,0]}
pub(super) fn rows()->[String;4]{
    let op=field("setup_op");let web=field("result_web_id");let smb=field("result_smb_id");let app=field("approval_ref");
    let wn=field("web_login_hex");let sn=field("smb_login_hex");let sid=field("smb_sid");
    let salt=field("web_salt");let tag=field("web_verifier");let key=field("synthetic_smb_field");
    let actor=field("offline_actor");let sha=field("request_sha");
    [format!("INSERT INTO account_pair VALUES(1,X'{op}',X'{app}',X'{web}',X'{wn}',X'{smb}',X'{sn}',X'{sid}');"),
     format!("INSERT INTO web_password VALUES(1,X'{op}',1,X'{salt}',X'{tag}');"),
     format!("INSERT INTO smb_password VALUES(1,X'{op}',1,X'{key}');"),
     format!("INSERT INTO operation VALUES(X'{op}',2,X'{actor}',X'{sha}',1);")]
}
#[test]fn two_roles_and_standalone_sid_enter_the_original_nonsecret_request(){
    let op=OperationId([0x44;16]);let init=OperationId([1;16]);let p=PoolId([4;16]);
    let req=|web:&[u8],smb:&[u8],sid:&[u8;28]|setup_request(p,init,op,[0x66;16],[0x77;16],web,smb,sid);
    assert_eq!(hex(&req(b"first.admin",b"files.smb",&sid()).unwrap()),field("request_sha"));
    assert_ne!(req(b"first.admin",b"files.smb",&sid()).unwrap(),req(b"files.smb",b"first.admin",&sid()).unwrap());
    for bad in [b"".as_slice(),b"guest",b"anonymous",b"first.admin",b"Files.Smb"]{assert!(req(b"first.admin",bad,&sid()).is_err());}
    let mut guest=sid();guest[24..].copy_from_slice(&501u32.to_le_bytes());assert!(req(b"admin",b"files",&guest).is_err());
}
#[test]
#[ignore="source-only real SQLite/Argon2id qualification, non-root storage principal"]
fn one_atomic_pair_no_guest_and_only_web_credentials_can_open_a_web_session(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=record(&t,InitState::Committed,"");
    let op=OperationId([0x44;16]);let actor=[0x66;16];let web=AccountId([0x55;16]);let smb=AccountId([0x56;16]);
    let [a,w,k,o]=rows();
    {
        let owned=HeldDir::trusted_system(&s).unwrap();owned.exclusive().unwrap();
        let db=Builder::existing(&s.join("SYSTEM.sqlite3"));
        db.exec("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=EXTRA;");
        for (first,second) in [(&w,&k),(&k,&w)]{
            db.exec("BEGIN IMMEDIATE");db.exec(&a);db.exec(first);
            assert!(db.try_exec(&o).is_err());assert!(db.try_exec("COMMIT").is_err());
            db.exec("ROLLBACK"); // neither half-pair nor receipt survived
            db.exec("BEGIN IMMEDIATE");assert!(db.try_exec(second).is_err());db.exec("ROLLBACK");
        }
    }
    {let online=InitialReader::open(&s,p).unwrap();assert!(online.authenticate_web("first.admin","correct horse battery staple").is_err());}
    {
        let owned=HeldDir::trusted_system(&s).unwrap();owned.exclusive().unwrap();let db=Builder::existing(&s.join("SYSTEM.sqlite3"));
        db.exec("PRAGMA foreign_keys=ON; PRAGMA synchronous=EXTRA; BEGIN IMMEDIATE");
        db.exec(&a);
        assert!(db.try_exec(&k.replace(field("synthetic_smb_field"),"31d6cfe0d16ae931b73c59d7e0c089c0")).is_err());
        db.exec(&w);db.exec(&k);db.exec(&o);db.exec("COMMIT");
        for stmt in [&a,&w,&k,&o]{assert!(db.try_exec(&stmt.replace("INSERT INTO","INSERT OR REPLACE INTO")).is_err());}
    }
    let online=InitialReader::open(&s,p).unwrap();
    assert_eq!(setup_receipt(&online,op,actor,[0x77;16],b"first.admin",b"files.smb",&sid()).unwrap(),Some((web,smb)));
    assert_eq!(setup_receipt(&online,op,[0xa0;16],[0x77;16],b"first.admin",b"files.smb",&sid()).unwrap(),None);
    for login in ["","guest","anonymous","files.smb","FIRST.ADMIN"]{assert!(online.authenticate_web(login,"correct horse battery staple").is_err());}
    assert!(online.authenticate_web("first.admin","").is_err());assert!(online.authenticate_web("first.admin","wrong").is_err());
    let m=online.authenticate_web("first.admin","correct horse battery staple").unwrap();assert_eq!(m.id().unwrap(),web);
    let other=Builder::existing(&s.join("SYSTEM.sqlite3"));
    assert!(online.authenticate_web_after("first.admin","correct horse battery staple",||{
        assert_ne!(unsafe{sqlite3_get_autocommit(online.conn.0)},0);other.exec("CREATE TABLE unqualified(x)");Ok(())
    }).is_err());assert!(m.id().is_err());other.exec("DROP TABLE unqualified");assert_eq!(m.id().unwrap(),web);
}

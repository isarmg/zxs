//! Ordinary synthetic SQLite fixture, not a first-setup writer or a network
//! credential. All fixed KDF material below comes from the public vector.
use super::*;
use crate::identity::AccountId;
use crate::system::reader::account::{Management,first_receipt,request};
const V:&str=include_str!("../sqlite/first-account-v1.txt");
fn field(k:&str)->&'static str{
    V.lines().find_map(|line|line.split_once('=').filter(|(n,_)|*n==k).map(|(_,v)|v)).unwrap()
}
fn inputs(profile:u8)->(&'static str,Management){
    match profile{1=>(field("profile1_sha"),Management::ReadAcl),2=>(field("profile2_sha"),Management::ReadAndWriteAcl),_=>unreachable!()}
}
fn rows(profile:u8)->(String,String,String){
    let op=field("first_op");let account=field("result_account");let approval=field("approval_ref");
    let actor=field("offline_actor");let login=field("login_hex");let salt=field("salt");let tag=field("verifier");
    let (sha,_)=inputs(profile);
    (format!("INSERT INTO account(account_id,birth_op,login,approval_ref,boot_profile) VALUES(X'{account}',X'{op}',X'{login}',X'{approval}',{profile});"),
     format!("INSERT INTO account_password(account_id,credential_op,generation,salt,verifier) VALUES(X'{account}',X'{op}',1,X'{salt}',X'{tag}');"),
     format!("INSERT INTO operation(op_id,kind,actor_id,request_sha,state) VALUES(X'{op}',2,X'{actor}',X'{sha}',1);"))
}
#[test]fn first_account_request_excludes_issued_results_and_matches_both_exact_choices(){
    let op=OperationId([0x44;16]);let init=OperationId([1;16]);let p=PoolId([4;16]);
    for profile in [1,2]{let (sha,scope)=inputs(profile);
        assert_eq!(hex(&request(p,init,op,[0x66;16],[0x77;16],scope,b"first.admin").unwrap()),sha);
        assert!(request(p,init,init,[0x66;16],[0x77;16],scope,b"first.admin").is_err());
        assert!(request(p,init,op,[0x66;16],[0x77;16],scope,b"First.Admin").is_err());
    }
}
#[test]
#[ignore="source-only real SQLite/Argon2id qualification, non-root storage principal"]
fn first_account_is_atomic_and_its_exact_credential_is_rechecked_after_the_unlocked_kdf(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=record(&t,InitState::Committed,"");
    let op=OperationId([0x44;16]);let actor=[0x66;16];let id=AccountId([0x55;16]);
    let (a,pw,receipt)=rows(2);
    {
        let selected=HeldDir::trusted_system(&s).unwrap();selected.exclusive().unwrap();
        let writer=Builder::existing(&s.join("SYSTEM.sqlite3"));
        writer.exec("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=EXTRA; BEGIN IMMEDIATE;");
        // A new identity and verifier without their original decision cannot
        // commit. The connection remains in the transaction until rollback.
        writer.exec(&a);writer.exec(&pw);assert!(writer.try_exec("COMMIT").is_err());writer.exec("ROLLBACK");
    }
    {
        let online=InitialReader::open(&s,p).unwrap();
        assert!(online.authenticate_first("first.admin","correct horse battery staple").is_err());
    }
    {
        let selected=HeldDir::trusted_system(&s).unwrap();selected.exclusive().unwrap();
        let writer=Builder::existing(&s.join("SYSTEM.sqlite3"));
        writer.exec("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=EXTRA; BEGIN IMMEDIATE;");
        assert!(writer.try_exec(&receipt).is_err()); // not even an in-transaction orphan receipt
        writer.exec(&a);writer.exec(&pw);writer.exec(&receipt);writer.exec("COMMIT");
        // Lost reply: cannot REPLACE the identity, verifier, or original action.
        for stmt in [&a,&pw,&receipt] {assert!(writer.try_exec(&stmt.replace("INSERT INTO","INSERT OR REPLACE INTO")).is_err());}
    }
    let online=InitialReader::open(&s,p).unwrap();
    assert_eq!(first_receipt(&online,op,actor,[0x77;16],Management::ReadAndWriteAcl,b"first.admin").unwrap(),
               Some((id,Management::ReadAndWriteAcl)));
    assert_eq!(first_receipt(&online,op,[0xaa;16],[0x77;16],Management::ReadAndWriteAcl,b"first.admin").unwrap(),None);
    assert_eq!(first_receipt(&online,op,actor,[0x77;16],Management::ReadAcl,b"first.admin").unwrap(),None);
    assert_eq!(first_receipt(&online,op,actor,[0x77;16],Management::ReadAndWriteAcl,b"different").unwrap(),None);
    assert!(online.authenticate_first("first.admin","wrong public fixture").is_err());
    assert!(online.authenticate_first("FIRST.ADMIN","correct horse battery staple").is_err());
    let manager=online.authenticate_first("first.admin","correct horse battery staple").unwrap();
    assert_eq!(manager.profile().unwrap(),(id,Management::ReadAndWriteAcl));
    // Deliberate qualification fault via a separate SQLite connection, never
    // raw-open or close the active DB inode. KDF has no live read transaction.
    let other=Builder::existing(&s.join("SYSTEM.sqlite3"));
    assert!(online.authenticate_first_after("first.admin","correct horse battery staple",||{
        assert_ne!(unsafe{sqlite3_get_autocommit(online.conn.0)},0);
        other.exec("CREATE TABLE unqualified(x);");Ok(())
    }).is_err());
    assert!(manager.profile().is_err());other.exec("DROP TABLE unqualified;");
    assert_eq!(manager.profile().unwrap(),(id,Management::ReadAndWriteAcl));
}

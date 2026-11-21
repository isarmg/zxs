//! Exact production SQL + independently generated public vectors, on a new
//! in-memory database only. No actual vault, secret, verifier provider or
//! terminal is exercised: vault currentness is a separate physical proof.
use super::*;
use crate::sqlite_install::request::Request;
use rusqlite::{Connection,TransactionBehavior};
use std::path::PathBuf;

fn field(name:&str)->&'static str{
    include_str!("shared-account-public-v1.txt").lines()
        .find_map(|line|line.split_once('=').filter(|(k,_)|*k==name).map(|(_,v)|v)).unwrap()
}
fn decode<const N:usize>(raw:&str)->[u8;N]{
    assert_eq!(raw.len(),2*N);
    std::array::from_fn(|i|u8::from_str_radix(&raw[2*i..2*i+2],16).unwrap())
}
fn vector<const N:usize>(name:&str)->[u8;N]{decode(field(name))}
fn setup_spec()->SetupSpec{
    SetupSpec{account:vector("account"),login:field("login").as_bytes().to_vec(),sid:vector("sid"),record:vector("first_record")}
}
fn seed()->(Connection,RustPlan,[u8;32]){
    let wire=include_str!("../smb-host/rust-install-v1.txt").lines().find_map(|s|s.strip_prefix("wire_hex=")).unwrap();
    let raw=decode::<197>(wire);
    let plan=RustPlan::decode(&raw).unwrap();let o=plan.origin;let actor=vector::<16>("actor");
    let approval=[72;16];
    let roots=[([73;16],PathBuf::from("/synthetic/S")),([74;16],"/synthetic/R".into())].into_iter()
        .chain(plan.folders().iter().enumerate().map(|(i,(f,_))|(f.volume,PathBuf::from(format!("/synthetic/F{i:02}")))))
        .collect::<Vec<_>>();
    let request=Request::new(o.init,actor,approval,1001,roots.iter().map(|(_,path)|path.clone()).collect()).unwrap();
    let init_sha=request.sha();
    let mut c=Connection::open_in_memory().unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;").unwrap();
    c.execute_batch(crate::sqlite_install::stage::SCHEMA).unwrap();
    let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    tx.execute("INSERT INTO main.operation VALUES(?1,1,?2,?3,0)",
        params![&o.init[..],&actor[..],&init_sha[..]]).unwrap();
    tx.execute("INSERT INTO main.pool_birth VALUES(1,?1,?2,?3,1001,?4)",
        params![&o.pool[..],&o.init[..],&approval[..],roots.len() as i64]).unwrap();
    for(i,(volume,path))in roots.iter().enumerate(){
        tx.execute("INSERT INTO main.volume_claim VALUES(?1,?2,?3,?4,?5,255,?6)",
            params![&o.init[..],i as i64,crate::sqlite_install::request::role(i),&volume[..],path.as_os_str().as_encoded_bytes(),
                &[(100+i) as u8;32][..]]).unwrap();
    }
    tx.execute("UPDATE main.operation SET state=1 WHERE op_id=?1",[&o.init[..]]).unwrap();
    tx.commit().unwrap();
    (c,plan,init_sha)
}
fn setup(plan:&RustPlan,sha:[u8;32])->RustChange{
    RustChange::for_fresh(plan,&RustOrder{actor:vector("actor"),approval:vector("setup_approval"),op:plan.origin.setup,
        role:Desired::Setup(setup_spec())},sha).unwrap()
}
fn reset(old:ActiveRustMeta)->RustChange{
    RustChange{actual:Change::Reset(Reset::new(old,vector("new_op"),vector("actor"),vector("reset_approval"),vector("new_record")).unwrap())}
}
fn key(c:&RustChange)->Binding{observed_binding([0x46;32],c.digest(),c.head(),c.previous()).unwrap()}
fn install(db:&mut Connection,plan:&RustPlan,sha:[u8;32])->ActiveRustMeta{
    let b=setup(plan,sha);let k=key(&b);
    let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    let staged=stage_shared(&tx,plan,&b,&k,Received::synthetic(k,0x42)).unwrap();
    tx.commit().unwrap();
    let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    activate(&tx,plan,&b,&k,&staged).unwrap();
    tx.commit().unwrap();b.target()
}
fn scalar(db:&Connection,sql:&str)->i64{db.query_row(sql,[],|r|r.get(0)).unwrap()}
fn attempt(db:&Connection,op:&Id)->i64{
    db.query_row("SELECT state FROM main.operation WHERE op_id=?1",[&op[..]],|r|r.get(0)).unwrap()
}
fn two(db:&Connection,w:&ActiveRustMeta){
    let mut q=db.prepare("SELECT role,credential_op,generation FROM main.active_credential ORDER BY role").unwrap();
    let rows=q.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Vec<u8>>(1)?,r.get::<_,i64>(2)?)))
        .unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert_eq!(rows,[(0,w.op.to_vec(),w.generation),(1,w.op.to_vec(),w.generation)]);
}

#[test]
fn shared_driver_public_digest_prefix_and_role_keys_match_independent_core_vector(){
    let spec=setup_spec();
    let s=ActiveRustMeta{pool:vector("pool"),init:vector("init"),setup:vector("setup"),instance:vector("instance"),bind:vector("bind"),
        init_sha:vector("init_sha"),descriptor_sha:vector("plan_sha"),account:spec.account,login:spec.login,sid:spec.sid,
        op:vector("setup"),generation:1,record:spec.record};
    let role=roles(s.account,vector("actor")).unwrap();
    assert_eq!([role.web,role.smb],[vector("web_actor"),vector("smb_actor")]);
    assert_eq!(s.record_prefix().unwrap(),vector("first_prefix"));
    let mut h=s.request(SETUP,vector("actor"),vector("setup_approval")).unwrap();h.update(s.record);
    assert_eq!(<[u8;32]>::from(h.finalize()),vector("setup_sha"));
    let reset=reset(s);
    assert_eq!(reset.digest(),vector("reset_sha"));
    let prefix=reset.head().record_prefix().unwrap();assert_eq!(prefix,vector("new_prefix"));
    let mut old_magic=prefix;old_magic[14]=b'1';
    assert!(helper_linux_evidence::first_smb::require_rust_v2_public_prefix(&old_magic).is_err());
}

#[test]
fn shared_driver_first_t1_closes_both_then_t2_publishes_same_epoch_and_first_roots(){
    let(mut db,plan,sha)=seed();let b=setup(&plan,sha);let k=key(&b);
    let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    let staged=stage_shared(&tx,&plan,&b,&k,Received::synthetic(k,0x42)).unwrap();
    tx.commit().unwrap();
    assert_eq!(attempt(&db,&b.head().op),0);
    assert_eq!(scalar(&db,"SELECT count(*) FROM main.active_credential"),0);
    assert_eq!(scalar(&db,"SELECT count(*) FROM main.root_share"),0);
    assert_eq!(scalar(&db,"SELECT count(*) FROM main.share_mode"),0);
    let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    let Original::Pending(original)=Birth::original(&tx,&plan).unwrap()else{panic!("not original PENDING");};
    assert_eq!(original.digest,b.digest());
    activate(&tx,&plan,&b,&k,&staged).unwrap();tx.commit().unwrap();
    two(&db,b.head());assert_eq!(attempt(&db,&b.head().op),1);
    assert_eq!(scalar(&db,"SELECT count(*) FROM main.root_share"),plan.folders().len() as i64);
    assert_eq!(scalar(&db,"SELECT count(*) FROM main.share_mode"),plan.folders().len() as i64);
}

#[test]
fn shared_driver_reset_t1_cuts_both_roles_without_fallback_and_t2_reopens_one_new_epoch(){
    let(mut db,plan,sha)=seed();let old=install(&mut db,&plan,sha);two(&db,&old);let r=reset(old.clone());let k=key(&r);
    let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    let staged=stage_shared(&tx,&plan,&r,&k,Received::synthetic(k,0x55)).unwrap();tx.commit().unwrap();
    assert_eq!(attempt(&db,&r.head().op),0);
    assert_eq!(scalar(&db,"SELECT count(*) FROM main.active_credential"),0);
    assert_eq!(scalar(&db,"SELECT count(*) FROM main.credential_epoch WHERE generation=1"),2);
    let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    let Original::Committed(b)=Birth::original(&tx,&plan).unwrap()else{panic!("missing first commit");};
    assert_eq!(select_reset(&tx,&b,r.head().op,2,0).unwrap().digest,r.digest());
    assert!(active_at(&tx,&plan).is_err());
    activate(&tx,&plan,&r,&k,&staged).unwrap();tx.commit().unwrap();
    two(&db,r.head());assert_eq!(attempt(&db,&r.head().op),1);
    assert!(r.head().account==old.account&&r.head().login==old.login);
    assert_eq!(scalar(&db,"SELECT count(*) FROM main.account_pair"),1);
    assert_eq!(scalar(&db,"SELECT count(*) FROM main.credential_epoch WHERE generation=2"),2);
}

#[test]
fn shared_driver_crossed_low_web_scope_rolls_back_and_interrupted_t2_remains_original_pending(){
    let(mut db,plan,sha)=seed();let old=install(&mut db,&plan,sha);let r=reset(old.clone());let k=key(&r);
    let cross=observed_binding([0x47;32],r.digest(),r.head(),r.previous()).unwrap();
    {
        let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        assert!(stage_shared(&tx,&plan,&r,&k,Received::synthetic(cross,0x55)).is_err());
        assert_eq!(scalar(&tx,"SELECT count(*) FROM main.credential_reset"),1); // partial before rollback
        tx.rollback().unwrap();
    }
    assert_eq!(scalar(&db,"SELECT count(*) FROM main.credential_reset"),0);
    two(&db,&old);assert_eq!(scalar(&db,"SELECT generation FROM main.web_password"),1);
    let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    let staged=stage_shared(&tx,&plan,&r,&k,Received::synthetic(k,0x55)).unwrap();tx.commit().unwrap();
    {
        let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        assert!(activate(&tx,&plan,&r,&cross,&staged).is_err());
        assert_eq!(attempt(&tx,&r.head().op),0);
        activate(&tx,&plan,&r,&k,&staged).unwrap();
        assert_eq!(attempt(&tx,&r.head().op),1);
        tx.rollback().unwrap(); // emulate nonconfirmation before commit
    }
    assert_eq!(attempt(&db,&r.head().op),0);
    assert_eq!(scalar(&db,"SELECT count(*) FROM main.active_credential"),0);
    assert_eq!(scalar(&db,"SELECT generation FROM main.web_password"),2);
}

#[test]
fn shared_driver_open_old_file_or_directory_birth_refuses_before_capture_and_again_in_t1(){
    use crate::smb_profile::first::Mode;
    for kind in [5,13]{
        let(mut db,plan,sha)=seed();let old=install(&mut db,&plan,sha);
        let (folder,share)=plan.folders().iter().find(|(f,_)|f.mode==Mode::ReadWrite).unwrap();
        let op=[0xd0u8;16];
        let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        if kind==5{
            let terminal="file-".to_owned()+&"1".repeat(32);
            tx.execute("INSERT INTO root_entry(create_op,share_id,setting_op,auth_op,volume_id,display,terminal,
                 entry_id,file_id,history_id,branch_id,ticket,parent_id)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,zeroblob(16))",
                params![&op[..],&share.share[..],&share.setting[..],&old.op[..],&folder.volume[..],b"file".as_slice(),
                    terminal.as_bytes(),&[0xd1u8;16][..],&[0xd2u8;16][..],&[0xd3u8;16][..],&[0xd4u8;16][..],&[0xd5u8;16][..]]).unwrap();
        }else{
            tx.execute("INSERT INTO directory_entry(create_op,directory_id,entry_id,volume_id,parent_id,display,share_id,
                setting_op,auth_op) VALUES(?1,?2,?3,?4,zeroblob(16),?5,?6,?7,?8)",
                params![&op[..],&[0xd1u8;16][..],&[0xd2u8;16][..],&folder.volume[..],b"folder".as_slice(),&share.share[..],
                    &share.setting[..],&old.op[..]]).unwrap();
        }
        let smb=roles(old.account,vector("actor")).unwrap().smb;
        tx.execute("INSERT INTO operation VALUES(?1,?2,?3,?4,0)",params![&op[..],kind,&smb[..],&[0xd6u8;32][..]]).unwrap();
        tx.commit().unwrap();
        let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        assert!(forbid_unsettled_births(&tx,old.op).is_err()); // the same actual pre-prompt helper
        let change=reset(old.clone());let k=key(&change);
        assert!(stage_shared(&tx,&plan,&change,&k,Received::synthetic(k,0x55)).is_err()); // repeat inside T1
        assert_eq!(scalar(&tx,"SELECT count(*) FROM main.credential_reset"),0);
        tx.rollback().unwrap();
        two(&db,&old);
    }
}

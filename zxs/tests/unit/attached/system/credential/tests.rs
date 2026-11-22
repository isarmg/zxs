use super::*;
use crate::{Role,VolumeId,fs::ComponentLimit,system::{BirthRow,ClaimRow,InitState,SCHEMA,init_request}};
use std::{ffi::CString,path::Path,ptr};

fn blob(b:&[u8])->String{format!("X'{}'",b.iter().map(|n|format!("{n:02x}")).collect::<String>())}
fn run(c:&Connection,sql:String){c.fixed(&CString::new(sql).unwrap()).unwrap();}
fn status(c:&Connection,op:[u8;16]){run(c,format!("UPDATE operation SET state=1 WHERE op_id={}",blob(&op)));}
const INIT:OperationId=OperationId([1;16]);
const POOL:PoolId=PoolId([2;16]);
const ACTOR:[u8;16]=[3;16];
const SETUP:[u8;16]=[4;16];
const RESET:[u8;16]=[5;16];
const APPROVAL:[u8;16]=[6;16];
const FIRST:[u8;16]=[7;16];
const NEXT:[u8;16]=[8;16];

fn memory()->(Connection,Catalog){
    let sql=CString::new(SCHEMA).unwrap();let mut raw=ptr::null_mut();
    check(unsafe{super::super::zxs_sql_template(sql.as_ptr(),&mut raw)}).unwrap();
    assert!(!raw.is_null());let db=Connection(raw);
    db.fixed(c"PRAGMA foreign_keys=ON").unwrap();
    db.fixed(c"PRAGMA trusted_schema=OFF").unwrap();
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    let paths=["/project/system","/project/recycle","/project/files"].map(Path::new);
    let request=init_request(INIT,ACTOR,APPROVAL,1000,&paths).unwrap();
    run(&db,format!("INSERT INTO operation VALUES({},1,{},{},0)",blob(&INIT.0),blob(&ACTOR),blob(&request)));
    run(&db,format!("INSERT INTO pool_birth VALUES(1,{},{},{},1000,3)",blob(&POOL.0),blob(&INIT.0),blob(&APPROVAL)));
    let rows=paths.into_iter().zip([Role::System,Role::Recycle,Role::File]).enumerate().map(|(i,(path,role))|{
        let id=VolumeId([80+i as u8;16]);let token=[90+i as u8;32];
        run(&db,format!("INSERT INTO volume_claim VALUES({},{i},{i},{},{},255,{})",
            blob(&INIT.0),blob(&id.0),blob(path.to_str().unwrap().as_bytes()),blob(&token)));
        ClaimRow{ordinal:i as u8,role,volume:id,path:path.to_path_buf(),limit:ComponentLimit::from_original(255).unwrap(),token}
    }).collect();
    status(&db,INIT.0);db.fixed(c"COMMIT").unwrap();
    let birth=BirthRow{pool:POOL,op:INIT,actor:ACTOR,request,state:InitState::Committed,approval:APPROVAL,uid:1000,count:3};
    (db,Catalog::checked(POOL,birth,rows).unwrap())
}
fn scope(c:&Catalog)->SharedScope<'static>{
    let mut sid=[10;28];sid[..12].copy_from_slice(b"\x01\x05\x00\x00\x00\x00\x00\x05\x15\x00\x00\x00");
    sid[24..].copy_from_slice(&1000u32.to_le_bytes());
    SharedScope{pool:POOL.0,init:INIT.0,setup:SETUP,instance:[11;16],bind:[12;16],
        account:[13;16],init_sha:c.birth.request,plan_sha:[14;32],original_actor:ACTOR,login:b"shared.operator",sid}
}
fn record(db:&Connection,op:[u8;16],rid:[u8;16]){
    run(db,format!("INSERT INTO smb_rust_revision VALUES({},{})",blob(&op),blob(&rid)));
}
fn setup(db:&Connection,scope:SharedScope<'_>,web:[u8;16]){
    setup_widths(db,scope,web,16,32)
}
fn setup_widths(db:&Connection,scope:SharedScope<'_>,web:[u8;16],salt:usize,tag:usize){
    let keys=scope.role_keys().unwrap();
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    run(db,format!("INSERT INTO account_pair VALUES(1,{},{},{},{},{},{},{},{},{},{})",
        blob(&SETUP),blob(&APPROVAL),blob(&scope.instance),blob(&scope.plan_sha),blob(&scope.account),
        blob(scope.login),blob(&web),blob(&keys.smb),blob(&scope.sid),blob(&scope.bind)));
    run(db,format!("INSERT INTO web_password VALUES(1,{},1,zeroblob({salt}),zeroblob({tag}))",blob(&SETUP)));
    record(db,SETUP,FIRST);
    run(db,format!("INSERT INTO operation VALUES({},2,{},{},0)",blob(&SETUP),blob(&ACTOR),
        blob(&scope.setup_request(APPROVAL,FIRST).unwrap())));
    db.fixed(c"COMMIT").unwrap();
}
fn reset(db:&Connection,scope:SharedScope<'_>,hash:[u8;32]){
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    run(db,format!("INSERT INTO credential_reset VALUES({},{},2,{},{})",
        blob(&RESET),blob(&SETUP),blob(&APPROVAL),blob(&scope.plan_sha)));
    run(db,format!("UPDATE web_password SET credential_op={},generation=2,salt={}",blob(&RESET),blob(&[19;16])));
    record(db,RESET,NEXT);
    run(db,format!("INSERT INTO operation VALUES({},9,{},{},0)",blob(&RESET),blob(&ACTOR),blob(&hash)));
    db.fixed(c"COMMIT").unwrap();
}

#[test]
fn actual_core_selects_only_the_exact_joint_metadata_before_any_secret_load(){
    let (db,c)=memory();let scope=scope(&c);let keys=scope.role_keys().unwrap();
    let first=PublicEpoch{op:SETUP,generation:1,record:FIRST};
    let next=PublicEpoch{op:RESET,generation:2,record:NEXT};
    assert!(OneAccount::confirmed(&db,&c).unwrap().is_none());
    assert!(OneAccount::qualified_web_metadata(&db,&c).unwrap().is_none());
    setup(&db,scope,keys.web);
    assert!(OneAccount::confirmed(&db,&c).unwrap().is_none());
    assert!(OneAccount::qualified_web_metadata(&db,&c).unwrap().is_none());
    status(&db,SETUP);
    let shared=OneAccount::confirmed(&db,&c).unwrap().unwrap();
    assert_eq!(shared.canonical.0,scope.account);assert_eq!(shared.web.0,keys.web);
    assert_eq!(shared.smb.0,keys.smb);assert_eq!(shared.login,"shared.operator");
    assert!(shared.current(&db,&c).unwrap()==Some(first));
    assert!(OneAccount::qualified_web(&db,&c).unwrap().is_some_and(|(_,e,_)|e==first));
    assert!(OneAccount::qualified_web_metadata(&db,&c).unwrap().is_some_and(|(a,e)|a.web.0==keys.web&&e==first));
    reset(&db,scope,scope.reset_request(APPROVAL,first,next).unwrap());
    assert!(shared.current(&db,&c).unwrap().is_none());
    assert!(OneAccount::qualified_web(&db,&c).unwrap().is_none());
    assert!(OneAccount::qualified_web_metadata(&db,&c).unwrap().is_none());
    status(&db,RESET);
    assert!(shared.current(&db,&c).unwrap()==Some(next));
    assert!(OneAccount::qualified_web(&db,&c).unwrap().is_some_and(|(_,e,_)|e==next));
    assert!(OneAccount::qualified_web_metadata(&db,&c).unwrap().is_some_and(|(a,e)|a.web.0==keys.web&&e==next));
}

#[test]
fn shared_account_rows_do_not_prove_wrong_role_key_or_bad_reset_preimage(){
    let (db,c)=memory();let origin=scope(&c);
    setup(&db,origin,[42;16]);status(&db,SETUP);
    assert!(OneAccount::confirmed(&db,&c).is_err()); // DDL is shape, not the checked original
    let (db,c)=memory();let origin=scope(&c);
    setup(&db,origin,origin.role_keys().unwrap().web);status(&db,SETUP);
    let shared=OneAccount::confirmed(&db,&c).unwrap().unwrap();
    reset(&db,origin,[101;32]);status(&db,RESET);
    assert!(shared.current(&db,&c).is_err());
    assert!(OneAccount::qualified_web(&db,&c).is_err());
    assert!(OneAccount::qualified_web_metadata(&db,&c).is_err());
}

#[test]
fn business_web_cut_rejects_the_same_bad_widths_as_login(){
    for (salt,tag) in [(0,32),(15,32),(17,32),(16,0),(16,31),(16,33)]{
        let (db,c)=memory();let origin=scope(&c);
        // In-memory schema fixture only: model invalid stored widths
        // without changing the canonical table, triggers, or joins.
        db.fixed(c"PRAGMA ignore_check_constraints=ON").unwrap();
        setup_widths(&db,origin,origin.role_keys().unwrap().web,salt,tag);
        db.fixed(c"PRAGMA ignore_check_constraints=OFF").unwrap();
        status(&db,SETUP);
        let active=PublicEpoch{op:SETUP,generation:1,record:FIRST};
        assert!(OneAccount::web_head(&db,&c).unwrap().is_some_and(|(_,e)|e==active));
        let business=OneAccount::qualified_web_metadata(&db,&c).err().unwrap();
        let login=OneAccount::qualified_web(&db,&c).err().unwrap();
        assert_eq!(business.kind(),login.kind());
        assert_eq!(business.to_string(),login.to_string());
    }
}

//! SQL interoperation only. No native SAM, secrets, daemon or SMB login is created.
use super::*;
use crate::identity::AccountId;
use crate::system::reader::{account::setup_state,credential::{setup_request,reset_request}};
const V:&str=include_str!("../sqlite/first-account-v1.txt");
fn field(k:&str)->&'static str{
    V.lines().find_map(|line|line.split_once('=').filter(|(n,_)|*n==k).map(|(_,v)|v)).unwrap()
}
fn bytes<const N:usize>(k:&str)->[u8;N]{
    let v=field(k);assert_eq!(v.len(),2*N);
    std::array::from_fn(|i|u8::from_str_radix(&v[i*2..i*2+2],16).unwrap())
}
fn sid()->[u8;28]{bytes("smb_sid")}
pub(super) fn rows()->[String;4]{
    let op=field("setup_op");let web=field("result_web_id");let smb=field("result_smb_id");let app=field("approval_ref");
    let wn=field("web_login_hex");let sn=field("smb_login_hex");let sid=field("smb_sid");
    let salt=field("web_salt");let tag=field("web_verifier");let inst=field("instance");let bound=field("setup_bound_sha");
    let actor=field("offline_actor");let sha=field("request_sha");
    [format!("INSERT INTO account_pair VALUES(1,X'{op}',X'{app}',X'{inst}',X'{bound}',X'{web}',X'{wn}',X'{smb}',X'{sn}',X'{sid}');"),
     format!("INSERT INTO web_password VALUES(1,X'{op}',1,X'{salt}',X'{tag}');"),
     format!("INSERT INTO operation VALUES(X'{op}',2,X'{actor}',X'{sha}',0);"),
     format!("UPDATE operation SET state=1 WHERE op_id=X'{op}' AND kind=2 AND state=0;")]
}
#[test]fn both_roles_and_real_native_instance_enter_distinct_public_requests(){
    let op=OperationId([0x44;16]);let init=OperationId([1;16]);let p=PoolId([4;16]);
    let instance=bytes("instance");let bound=bytes("setup_bound_sha");let actor=bytes("offline_actor");
    for(name,expected) in [("setup_bound","setup_bound_sha"),("smb_reset_bound","smb_reset_bound_sha")]{
        let mut h=crate::hash::Sha256::new();h.update(&bytes::<128>(name)).unwrap();assert_eq!(h.finish(),bytes(expected));
    }
    let req=|web:&[u8],smb:&[u8],sid:&[u8;28]|setup_request(p,init,op,actor,[0x77;16],instance,bound,web,smb,sid);
    assert_eq!(hex(&req(b"first.admin",b"files.smb",&sid()).unwrap()),field("request_sha"));
    assert_ne!(req(b"first.admin",b"files.smb",&sid()).unwrap(),req(b"files.smb",b"first.admin",&sid()).unwrap());
    for bad in [b"".as_slice(),b"guest",b"anonymous",b"first.admin",b"Files.Smb"]{assert!(req(b"first.admin",bad,&sid()).is_err());}
    let mut guest=sid();guest[24..].copy_from_slice(&501u32.to_le_bytes());assert!(req(b"admin",b"files",&guest).is_err());
    for (prefix,id,domain) in [("smb","result_smb_id",Some((instance,bytes("smb_reset_bound_sha")))),("web","result_web_id",None)]{
        let new=OperationId(bytes(&format!("{prefix}_reset_op")));
        let got=reset_request(p,init,op,new,actor,bytes("reset_approval"),AccountId(bytes(id)),op,1,domain).unwrap();
        assert_eq!(hex(&got),field(&format!("{prefix}_reset_sha")));
    }
}

// Helper's independently approved native evidence is deliberately NOT
// implemented by this SQL-only fixture. Exercise the identical query, types,
// one RETURNING, and committed boundary, not a made-up password verifier.
fn activate(db:&Builder,setup:bool,marker_sha:&[u8;32])->bool{
    let op=bytes::<16>(if setup{"setup_op"}else{"smb_reset_op"});
    let q=CString::new(include_str!("../../src/system/activate-native.sql")).unwrap();
    let s=db.0.prepare(&q).unwrap();
    for(i,v)in [bytes::<16>("pool").to_vec(),bytes::<16>("init_op").to_vec(),bytes::<16>("setup_op").to_vec(),
        op.to_vec(),bytes::<16>("offline_actor").to_vec(),bytes::<32>(if setup{"request_sha"}else{"smb_reset_sha"}).to_vec(),
        bytes::<16>(if setup{"approval_ref"}else{"reset_approval"}).to_vec(),bytes::<16>("instance").to_vec(),
        marker_sha.to_vec(),bytes::<16>("result_smb_id").to_vec(),b"files.smb".to_vec(),sid().to_vec()].iter().enumerate(){
        s.bind_bytes(1+i as i32,v).unwrap();
    }
    s.bind_integer(13,if setup{1}else{2}).unwrap();
    if !setup{s.bind_bytes(14,&bytes::<16>("setup_op")).unwrap();} // otherwise SQL NULL
    match unsafe{sqlite3_step(s.0)}{101=>false,100=>{assert_eq!(s.array::<16>(0).unwrap(),op);s.done().unwrap();true},x=>panic!("unexpected SQLite {x}")}
}
fn record_reset(db:&Builder,smb:bool){
    let (r,kind)=if smb{(1,7)}else{(0,8)};let p=if smb{"smb"}else{"web"};let reset=field(&format!("{p}_reset_op"));
    let old=field("setup_op");let actor=field("offline_actor");let ap=field("reset_approval");let req=field(&format!("{p}_reset_sha"));
    let bound=if smb{format!("X'{}'",field("smb_reset_bound_sha"))}else{"NULL".to_owned()};
    db.exec(&format!("INSERT INTO credential_reset VALUES(X'{reset}',{r},X'{old}',2,X'{ap}',{bound})"));
    if !smb{
        db.exec(&format!("UPDATE web_password SET credential_op=X'{reset}',generation=2,salt=salt,verifier=verifier WHERE credential_op=X'{old}'"));
    }
    db.exec(&format!("INSERT INTO operation VALUES(X'{reset}',{kind},X'{actor}',X'{req}',{})",if smb{0}else{1}));
}
// History SELECT does not restamp the current permission or operate on SAM.
fn reset_state(db:&Builder,op_smb:bool,role_smb:bool,storage_bound:Option<[u8;32]>)->Option<(i64,i64)>{
    let p=if op_smb{"smb"}else{"web"};let q=CString::new(include_str!("../../src/system/original-reset.sql")).unwrap();
    let s=db.0.prepare(&q).unwrap();
    for(i,b)in [bytes::<16>("pool").to_vec(),bytes::<16>("init_op").to_vec(),bytes::<16>("setup_op").to_vec(),
      bytes::<16>(&format!("{p}_reset_op")).to_vec(),bytes::<16>("offline_actor").to_vec(),
      bytes::<32>(&format!("{p}_reset_sha")).to_vec(),bytes::<16>("reset_approval").to_vec()].iter().enumerate(){
        s.bind_bytes(i as i32+1,b).unwrap();
    }
    s.bind_integer(8,if role_smb{1}else{0}).unwrap();
    s.bind_bytes(9,&bytes::<16>(if role_smb{"result_smb_id"}else{"result_web_id"})).unwrap();
    s.bind_bytes(10,&bytes::<16>("setup_op")).unwrap();s.bind_integer(11,2).unwrap();
    if let Some(d)=storage_bound{s.bind_bytes(12,&bytes::<16>("instance")).unwrap();s.bind_bytes(13,&d).unwrap();}
    match unsafe{sqlite3_step(s.0)}{101=>None,100=>{let r=(s.integer(0).unwrap(),s.integer(1).unwrap());s.done().unwrap();Some(r)},x=>panic!("unexpected SQLite {x}")}
}
pub(super) fn fixture_rotate_web(db:&Builder){
    db.exec("PRAGMA foreign_keys=ON; PRAGMA synchronous=EXTRA; BEGIN IMMEDIATE");record_reset(db,false);db.exec("COMMIT");
}
pub(super) fn fixture_rotate_smb(db:&Builder){
    db.exec("PRAGMA foreign_keys=ON; PRAGMA synchronous=EXTRA; BEGIN IMMEDIATE");record_reset(db,true);db.exec("COMMIT");
    db.exec("BEGIN IMMEDIATE");assert!(activate(db,false,&bytes("smb_reset_bound_sha")));db.exec("COMMIT");
}
#[test]
#[ignore="source-only qualification; synthetic separate SQL cuts. No native SAM or maintenance authorization."]
fn setup_is_pending_until_both_sources_are_ready_and_only_the_reset_role_expires(){
    use crate::system::reader::smb::SmbSession;
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=record(&t,InitState::Committed,"");
    let [a,w,pending,_]=rows();let op=OperationId(bytes("setup_op"));let actor=bytes("offline_actor");let approval=bytes("approval_ref");
    let instance=bytes("instance");let bound=bytes("setup_bound_sha");let(web,smb)=(AccountId(bytes("result_web_id")),AccountId(bytes("result_smb_id")));
    let db=Builder::existing(&s.join("SYSTEM.sqlite3"));db.exec("PRAGMA foreign_keys=ON; PRAGMA synchronous=EXTRA; BEGIN IMMEDIATE");
    db.exec(&a);assert!(db.try_exec(&pending).is_err());assert!(db.try_exec("COMMIT").is_err());db.exec("ROLLBACK");
    db.exec("BEGIN IMMEDIATE");db.exec(&a);db.exec(&w);assert!(db.try_exec(&pending.replace(",0);",",1);")).is_err());db.exec(&pending);db.exec("COMMIT");
    let online=InitialReader::open(&s,p).unwrap();
    let observe=|who|setup_state(&online,op,who,approval,instance,bound,b"first.admin",b"files.smb",&sid()).unwrap();
    assert_eq!(observe(actor),Some((false,web,smb)));assert_eq!(observe([0x99;16]),None);
    let login=||online.authenticate_web("first.admin","correct horse battery staple");
    let slot=||SmbSession::fixture_authenticated(&online,"files.smb");
    assert!(login().is_err());assert!(slot().is_err());
    db.exec("BEGIN IMMEDIATE");assert!(!activate(&db,true,&[0x99;32]));assert!(activate(&db,true,&bound));db.exec("COMMIT");
    assert_eq!(observe(actor),Some((true,web,smb)));let admin=login().unwrap();assert_eq!(admin.id().unwrap(),web);
    for wrong in ["","guest","anonymous","files.smb","FIRST.ADMIN"]{assert!(online.authenticate_web(wrong,"correct horse battery staple").is_err());}
    assert!(online.authenticate_web("first.admin","wrong").is_err());
    let old=slot().unwrap();let source=admin.publish_root(OperationId([0x81;16]),crate::VolumeId([0x13;16]),"documents").unwrap();
    admin.set_share_mode(OperationId([0x83;16]),source.share,source.op,account::share_mode::Mode::ReadOnly).unwrap();
    let _old_tree=old.connect(source.share).unwrap();
    db.exec("BEGIN IMMEDIATE");record_reset(&db,true);db.exec("COMMIT");
    assert!(slot().is_err());assert!(old.connect(source.share).is_err());assert_eq!(admin.id().unwrap(),web);
    let sb=Some(bytes("smb_reset_bound_sha"));assert_eq!(reset_state(&db,true,true,sb),Some((0,2)));
    assert_eq!(reset_state(&db,true,false,None),None);assert_eq!(reset_state(&db,true,true,Some(bound)),None);
    // A different authorization cannot acknowledge the pending native result.
    db.exec("BEGIN IMMEDIATE");assert!(!activate(&db,false,&bound));assert!(activate(&db,false,&bytes("smb_reset_bound_sha")));db.exec("COMMIT");
    assert!(old.connect(source.share).is_err());assert_eq!(reset_state(&db,true,true,sb),Some((1,2)));let renewed=slot().unwrap();assert!(renewed.connect(source.share).is_ok());
    assert_eq!(observe(actor),Some((true,web,smb))); // historical, not current session authority
    db.exec("BEGIN IMMEDIATE");record_reset(&db,false);db.exec("COMMIT");
    // Same known public Argon2 sample on purpose: it is the change
    // of credential origin/epoch that ends the old Web handle, not a bad test password.
    assert_eq!(reset_state(&db,false,false,None),Some((1,2)));assert_eq!(reset_state(&db,false,false,sb),None);
    assert!(admin.id().is_err());assert_eq!(login().unwrap().id().unwrap(),web);assert!(renewed.connect(source.share).is_ok());
    assert!(db.try_exec("SELECT * FROM main.smb_password").is_err()); // v7 has no SQL SMB verifier or legacy table
}

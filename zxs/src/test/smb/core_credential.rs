//! Public synthetic coordinates only. This fixture exercises the real DDL and
//! selection on an in-memory catalogue, not launcher/vault/password authority.
use super::RustPlan;
use crate::{OperationId,PoolId,Role as VolumeRole,VolumeId,fs::ComponentLimit};
use crate::system::{BirthRow,Catalog,ClaimRow,InitState,SCHEMA,init_request};
use crate::system::reader::{Connection,check,sqlite3_step,zxs_sql_template,credential::{Epoch,Pair,Role,setup_request}};
use std::{ffi::{CStr,CString},io,path::Path,ptr};

const POOL:PoolId=PoolId([0x11;16]);
const INIT:OperationId=OperationId([0x12;16]);
const SETUP:OperationId=OperationId([0x13;16]);
const ACTOR:[u8;16]=[0x14;16];
const APPROVAL:[u8;16]=[0x15;16];
const INSTANCE:[u8;16]=[0x16;16];
const BIND:[u8;16]=[0x17;16];
const WEB:[u8;16]=[0x18;16];
const SMB:[u8;16]=[0x19;16];
const FIRST:[u8;16]=[0x1a;16];
const RESET:OperationId=OperationId([0x1b;16]);
const NEXT:[u8;16]=[0x1c;16];
const RESET_APPROVAL:[u8;16]=[0x1d;16];

fn octets(hex:&str)->Vec<u8>{
    assert_eq!(hex.len()%2,0);
    (0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect()
}
fn literal(bytes:&[u8])->String{
    use std::fmt::Write;
    let mut out=String::from("X'");
    for byte in bytes{write!(&mut out,"{byte:02x}").unwrap();}
    out.push('\'');out
}
fn exec(db:&Connection,sql:&str)->io::Result<()>{db.fixed(&CString::new(sql).unwrap())}
fn on_memory()->(Connection,Catalog){
    let ddl=CString::new(SCHEMA).unwrap();
    let mut p=ptr::null_mut();
    check(unsafe{zxs_sql_template(ddl.as_ptr(),&mut p)}).unwrap();
    assert!(!p.is_null());
    let db=Connection(p);
    db.fixed(c"PRAGMA foreign_keys=ON").unwrap();
    let paths=["/fixtures/s","/fixtures/r","/fixtures/f"].map(Path::new);
    let actor=[8;16];let approval=[9;16];let uid=1000;
    let request=init_request(INIT,actor,approval,uid,&paths).unwrap();
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    exec(&db,&format!("INSERT INTO operation VALUES({},1,{},{},0)",
        literal(&INIT.0),literal(&actor),literal(&request))).unwrap();
    exec(&db,&format!("INSERT INTO pool_birth VALUES(1,{},{},{},{uid},3)",
        literal(&POOL.0),literal(&INIT.0),literal(&approval))).unwrap();
    let rows=paths.iter().zip([VolumeRole::System,VolumeRole::Recycle,VolumeRole::File]).enumerate().map(|(i,(p,role))|{
        let volume=VolumeId([0x31+i as u8;16]);let token=[0x34+i as u8;32];
        exec(&db,&format!("INSERT INTO volume_claim VALUES({},{i},{i},{},{},255,{})",
             literal(&INIT.0),literal(&volume.0),literal(p.to_str().unwrap().as_bytes()),literal(&token))).unwrap();
        ClaimRow{ordinal:i as u8,role,volume,path:p.to_path_buf(),limit:ComponentLimit::from_original(255).unwrap(),token}
    }).collect();
    exec(&db,&format!("UPDATE operation SET state=1 WHERE op_id={}",literal(&INIT.0))).unwrap();
    db.fixed(c"COMMIT").unwrap();
    let c=Catalog::checked(POOL,BirthRow{pool:POOL,op:INIT,actor,request,state:InitState::Committed,approval,uid,count:3},rows).unwrap();
    (db,c)
}
fn stage(db:&Connection,op:OperationId,kind:u8,digest:&[u8])->io::Result<()>{
    exec(db,&format!("INSERT INTO operation VALUES({},{kind},{},{},0)",literal(&op.0),literal(&ACTOR),literal(digest)))
}
fn vault(db:&Connection,op:OperationId,record:&[u8;16])->io::Result<()>{
    exec(db,&format!("INSERT OR REPLACE INTO smb_rust_revision VALUES({},{})",literal(&op.0),literal(record)))
}
enum Proof<'a>{Native,Rust{record:&'a [u8;16],prior:Option<&'a [u8;16]>}}
fn finish(db:&Connection,sql:&CStr,reset:bool,bound:&[u8],sid:&[u8],digest:&[u8],proof:Proof<'_>)->bool{
    let s=db.prepare(sql).unwrap();
    let op=if reset{RESET}else{SETUP};
    let approval=if reset{RESET_APPROVAL}else{APPROVAL};
    for (i,b) in [&POOL.0[..],&INIT.0,&SETUP.0,&op.0,&ACTOR,digest,&approval,&INSTANCE,bound,&SMB,b"files",sid]
        .iter().enumerate(){s.bind_bytes(i as i32+1,b).unwrap();}
    s.bind_integer(13,if reset{2}else{1}).unwrap();
    if reset{s.bind_bytes(14,&SETUP.0).unwrap();}
    // The native specimen only has 14 parameters, never the Rust proof.
    if let Proof::Rust{record,prior}=proof{
        s.bind_bytes(15,&BIND).unwrap();s.bind_bytes(16,record).unwrap();
        if let Some(prior)=prior{s.bind_bytes(17,prior).unwrap();}
    }
    match unsafe{sqlite3_step(s.0)}{
        101=>false,100=>{assert_eq!(s.array::<16>(0).unwrap(),op.0);s.done().unwrap();true},
        rc=>panic!("unexpected SQLite {rc}"),
    }
}
fn pair(db:&Connection,bind:Option<&[u8;16]>,bound:&[u8],sid:&[u8]){
    let bind=bind.map(|b|literal(b));
    let query=format!("INSERT INTO account_pair VALUES(1,{},{},{},{},{},{},{},{},{},{},{})",
        literal(&SETUP.0),literal(&APPROVAL),literal(&INSTANCE),literal(bound),literal(&WEB),literal(b"admin"),
        literal(&SMB),literal(b"files"),literal(sid),bind.is_some() as u8,bind.as_deref().unwrap_or("NULL"));
    exec(db,&query).unwrap();
    exec(db,&format!("INSERT INTO web_password VALUES(1,{},1,zeroblob(16),zeroblob(32))",literal(&SETUP.0))).unwrap();

}
fn roots(db:&Connection){
    let share=literal(&octets("b5203c873ae8c37befe82574a4f157c5"));let p=literal(&octets("e9653a5d2d051054c2496eb0a19d3672"));let m=literal(&octets("9b95192b020c378ba058a588d3026df2"));
    let file=literal(&[0x33;16]);let admin=literal(&WEB);let born=literal(&SETUP.0);
    let pub_sha=literal(&octets("b891cc491cf51eb88c83ecd829007fefc7244de1e36d0d29eaea8f09741714ef"));let mode_sha=literal(&octets("e4b29152e5cdbda1439a72ee2b0ea905f0440fecf9da480e22554c3a64677bf1"));
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    exec(db,&format!("INSERT INTO root_share VALUES({share},{p},{born},{file},X'446f6373')")).unwrap();
    exec(db,&format!("INSERT INTO operation VALUES({p},3,{admin},{pub_sha},1)")).unwrap();
    exec(db,&format!("INSERT INTO share_mode VALUES({m},{share},{p},{born},1,2)")).unwrap();
    exec(db,&format!("INSERT INTO operation VALUES({m},4,{admin},{mode_sha},1)")).unwrap();
    db.fixed(c"COMMIT").unwrap();
}
const PLAN_BYTES:&str="5a58532d525553542d534d422d49310011111111111111111111111111111111121212121212121212121212121212121616161616161616161616161616161617171717171717171717171717171717131313131313131313131313131313130b666978747572652e6c616e010418c00002110133333333333333333333333333333333020004446f63730006e59b9ee694b6";
const BOUND:&str="e06d3be19b79dbbc03395e297938370a1aa8f66f9c680b92db721823fb1e8e00";
const SID:&str="010500000000000515000000292929292929292929292929e8030000";
const SETUP_DIGEST:&str="b46c46d6043bc41c28c7de74f0d3443497512a20e98b0e5098ed7743932dee12";
const RESET_DIGEST:&str="2b944639c79347b9a16336dee7fbac2a4f3d3f6e843372294fdf8600a643d6b4";

#[test]
fn only_the_original_rust_record_and_its_plan_are_current(){
    let (db,c)=on_memory();
    let (bytes,bound,sid,setup,reset)=(octets(PLAN_BYTES),octets(BOUND),octets(SID),octets(SETUP_DIGEST),octets(RESET_DIGEST));
    let rust=RustPlan::parse(&bytes).unwrap();
    assert_eq!(rust.hostname(),"fixture.lan");
    let addr=|v:&str|v.parse::<std::net::SocketAddr>().unwrap();
    let local=addr("192.0.2.17:445");
    assert!(rust.listener_is(0,local));
    assert!(!rust.listener_is(1,local));
    assert!(rust.admits_addresses(local,addr("192.0.2.51:53119")));
    assert!(!rust.admits_addresses(local,addr("192.0.3.51:53119")));
    assert!(!rust.admits_addresses(local,addr("[::ffff:192.0.2.51]:53119")));
    assert!(!rust.admits_addresses(local,addr("192.0.2.0:53119")));
    assert!(!rust.admits_addresses(addr("192.0.2.18:445"),addr("192.0.2.51:53119")));
    let mut old_transport=bytes.clone();old_transport[..16].copy_from_slice(b"ZXS-SMB-FIRST-1\0");
    assert!(RustPlan::parse(&old_transport).is_err());
    assert!(RustPlan::parse(&bytes[..bytes.len()-1]).is_err());
    let mut wrong_name=bytes.clone();wrong_name[97]=b'F';
    assert!(RustPlan::parse(&wrong_name).is_err());
    let mut other_name=bytes;other_name[97]=b'g';
    let other_plan=RustPlan::parse(&other_name).unwrap();
    let helper_rust=CString::new(include_str!("../../system/activate-rust.sql")).unwrap();
    let helper_native=CString::new(include_str!("../../system/activate-native.sql")).unwrap();

    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    pair(&db,Some(&BIND),&bound,&sid);
    assert!(stage(&db,SETUP,2,&setup).is_err()); // no vault coordinate
    vault(&db,SETUP,&FIRST).unwrap();
    assert!(vault(&db,SETUP,&NEXT).is_err()); // INSERT OR REPLACE may not change an original
    stage(&db,SETUP,2,&setup).unwrap();
    assert!(Pair::confirmed(&db,&c).unwrap().is_none());
    assert!(!finish(&db,&helper_native,false,&bound,&sid,&setup,Proof::Native));
    assert!(!finish(&db,&helper_rust,false,&bound,&sid,&setup,Proof::Rust{record:&NEXT,prior:None}));
    assert!(finish(&db,&helper_rust,false,&bound,&sid,&setup,Proof::Rust{record:&FIRST,prior:None}));
    db.fixed(c"COMMIT").unwrap();

    let selected=Pair::confirmed(&db,&c).unwrap().unwrap();
    assert!(!selected.is_native());
    assert!(selected.epoch(&db,&c,Role::Web).unwrap().unwrap().op==SETUP);
    assert_eq!(selected.active_rust_at(&db,&c).unwrap().unwrap().record,FIRST);
    assert!(rust.active_at(&db,&c).is_err()); // real planned list not installed yet
    let bound_digest:[u8;32]=bound.as_slice().try_into().unwrap();
    let probe=||rust.preflight_at(&db,&c,[8;16],bound_digest);
    assert!(probe().is_err()); // no valid pre-view projection until original first shares are complete
    roots(&db);
    let children=rust.plan.logical_after(&db,&c,b"").unwrap();
    assert_eq!(children.iter().map(|r|(r.name,r.number,r.mode.map(|s|s.mode))).collect::<Vec<_>>(),
        vec![("Docs",3,Some(crate::system::reader::account::share_mode::Mode::ReadWrite)),("回收",2,None)]);
    assert_eq!(rust.plan.logical_after(&db,&c,b"Docs").unwrap().iter().next().unwrap().name,"回收");
    let old=rust.active_at(&db,&c).unwrap();
    assert!(old.epoch==(Epoch{op:SETUP,generation:1}));
    assert_eq!((old.bind,old.record),(BIND,FIRST));
    assert!(other_plan.active_at(&db,&c).is_err());
    let early=probe().unwrap();
    assert_eq!(early.epoch.record,FIRST);assert_eq!(early.epoch.generation,1);
    assert_eq!(early.origin,[POOL.0,INIT.0,INSTANCE,BIND,SETUP.0]);
    assert_eq!(early.ordinary.len(),1);assert_eq!(early.ordinary[0].ordinal,2);
    assert_eq!(early.ordinary[0].path,Path::new("/fixtures/f"));
    assert_eq!(early.ordinary[0].marker,c.expected_marker(2).unwrap());
    assert_eq!(early.recycle.ordinal,1);assert_eq!(early.recycle.marker,c.expected_marker(1).unwrap());
    assert!(rust.preflight_at(&db,&c,ACTOR,bound_digest).is_err()); // SETUP actor is not the original INIT actor
    assert!(rust.preflight_at(&db,&c,[8;16],[0x77;32]).is_err());

    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    let reset_row=|d:&[u8]|format!("INSERT INTO credential_reset VALUES({},1,{},2,{},{})",
        literal(&RESET.0),literal(&SETUP.0),literal(&RESET_APPROVAL),literal(d));
    assert!(exec(&db,&reset_row(&[0x66;32])).is_err());
    exec(&db,&reset_row(&bound)).unwrap();
    assert!(vault(&db,RESET,&FIRST).is_err());
    vault(&db,RESET,&NEXT).unwrap();
    stage(&db,RESET,7,&reset).unwrap();
    db.fixed(c"COMMIT").unwrap();
    assert!(selected.active_rust_at(&db,&c).unwrap().is_none());
    assert!(rust.active_at(&db,&c).is_err());
    assert!(probe().is_err()); // latest pending reset cannot fall back to the old metadata
    assert!(selected.epoch(&db,&c,Role::Web).unwrap().unwrap().op==SETUP);

    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    assert!(!finish(&db,&helper_rust,true,&bound,&sid,&reset,Proof::Rust{record:&NEXT,prior:Some(&NEXT)}));
    assert!(finish(&db,&helper_rust,true,&bound,&sid,&reset,Proof::Rust{record:&NEXT,prior:Some(&FIRST)}));
    db.fixed(c"COMMIT").unwrap();
    let current=rust.active_at(&db,&c).unwrap();
    assert!(current.epoch==(Epoch{op:RESET,generation:2}));
    assert_eq!(current.record,NEXT);
    assert_eq!(current.smb.0,old.smb.0);
    let late=probe().unwrap();assert_eq!((late.epoch.record,late.epoch.generation),(NEXT,2));
    assert_eq!((early.ordinary,early.recycle),(late.ordinary,late.recycle));
}

#[test]
fn native_record_cannot_be_reserved_or_activated_as_rust(){
    let (db,c)=on_memory();
    let bound=octets(BOUND);let sid:[u8;28]=octets(SID).try_into().unwrap();
    let digest=setup_request(POOL,INIT,SETUP,ACTOR,APPROVAL,INSTANCE,bound.clone().try_into().unwrap(),b"admin",b"files",&sid).unwrap();
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    pair(&db,None,&bound,&sid);
    assert!(vault(&db,SETUP,&FIRST).is_err());
    stage(&db,SETUP,2,&digest).unwrap();
    let native=CString::new(include_str!("../../system/activate-native.sql")).unwrap();
    let rust=CString::new(include_str!("../../system/activate-rust.sql")).unwrap();
    assert!(!finish(&db,&rust,false,&bound,&sid,&digest,Proof::Rust{record:&FIRST,prior:None}));
    assert!(finish(&db,&native,false,&bound,&sid,&digest,Proof::Native));
    db.fixed(c"COMMIT").unwrap();
    let pair=Pair::confirmed(&db,&c).unwrap().unwrap();
    assert!(pair.is_native());
    assert!(pair.active_rust_at(&db,&c).unwrap().is_none());
}

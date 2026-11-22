//! Public synthetic coordinates only. This fixture exercises the real DDL and
//! selection on an in-memory catalogue, not launcher/vault/password authority.
use super::RustPlan;
use crate::{OperationId,PoolId,Role as VolumeRole,VolumeId,fs::ComponentLimit};
use crate::system::{BirthRow,Catalog,ClaimRow,InitState,SCHEMA,init_request};
use crate::system::reader::{Connection,check,zxs_sql_template,credential::{Epoch,Pair}};
use crate::system::shared_account::{PublicEpoch,SharedScope};
use std::{ffi::CString,io,path::Path,ptr};

const POOL:PoolId=PoolId([0x11;16]);
const INIT:OperationId=OperationId([0x12;16]);
const SETUP:OperationId=OperationId([0x13;16]);
const ACTOR:[u8;16]=[0x14;16];
const APPROVAL:[u8;16]=[0x15;16];
const INSTANCE:[u8;16]=[0x16;16];
const BIND:[u8;16]=[0x17;16];
const ACCOUNT:[u8;16]=[0x18;16];
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
    let actor=ACTOR;let approval=[9;16];let uid=1000;
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
fn activate(db:&Connection,op:OperationId)->io::Result<()>{
    exec(db,&format!("UPDATE operation SET state=1 WHERE op_id={} AND state=0",literal(&op.0)))
}
fn spec(c:&Catalog,bound:[u8;32],sid:[u8;28])->SharedScope<'static>{
    SharedScope{pool:POOL.0,init:INIT.0,setup:SETUP.0,instance:INSTANCE,bind:BIND,account:ACCOUNT,
        init_sha:c.birth.request,plan_sha:bound,original_actor:ACTOR,login:b"fixture.account",sid}
}
fn pair(db:&Connection,s:&SharedScope<'_>){
    let keys=s.role_keys().unwrap();
    let query=format!("INSERT INTO account_pair VALUES(1,{},{},{},{},{},{},{},{},{},{})",
        literal(&SETUP.0),literal(&APPROVAL),literal(&INSTANCE),literal(&s.plan_sha),literal(&ACCOUNT),
        literal(s.login),literal(&keys.web),literal(&keys.smb),literal(&s.sid),literal(&BIND));
    exec(db,&query).unwrap();
    exec(db,&format!("INSERT INTO web_password VALUES(1,{},1,zeroblob(16),zeroblob(32))",literal(&SETUP.0))).unwrap();
}
fn roots(db:&Connection,web:[u8;16]){
    let share=literal(&octets("b5203c873ae8c37befe82574a4f157c5"));let p=literal(&octets("e9653a5d2d051054c2496eb0a19d3672"));let m=literal(&octets("9b95192b020c378ba058a588d3026df2"));
    let file=literal(&[0x33;16]);let admin=literal(&web);let born=literal(&SETUP.0);
    let pub_sha=literal(&octets("8e05fe3bd5b8a31a79bc5f6c230d827dc5da29445e7da45a836f03b31dd0a4ce"));let mode_sha=literal(&octets("0852efed9978ac8bd4b92a73a1ae897e3486a5a6920a88c7299a2289b63f2b66"));
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
#[test]
fn only_the_original_rust_record_and_its_plan_are_current(){
    let (db,c)=on_memory();
    let bytes=octets(PLAN_BYTES);
    let bound:[u8;32]=octets(BOUND).try_into().unwrap();
    let sid:[u8;28]=octets(SID).try_into().unwrap();
    let scope=spec(&c,bound,sid);
    let setup=scope.setup_request(APPROVAL,FIRST).unwrap();
    let first=PublicEpoch{op:SETUP.0,generation:1,record:FIRST};
    let next=PublicEpoch{op:RESET.0,generation:2,record:NEXT};
    let reset=scope.reset_request(RESET_APPROVAL,first,next).unwrap();
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
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    pair(&db,&scope);
    assert!(stage(&db,SETUP,2,&setup).is_err()); // no vault coordinate
    vault(&db,SETUP,&FIRST).unwrap();
    assert!(vault(&db,SETUP,&NEXT).is_err()); // INSERT OR REPLACE may not change an original
    stage(&db,SETUP,2,&setup).unwrap();
    assert!(Pair::confirmed(&db,&c).unwrap().is_none());
    activate(&db,SETUP).unwrap();
    db.fixed(c"COMMIT").unwrap();

    let selected=Pair::confirmed(&db,&c).unwrap().unwrap();
    assert!(Pair::qualified_web(&db,&c).unwrap().is_some_and(|(_,epoch,_)|epoch==first));
    assert_eq!(selected.active_rust_at(&db,&c).unwrap().unwrap().record,FIRST);
    assert!(rust.active_at(&db,&c).is_err()); // real planned list not installed yet
    let probe=||rust.preflight_at(&db,&c,ACTOR,bound);
    assert!(probe().is_err()); // no valid pre-view projection until original first shares are complete
    roots(&db,scope.role_keys().unwrap().web);
    let children=rust.plan.logical_after(&db,&c,b"").unwrap();
    assert_eq!(children.iter().map(|r|(r.name,r.number,r.mode.map(|s|s.mode))).collect::<Vec<_>>(),
        vec![("Docs",3,Some(crate::system::reader::account::share_mode::Mode::ReadWrite)),("回收",2,None)]);
    assert_eq!(rust.plan.logical_after(&db,&c,b"Docs").unwrap().iter().next().unwrap().name,"回收");
    let old=rust.active_at(&db,&c).unwrap();
    assert!(old.epoch==(Epoch{op:SETUP,generation:1}));
    assert_eq!((old.bind,old.record),(BIND,FIRST));
    assert!(other_plan.active_at(&db,&c).is_err());
    let early=probe().unwrap();
    assert_eq!(early.epoch.account,ACCOUNT);assert_eq!(early.epoch.record,FIRST);assert_eq!(early.epoch.generation,1);
    assert_eq!(early.origin,[POOL.0,INIT.0,INSTANCE,BIND,SETUP.0]);
    assert_eq!(early.ordinary.len(),1);assert_eq!(early.ordinary[0].ordinal,2);
    assert_eq!(early.ordinary[0].path,Path::new("/fixtures/f"));
    assert_eq!(early.ordinary[0].marker,c.expected_marker(2).unwrap());
    assert_eq!(early.recycle.ordinal,1);assert_eq!(early.recycle.marker,c.expected_marker(1).unwrap());
    assert!(rust.preflight_at(&db,&c,[8;16],bound).is_err()); // wrong original INIT actor
    assert!(rust.preflight_at(&db,&c,ACTOR,[0x77;32]).is_err());

    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    let reset_row=|d:&[u8]|format!("INSERT INTO credential_reset VALUES({},{},2,{},{})",
        literal(&RESET.0),literal(&SETUP.0),literal(&RESET_APPROVAL),literal(d));
    assert!(exec(&db,&reset_row(&[0x66;32])).is_err());
    exec(&db,&reset_row(&bound)).unwrap();
    assert!(vault(&db,RESET,&FIRST).is_err());
    vault(&db,RESET,&NEXT).unwrap();
    exec(&db,&format!("UPDATE web_password SET credential_op={},generation=2,salt={}",literal(&RESET.0),literal(&[57;16]))).unwrap();
    stage(&db,RESET,9,&reset).unwrap();
    db.fixed(c"COMMIT").unwrap();
    assert!(selected.active_rust_at(&db,&c).unwrap().is_none());
    assert!(rust.active_at(&db,&c).is_err());
    assert!(probe().is_err()); // latest pending reset cannot fall back to the old metadata
    assert!(Pair::qualified_web(&db,&c).unwrap().is_none());

    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    activate(&db,RESET).unwrap();
    db.fixed(c"COMMIT").unwrap();
    let current=rust.active_at(&db,&c).unwrap();
    assert!(current.epoch==(Epoch{op:RESET,generation:2}));
    assert_eq!(current.record,NEXT);
    assert_eq!((current.smb,current.canonical),(old.smb,old.canonical));
    assert!(Pair::qualified_web(&db,&c).unwrap().is_some_and(|(_,epoch,_)|epoch==next));
    let late=probe().unwrap();assert_eq!((late.epoch.record,late.epoch.generation),(NEXT,2));
    assert_eq!((early.ordinary,early.recycle),(late.ordinary,late.recycle));
}

#[test]
fn management_navigation_is_current_parent_scoped_and_byte_ordered(){
    use crate::{DirectoryId,FileId,hash::Sha256,identity::ShareId,
        ordinary::portable_birth::Birth,system::reader::account::navigation::{Directory,Entry,memory_navigation}};
    let (db,c)=on_memory();
    let scope=spec(&c,octets(BOUND).try_into().unwrap(),octets(SID).try_into().unwrap());
    let keys=scope.role_keys().unwrap();
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    pair(&db,&scope);vault(&db,SETUP,&FIRST).unwrap();
    stage(&db,SETUP,2,&scope.setup_request(APPROVAL,FIRST).unwrap()).unwrap();activate(&db,SETUP).unwrap();
    db.fixed(c"COMMIT").unwrap();
    roots(&db,keys.web);

    let share=ShareId(octets("b5203c873ae8c37befe82574a4f157c5").try_into().unwrap());
    let mode:[u8;16]=octets("9b95192b020c378ba058a588d3026df2").try_into().unwrap();
    let volume=VolumeId([0x33;16]);let root=[0;16];
    let id=|kind:u8,n:u8|{let mut out=[0x81;16];out[5]=n;out[15]=kind;out};
    let write=|x:String|exec(&db,&x).unwrap_or_else(|e|panic!("{x}: {e}"));
    let operation=|op:[u8;16],kind:u8,actor:[u8;16],hash:[u8;32],state:u8|write(format!(
        "INSERT INTO operation VALUES({},{kind},{},{},{state})",literal(&op),literal(&actor),literal(&hash)));
    let prefix=|domain:&[u8],op:[u8;16]|{
        let mut h=Sha256::new();h.update(domain).unwrap();
        for bytes in [POOL.0,op,keys.smb,SETUP.0]{h.update(&bytes).unwrap();}
        h.update(&1i64.to_be_bytes()).unwrap();
        h
    };
    let bytes=|h:&mut Sha256,text:&str|{h.update(&[text.len() as u8]).unwrap();h.update(text.as_bytes()).unwrap();};
    let make_dir=|n:u8,parent:[u8;16],name:&str,confirm:bool|{
        let op=id(0,n);let dir=id(1,n);let member=id(2,n);
        write(format!("INSERT INTO directory_entry VALUES({},{},{},{},{},{},{},{},{})",
            literal(&op),literal(&dir),literal(&member),literal(&volume.0),literal(&parent),literal(name.as_bytes()),
            literal(&share.0),literal(&mode),literal(&SETUP.0)));
        let mut h=prefix(b"ZXS-SQLITE-1/CREATE-DIRECTORY\0",op);
        for value in [share.0,mode,volume.0,parent,dir,member]{h.update(&value).unwrap();}
        bytes(&mut h,name);operation(op,13,keys.smb,h.finish(),0);
        if confirm{activate(&db,OperationId(op)).unwrap();}
        DirectoryId(dir)
    };
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    let parent=make_dir(0,root,"Projects",true);
    let pending=make_dir(254,root,"Pending",false);
    for n in 1..=34{make_dir(n,parent.0,&format!("Folder-{n:02}"),true);}
    let leaf=FileId(id(4,1));
    let file=OperationId(id(3,1));
    let mut h=prefix(b"ZXS-SQLITE-1/CREATE-FILE-IN-PARENT\0",file.0);
    for value in [share.0,mode,volume.0,parent.0]{h.update(&value).unwrap();}
    bytes(&mut h,"Notes.txt");
    write(format!("INSERT INTO root_entry VALUES({},{},{},{},{},{},{},{},{},{},{},{},{})",
        literal(&file.0),literal(&share.0),literal(&mode),literal(&SETUP.0),literal(&volume.0),literal(b"Notes.txt"),
        literal(Birth::placed_container("Notes.txt",leaf).unwrap().to_bytes()),literal(&id(5,1)),literal(&leaf.0),
        literal(&id(6,1)),literal(&id(7,1)),literal(&id(8,1)),literal(&parent.0)));
    operation(file.0,5,keys.smb,h.finish(),0);activate(&db,file).unwrap();
    db.fixed(c"COMMIT").unwrap();

    let read=|at,after|memory_navigation(&db,&c,share,at,after);
    let (_,root_page,_)=read(None,None).unwrap();
    assert_eq!(root_page,vec![Entry::Directory(Directory{id:parent,name:"Projects".into()})]);
    assert!(read(Some(pending),None).is_err());
    assert!(memory_navigation(&db,&c,ShareId(id(42,0)),None,None).is_err());
    let (crumbs,first,next)=read(Some(parent),None).unwrap();
    assert_eq!(crumbs,vec![Directory{id:parent,name:"Projects".into()}]);assert_eq!(first.len(),32);
    assert_eq!(first[31],Entry::Directory(Directory{id:DirectoryId(id(1,32)),name:"Folder-32".into()}));
    let next=next.as_ref();
    assert_eq!(read(None,next).err().unwrap().kind(),io::ErrorKind::WouldBlock);
    let (_,last,next_page)=read(Some(parent),next).unwrap();
    assert_eq!(last,vec![
        Entry::Directory(Directory{id:DirectoryId(id(1,33)),name:"Folder-33".into()}),
        Entry::Directory(Directory{id:DirectoryId(id(1,34)),name:"Folder-34".into()}),
        Entry::File{id:leaf,name:"Notes.txt".into()},
    ]);
    assert!(next_page.is_none());

    let op=id(9,0);let old=id(0,0);let member=id(2,0);
    let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/RENAME-DIRECTORY\0").unwrap();
    for value in [POOL.0,old,volume.0,root,parent.0,member,op,old,share.0,mode,SETUP.0,keys.smb]{h.update(&value).unwrap();}
    h.update(&1i64.to_be_bytes()).unwrap();h.update(&1i64.to_be_bytes()).unwrap();
    bytes(&mut h,"Projects");bytes(&mut h,"Renamed");
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    write(format!("INSERT INTO directory_rename VALUES({},{},{},1,{},{},{},{},{})",
        literal(&op),literal(&parent.0),literal(&old),literal(b"Projects"),literal(b"Renamed"),
        literal(&share.0),literal(&mode),literal(&SETUP.0)));
    operation(op,16,keys.smb,h.finish(),0);
    db.fixed(c"COMMIT").unwrap();
    assert!(read(None,None).unwrap().1.is_empty());
    assert_eq!(read(Some(parent),None).err().unwrap().kind(),io::ErrorKind::WouldBlock);
    activate(&db,OperationId(op)).unwrap();
    assert_eq!(read(Some(parent),next).err().unwrap().kind(),io::ErrorKind::WouldBlock);
    assert_eq!(read(Some(parent),None).unwrap().0,vec![Directory{id:parent,name:"Renamed".into()}]);

    activate(&db,OperationId(id(0,254))).unwrap();
    let disabled=id(10,0);db.fixed(c"BEGIN IMMEDIATE").unwrap();
    write(format!("INSERT INTO share_mode VALUES({},{},{},{},2,0)",
        literal(&disabled),literal(&share.0),literal(&mode),literal(&SETUP.0)));
    operation(disabled,4,keys.web,[0x64;32],1);
    db.fixed(c"COMMIT").unwrap();
    assert_eq!(read(Some(parent),None).err().unwrap().kind(),io::ErrorKind::NotFound);

    let first=PublicEpoch{op:SETUP.0,generation:1,record:FIRST};
    let second=PublicEpoch{op:RESET.0,generation:2,record:NEXT};
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    write(format!("INSERT INTO credential_reset VALUES({},{},2,{},{})",literal(&RESET.0),literal(&SETUP.0),
        literal(&RESET_APPROVAL),literal(&scope.plan_sha)));
    write(format!("UPDATE web_password SET credential_op={},generation=2,salt={}",
        literal(&RESET.0),literal(&[57;16])));
    vault(&db,RESET,&NEXT).unwrap();
    stage(&db,RESET,9,&scope.reset_request(RESET_APPROVAL,first,second).unwrap()).unwrap();
    db.fixed(c"COMMIT").unwrap();
    assert_eq!(read(None,None).err().unwrap().kind(),io::ErrorKind::PermissionDenied);
    activate(&db,RESET).unwrap();

    // A different committed share on this same logical disk is not an
    // installer share merely because it has the current RW mode.
    let other=ShareId(id(13,0));let published=id(11,0);let new_mode=id(12,0);
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    write(format!("INSERT INTO root_share VALUES({},{},{},{},{})",
        literal(&other.0),literal(&published),literal(&RESET.0),literal(&volume.0),literal(b"Other")));
    operation(published,3,keys.web,[0x65;32],1);
    write(format!("INSERT INTO share_mode VALUES({},{},{},{},1,2)",
        literal(&new_mode),literal(&other.0),literal(&published),literal(&RESET.0)));
    operation(new_mode,4,keys.web,[0x66;32],1);
    db.fixed(c"COMMIT").unwrap();
    assert_eq!(crate::system::reader::account::share_mode::find(&db,&c,other).unwrap().unwrap().mode,
        crate::system::reader::account::share_mode::Mode::ReadWrite);
    assert_eq!(memory_navigation(&db,&c,other,None,None).err().unwrap().kind(),io::ErrorKind::NotFound);
}

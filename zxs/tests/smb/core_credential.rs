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
    let share=literal(&octets("c94640448309832bfd618d007462ae28"));let p=literal(&octets("f91393b8c5f08af0e12da493a1845556"));let m=literal(&octets("3bca408a4df417ff75ff7300c664ec3c"));
    let file=literal(&[0x33;16]);let admin=literal(&web);let born=literal(&SETUP.0);
    let pub_sha=literal(&octets("aa3477e46aa7d2af94f4383e7a9b423a0cccd0574e9821e9e8f0503b9588bd30"));let mode_sha=literal(&octets("9609b5a84ecd7ff7bb37b513ba03ea93bb272753e5d283aba547b6eaadaafd32"));
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    exec(db,&format!("INSERT INTO root_share VALUES({share},{p},{born},{file},X'446f6373')")).unwrap();
    exec(db,&format!("INSERT INTO operation VALUES({p},3,{admin},{pub_sha},1)")).unwrap();
    exec(db,&format!("INSERT INTO share_mode VALUES({m},{share},{p},{born},1,2)")).unwrap();
    exec(db,&format!("INSERT INTO operation VALUES({m},4,{admin},{mode_sha},1)")).unwrap();
    db.fixed(c"COMMIT").unwrap();
}
const PLAN_BYTES:&str="5a58532d525553542d534d422d49320011111111111111111111111111111111121212121212121212121212121212121616161616161616161616161616161617171717171717171717171717171717131313131313131313131313131313130b666978747572652e6c616e0133333333333333333333333333333333020004446f63730006e59b9ee694b6";
const BOUND:&str="17e08c7c898fdfeef4da8c07f05191002ea4720086719cfed85a707b7c21b764";
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
    let mut old_layout=bytes.clone();
    let after_host=97+usize::from(old_layout[96]);
    old_layout.splice(after_host..after_host,[1,4,24,192,0,2,17]);
    assert!(RustPlan::parse(&old_layout).is_err()); // never interpret endpoint fields in I2
    old_layout[14]=b'1';
    assert!(RustPlan::parse(&old_layout).is_err()); // an original I1 plan has no fallback
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
        label_text::Label,ordinary::portable_birth::Birth,system::reader::account::{
            file_labels::Selected,navigation::{Directory,Entry,memory_label_check,memory_label_navigation,memory_navigation}}};
    let (db,c)=on_memory();
    let scope=spec(&c,octets(BOUND).try_into().unwrap(),octets(SID).try_into().unwrap());
    let keys=scope.role_keys().unwrap();
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    pair(&db,&scope);vault(&db,SETUP,&FIRST).unwrap();
    stage(&db,SETUP,2,&scope.setup_request(APPROVAL,FIRST).unwrap()).unwrap();activate(&db,SETUP).unwrap();
    db.fixed(c"COMMIT").unwrap();
    roots(&db,keys.web);

    let share=ShareId(octets("c94640448309832bfd618d007462ae28").try_into().unwrap());
    let mode:[u8;16]=octets("3bca408a4df417ff75ff7300c664ec3c").try_into().unwrap();
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
    let make_file=|n:u8,name:&str|{
        let leaf=FileId(id(4,n));
        let file=OperationId(id(3,n));
        let mut h=prefix(b"ZXS-SQLITE-1/CREATE-FILE-IN-PARENT\0",file.0);
        for value in [share.0,mode,volume.0,parent.0]{h.update(&value).unwrap();}
        bytes(&mut h,name);
        write(format!("INSERT INTO root_entry VALUES({},{},{},{},{},{},{},{},{},{},{},{},{})",
            literal(&file.0),literal(&share.0),literal(&mode),literal(&SETUP.0),literal(&volume.0),literal(name.as_bytes()),
            literal(Birth::placed_container(name,leaf).unwrap().to_bytes()),literal(&id(5,n)),literal(&leaf.0),
            literal(&id(6,n)),literal(&id(7,n)),literal(&id(8,n)),literal(&parent.0)));
        operation(file.0,5,keys.smb,h.finish(),0);activate(&db,file).unwrap();
        (leaf,file)
    };
    let (leaf,file)=make_file(1,"Notes.txt");
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
        Entry::File{id:leaf,name:"Notes.txt".into(),position:file},
    ]);
    assert!(next_page.is_none());

    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    for n in 2..=36{make_file(n,&format!("Before-{n:02}.txt"));}
    db.fixed(c"COMMIT").unwrap();
    assert!(read(Some(parent),None).unwrap().1.iter().all(|item| matches!(item,Entry::File{..})));
    let exact=Label::parse("CAD").unwrap();
    let other_case=Label::parse("cad").unwrap();
    write(format!("INSERT INTO file_label VALUES({},{})",literal(&leaf.0),literal(exact.bytes())));
    let filtered=|name,after|memory_label_navigation(&db,&c,share,Some(parent),after,Some(name));
    let (_,first_filtered,next_filtered,route_scope,writable)=filtered(&exact,None).unwrap();
    assert_eq!(first_filtered,first); // Navigation remains available in a filtered view.
    assert!(writable);
    let second=filtered(&exact,next_filtered.as_ref()).unwrap().1;
    assert_eq!(second,last); // A label beyond the first page still matches.
    assert_eq!(filtered(&other_case,next_filtered.as_ref()).err().unwrap().kind(),io::ErrorKind::WouldBlock);
    let (_,_,case_next,_,_)=filtered(&other_case,None).unwrap();
    assert_eq!(filtered(&other_case,case_next.as_ref()).unwrap().1,last[..2]);
    let selected=Selected{share,parent:Some(parent),file:leaf,position:file,scope:route_scope};
    assert!(memory_label_check(&db,&c,selected,true).is_ok());
    assert_eq!(memory_label_check(&db,&c,Selected{position:OperationId([0x19;16]),..selected},true)
        .unwrap_err().kind(),io::ErrorKind::WouldBlock);
    assert_eq!(memory_label_check(&db,&c,Selected{scope:[0;32],..selected},false)
        .unwrap_err().kind(),io::ErrorKind::WouldBlock);

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
    assert_eq!(memory_label_check(&db,&c,selected,false).unwrap_err().kind(),io::ErrorKind::WouldBlock);
    let (_,_,still_next,renamed_scope,_)=filtered(&exact,None).unwrap();
    assert_eq!(filtered(&exact,still_next.as_ref()).unwrap().1,last);
    let after_rename=Selected{scope:renamed_scope,..selected};
    assert!(memory_label_check(&db,&c,after_rename,true).is_ok());

    // Move-shaped logical cuts exercise the real placement triggers only.
    // This fixture has no mounted roots, bytes, SMB sessions or cleanup.
    let transfer=|op:OperationId,direction:u8,target:VolumeId,name:&str,target_parent:[u8;16]|{
        db.fixed(c"BEGIN IMMEDIATE").unwrap();
        write(format!("INSERT INTO file_transfer(transfer_op,file_id,direction,source_op,source_gen,source_id,source_name,
                target_id,target_name,share_id,setting_op,auth_op,save_op,manual_op,
                source_parent,target_parent,source_backing,source_terminal,target_setting_op)
            SELECT {},l.file_id,{direction},l.position_op,l.generation,l.volume_id,l.display,
                {},{},a.share_id,a.setting_op,a.auth_op,l.save_op,l.manual_op,l.parent_id,{},l.backing_op,l.flat_terminal,NULL
            FROM transferable_location l JOIN live_ordinary_writer a ON a.share_id={}
            WHERE l.file_id={}",
            literal(&op.0),literal(&target.0),literal(name.as_bytes()),literal(&target_parent),
            literal(&share.0),literal(&leaf.0)));
        operation(op.0,12,keys.smb,[0x66;32],0);
        activate(&db,op).unwrap();
        db.fixed(c"COMMIT").unwrap();
    };
    transfer(OperationId(id(15,1)),1,VolumeId([0x32;16]),"Notes.txt",root);
    assert_eq!(memory_label_check(&db,&c,after_rename,false).unwrap_err().kind(),io::ErrorKind::WouldBlock);
    db.fixed(c"BEGIN IMMEDIATE").unwrap();
    let (unlabelled_namesake,_)=make_file(37,"Notes.txt");
    db.fixed(c"COMMIT").unwrap();
    assert_ne!(unlabelled_namesake,leaf);
    let restored=OperationId(id(15,2));
    transfer(restored,2,volume,"Restored.txt",parent.0);
    assert_eq!(memory_label_check(&db,&c,after_rename,true).unwrap_err().kind(),io::ErrorKind::WouldBlock);
    let after_restore=Selected{position:restored,..after_rename};
    assert!(memory_label_check(&db,&c,after_restore,true).is_ok());
    let (_,_,restored_next,_,_)=filtered(&exact,None).unwrap();
    let restored_tail=filtered(&exact,restored_next.as_ref()).unwrap().1;
    assert!(restored_tail.contains(&Entry::File{id:leaf,name:"Restored.txt".into(),position:restored}));
    assert!(!restored_tail.iter().any(|item|matches!(item,Entry::File{id,..} if *id==unlabelled_namesake)));

    activate(&db,OperationId(id(0,254))).unwrap();
    let readonly=id(14,0);db.fixed(c"BEGIN IMMEDIATE").unwrap();
    write(format!("INSERT INTO share_mode VALUES({},{},{},{},2,1)",
        literal(&readonly),literal(&share.0),literal(&mode),literal(&SETUP.0)));
    operation(readonly,4,keys.web,[0x63;32],1);
    db.fixed(c"COMMIT").unwrap();
    let (_,_,_,ro_scope,writable)=filtered(&exact,None).unwrap();
    assert!(!writable);
    let ro=Selected{scope:ro_scope,..after_restore};
    assert!(memory_label_check(&db,&c,ro,false).is_ok());
    assert_eq!(memory_label_check(&db,&c,ro,true).unwrap_err().kind(),io::ErrorKind::PermissionDenied);

    let disabled=id(10,0);db.fixed(c"BEGIN IMMEDIATE").unwrap();
    write(format!("INSERT INTO share_mode VALUES({},{},{},{},3,0)",
        literal(&disabled),literal(&share.0),literal(&readonly),literal(&SETUP.0)));
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

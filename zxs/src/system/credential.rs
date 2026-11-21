//! Qualified reads for one account. Must be used only inside the same original
//! S transaction as its validated catalog. Nothing here issues a credential,
//! opens the vault or a writer, or makes an authenticated role.
use super::{Catalog,Connection,check,sqlite3_step,sqlite3_column_type,super::shared_account::{PublicEpoch,SharedScope}};
use crate::{OperationId,PoolId,fs::invalid,identity::AccountId,password::{self,Verifier}};
use std::io;

#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) struct Epoch{pub(super) op:OperationId,pub(super) generation:i64}

/// Shared public coordinates plus the SMB actor domain, not a secret or grant.
#[derive(Clone,Eq,PartialEq)]
pub(super) struct ActiveRustMeta{
    pub(super) pool:PoolId,pub(super) init:OperationId,pub(super) setup:OperationId,pub(super) install_sha:[u8;32],
    pub(super) instance:[u8;16],pub(super) bind:[u8;16],pub(super) canonical:AccountId,pub(super) smb:AccountId,
    pub(super) login:String,pub(super) sid:[u8;28],pub(super) epoch:Epoch,pub(super) record:[u8;16],
}
pub(super) type Pair=OneAccount;

pub(super) struct OneAccount{
    pub(super) setup:OperationId,
    pub(super) setup_bound:[u8;32],
    pub(super) instance:[u8;16],
    pub(super) bind:[u8;16],
    pub(super) canonical:AccountId,
    // These are internal role actor keys, not distinct configured accounts.
    pub(super) web:AccountId,
    pub(super) smb:AccountId,
    pub(super) login:String,
    pub(super) sid:[u8;28],
    first:[u8;16],
}
fn bad()->io::Error{invalid("cannot qualify the exact shared account and epoch")}
impl OneAccount{
    fn scope<'a>(&'a self,c:&Catalog)->SharedScope<'a>{
        SharedScope{pool:c.birth.pool.0,init:c.birth.op.0,setup:self.setup.0,instance:self.instance,
            bind:self.bind,account:self.canonical.0,init_sha:c.birth.request,plan_sha:self.setup_bound,
            original_actor:c.birth.actor,login:self.login.as_bytes(),sid:self.sid}
    }
    pub(super) fn confirmed(db:&Connection,c:&Catalog)->io::Result<Option<Self>>{
        let s=db.prepare(c"SELECT a.birth_op,a.setup_bound_sha,a.instance_id,a.smb_rust_bind_id,
                 a.account_id,a.web_id,a.smb_id,a.login,a.smb_sid,v.record_id,
                 o.actor_id,a.approval_ref,o.request_sha
            FROM main.account_pair a
            CROSS JOIN main.operation o ON o.op_id=a.birth_op AND o.kind=2 AND o.state=1
            LEFT JOIN main.smb_rust_revision v ON v.credential_op=a.birth_op
            WHERE a.singleton=1")?;
        match unsafe{sqlite3_step(s.0)}{101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(bad());}}
        let identity=Self{setup:OperationId(s.array(0)?),setup_bound:s.array(1)?,instance:s.array(2)?,
            bind:s.array(3)?,canonical:AccountId(s.array(4)?),web:AccountId(s.array(5)?),smb:AccountId(s.array(6)?),
            login:password::login_name(s.blob(7,64)?)?.to_owned(),sid:s.array(8)?,first:s.array(9)?};
        let scope=identity.scope(c);let keys=scope.role_keys()?;
        if s.array::<16>(10)?!=c.birth.actor||keys.web!=identity.web.0||keys.smb!=identity.smb.0||
            scope.setup_request(s.array(11)?,identity.first)?!=s.array::<32>(12)?{return Err(bad());}
        s.done()?;
        Ok(Some(identity))
    }

    /// The only lookup exposing a Web verifier. Exact setup, current reset,
    /// both role actor keys and the same already active row are qualified in
    /// the single calling S cut; raw public coordinates cannot load the tag.
    pub(super) fn qualified_web(db:&Connection,c:&Catalog)->io::Result<Option<(Self,PublicEpoch,Verifier)>>{
        let Some(pair)=Self::confirmed(db,c)? else{return Ok(None);};
        let Some(epoch)=pair.current(db,c)? else{return Ok(None);};
        Ok(Some((pair,epoch,web_material(db,epoch)?)))
    }

    pub(super) fn active_rust_at(&self,db:&Connection,c:&Catalog)->io::Result<Option<ActiveRustMeta>>{
        let Some(e)=self.current(db,c)? else{return Ok(None);};
        Ok(Some(ActiveRustMeta{pool:c.birth.pool,init:c.birth.op,setup:self.setup,install_sha:self.setup_bound,
            instance:self.instance,bind:self.bind,canonical:self.canonical,smb:self.smb,login:self.login.clone(),
            sid:self.sid,epoch:Epoch{op:OperationId(e.op),generation:e.generation},record:e.record}))
    }

    /// One original head, or neither role is usable. No password material
    /// is loaded or returned. The DDL's highest attempted generation is
    /// bounded by index; select the exact predecessor by a primary-key join
    /// rather than materializing the historical epoch/role view.
    pub(super) fn current(&self,db:&Connection,c:&Catalog)->io::Result<Option<PublicEpoch>>{
        let s=db.prepare(c"SELECT e.credential_op,e.generation,e.record_id,o.actor_id,o.request_sha,
                r.approval_ref,r.previous_op,r.bound_sha,p.generation,po.state,po.kind,po.actor_id,vp.record_id
            FROM main.active_shared_credential e
            CROSS JOIN main.operation o ON o.op_id=e.credential_op
            LEFT JOIN main.credential_reset r ON r.op_id=e.credential_op
            LEFT JOIN main.credential_reset p ON p.op_id=r.previous_op
            LEFT JOIN main.operation po ON po.op_id=r.previous_op
            LEFT JOIN main.smb_rust_revision vp ON vp.credential_op=r.previous_op
            WHERE e.slot=1")?;
        match unsafe{sqlite3_step(s.0)}{101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(bad());}}
        let result=PublicEpoch{op:s.array(0)?,generation:s.integer(1)?,record:s.array(2)?};
        let is_null=|i|unsafe{sqlite3_column_type(s.0,i)}==5;
        if s.array::<16>(3)?!=c.birth.actor{return Err(bad());}
        if result.generation==1{
            if result.op!=self.setup.0||result.record!=self.first||(5..13).any(|i|!is_null(i)){return Err(bad());}
        }else{
            let old=PublicEpoch{op:s.array(6)?,generation:result.generation.checked_sub(1).ok_or_else(bad)?,record:s.array(12)?};
            let first=old.generation==1;
            let previous_matches=if first{is_null(8)&&old.record==self.first}else{s.integer(8)?==old.generation};
            if result.generation<=1||s.array::<32>(7)?!=self.setup_bound||s.integer(9)?!=1||
                s.integer(10)?!=(if first{2}else{9})||s.array::<16>(11)?!=c.birth.actor||
                !previous_matches
            {return Err(bad());}
            let expected=self.scope(c).reset_request(s.array(5)?,old,result)?;
            if expected!=s.array::<32>(4)?{return Err(bad());}
        }
        s.done()?;
        Ok(Some(result))
    }
}

/// The Web-only caller uses its just-selected exact head in the same cut.
/// Neither SMB nor launch/custody receives this verifier.
fn web_material(db:&Connection,e:PublicEpoch)->io::Result<Verifier>{
    let s=db.prepare(c"SELECT w.salt,w.verifier FROM main.active_shared_credential a
        CROSS JOIN main.web_password w ON w.slot=a.slot AND w.credential_op=a.credential_op AND w.generation=a.generation
        WHERE a.credential_op=?1 AND a.generation=?2 AND a.record_id=?3")?;
    s.bind_bytes(1,&e.op)?;s.bind_integer(2,e.generation)?;s.bind_bytes(3,&e.record)?;
    match unsafe{sqlite3_step(s.0)}{100=>{},rc=>{check(rc)?;return Err(bad());}}
    let result=Verifier::new(s.array(0)?,s.array(1)?);
    s.done()?;
    Ok(result)
}

#[cfg(test)]
mod tests{
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
        let keys=scope.role_keys().unwrap();
        db.fixed(c"BEGIN IMMEDIATE").unwrap();
        run(db,format!("INSERT INTO account_pair VALUES(1,{},{},{},{},{},{},{},{},{},{})",
            blob(&SETUP),blob(&APPROVAL),blob(&scope.instance),blob(&scope.plan_sha),blob(&scope.account),
            blob(scope.login),blob(&web),blob(&keys.smb),blob(&scope.sid),blob(&scope.bind)));
        run(db,format!("INSERT INTO web_password VALUES(1,{},1,zeroblob(16),zeroblob(32))",blob(&SETUP)));
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
        setup(&db,scope,keys.web);
        assert!(OneAccount::confirmed(&db,&c).unwrap().is_none());
        status(&db,SETUP);
        let shared=OneAccount::confirmed(&db,&c).unwrap().unwrap();
        assert_eq!(shared.canonical.0,scope.account);assert_eq!(shared.web.0,keys.web);
        assert_eq!(shared.smb.0,keys.smb);assert_eq!(shared.login,"shared.operator");
        assert!(shared.current(&db,&c).unwrap()==Some(first));
        assert!(OneAccount::qualified_web(&db,&c).unwrap().is_some_and(|(_,e,_)|e==first));
        reset(&db,scope,scope.reset_request(APPROVAL,first,next).unwrap());
        assert!(shared.current(&db,&c).unwrap().is_none());
        assert!(OneAccount::qualified_web(&db,&c).unwrap().is_none());
        status(&db,RESET);
        assert!(shared.current(&db,&c).unwrap()==Some(next));
        assert!(OneAccount::qualified_web(&db,&c).unwrap().is_some_and(|(_,e,_)|e==next));
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
    }
}

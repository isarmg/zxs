//! The two fixed principals and their public revisions. There is no SMB
//! verifier in this database. None of the values here authenticate a client.
use super::{Catalog,Connection,check,sqlite3_step,sqlite3_column_type};
use crate::{OperationId,PoolId,fs::invalid,hash::Sha256,identity::AccountId,password::login_name};
use std::io;

const SID_PREFIX:[u8;12]=[1,5,0,0,0,0,0,5,21,0,0,0];
#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) enum Role{Web=0,Smb=1}
pub(super) fn setup_request(pool:PoolId,init:OperationId,op:OperationId,actor:[u8;16],
    approval:[u8;16],instance:[u8;16],bound:[u8;32],web:&[u8],smb:&[u8],sid:&[u8;28])->io::Result<[u8;32]>{
    login_name(web)?;let s=login_name(smb)?;
    if web==smb||matches!(s,"guest"|"anonymous"|"root"|"nobody")||
        sid[..12]!=SID_PREFIX||sid[12..24]==[0;12]||sid[24..]!=1000u32.to_le_bytes()||
        [pool.0,init.0,op.0,actor,approval,instance].contains(&[0;16])||op==init{
        return Err(invalid("invalid native pair origin"));
    }
    let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/NATIVE-PAIR\0")?;
    for id in [pool.0,init.0,op.0,actor,approval,instance]{h.update(&id)?;}h.update(&bound)?;
    for name in [web,smb]{h.update(&[name.len() as u8])?;h.update(name)?;}
    h.update(sid)?;Ok(h.finish())
}
pub(super) fn reset_request(pool:PoolId,init:OperationId,setup:OperationId,op:OperationId,actor:[u8;16],
    approval:[u8;16],account:AccountId,previous:OperationId,previous_gen:i64,native:Option<([u8;16],[u8;32])>)->io::Result<[u8;32]>{
    if [pool.0,init.0,setup.0,op.0,actor,approval,account.0,previous.0].contains(&[0;16])||
        [init,setup,previous].contains(&op)||previous_gen<1||previous_gen==i64::MAX||native.is_some_and(|(i,_)|i==[0;16]){
        return Err(invalid("invalid reset origin"));
    }
    let mut h=Sha256::new();
    h.update(if native.is_some(){b"ZXS-SQLITE-1/RESET-SMB\0"}else{b"ZXS-SQLITE-1/RESET-WEB\0"})?;
    for id in [pool.0,init.0,setup.0,op.0,actor,approval,account.0,previous.0]{h.update(&id)?;}
    h.update(&previous_gen.to_be_bytes())?;
    if let Some((inst,bound))=native{h.update(&inst)?;h.update(&bound)?;}Ok(h.finish())
}

pub(super) struct Pair{pub(super) setup:OperationId,pub(super) setup_bound:[u8;32],pub(super) instance:[u8;16],pub(super) web:AccountId,
    pub(super) web_login:String,pub(super) smb:AccountId,pub(super) smb_login:String,pub(super) sid:[u8;28]}
#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) struct Epoch{pub(super) op:OperationId,pub(super) generation:i64}
impl Pair{
    // Call only in the same original-S read transaction as epoch()/payload.
    pub(super) fn confirmed(db:&Connection,c:&Catalog)->io::Result<Option<Self>>{
        let s=db.prepare(c"SELECT a.birth_op,o.actor_id,o.request_sha,a.approval_ref,a.instance_id,a.setup_bound_sha,a.web_id,a.web_login,a.smb_id,a.smb_login,a.smb_sid FROM main.account_pair a JOIN main.operation o ON o.op_id=a.birth_op WHERE a.singleton=1 AND o.kind=2 AND o.state=1")?;
        match unsafe{sqlite3_step(s.0)}{101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(invalid("cannot select the original pair"));}}
        let setup=OperationId(s.array(0)?);let instance=s.array(4)?;let setup_bound=s.array(5)?;let web=AccountId(s.array(6)?);let smb=AccountId(s.array(8)?);
        let (wn,sn,sid)=(s.blob(7,64)?,s.blob(9,64)?,s.array(10)?);
        if web==smb||[web.0,smb.0].contains(&[0;16])||
            setup_request(c.birth.pool,c.birth.op,setup,s.array(1)?,s.array(3)?,instance,setup_bound,wn,sn,&sid)?!=s.array::<32>(2)?{
            return Err(invalid("wrong original fixed-pair request"));}
        let row=Self{setup,setup_bound,instance,web,smb,web_login:login_name(wn)?.to_owned(),smb_login:login_name(sn)?.to_owned(),sid};s.done()?;Ok(Some(row))
    }
    pub(super) fn epoch(&self,db:&Connection,c:&Catalog,role:Role)->io::Result<Option<Epoch>>{
        // One indexed max(role,generation), not a scan of all older resets.
        let s=db.prepare(c"SELECT e.credential_op,e.generation,o.actor_id,o.request_sha,r.approval_ref,r.previous_op,r.bound_sha,p.generation,po.state,po.kind FROM main.active_credential e JOIN main.operation o ON o.op_id=e.credential_op LEFT JOIN main.credential_reset r ON r.role=e.role AND r.op_id=e.credential_op LEFT JOIN main.credential_epoch p ON p.role=e.role AND p.credential_op=r.previous_op LEFT JOIN main.operation po ON po.op_id=p.credential_op WHERE e.role=?1 AND e.generation=COALESCE((SELECT max(generation) FROM main.credential_reset WHERE role=?1),1)")?;
        s.bind_integer(1,role as i64)?;
        match unsafe{sqlite3_step(s.0)}{101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(invalid("cannot select the current credential revision"));}}
        let result=Epoch{op:OperationId(s.array(0)?),generation:s.integer(1)?};
        if result.generation==1{
            if result.op!=self.setup{return Err(invalid("wrong original credential"));}
        }else{
            let previous=s.integer(7)?;
            let prev_kind=if previous==1{2}else if role==Role::Smb{7}else{8};
            if result.generation<=1||previous!=result.generation-1||s.integer(8)?!=1||s.integer(9)?!=prev_kind{
                return Err(invalid("credential has no committed original predecessor"));}
            let (account,native)=match role{
                Role::Smb=>(self.smb,Some((self.instance,s.array(6)?))),
                Role::Web=>{if unsafe{sqlite3_column_type(s.0,6)}!=5{return Err(invalid("Web reset may not select a native credential"));}(self.web,None)}
            };
            if reset_request(c.birth.pool,c.birth.op,self.setup,result.op,s.array(2)?,s.array(4)?,account,
                OperationId(s.array(5)?),previous,native)?!=s.array::<32>(3)?{
                return Err(invalid("wrong original credential reset request"));}
        }
        s.done()?;Ok(Some(result))
    }
}

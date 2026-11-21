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

// Rust scope: no secrets, login proof, endpoint approval or ordinary access.
// The same fields have an independently implemented writer in Helper.
#[derive(Clone,Copy,Eq,PartialEq)]
enum SmbFamily { Native, RustV1 { bind:[u8;16], first:[u8;16] } }

fn put_names(h:&mut Sha256,web:Option<&[u8]>,smb:&[u8],sid:&[u8;28])->io::Result<()>{
    let user=login_name(smb)?;
    if matches!(user,"guest"|"anonymous"|"root"|"nobody") ||
        sid[..12]!=SID_PREFIX || sid[12..24]==[0;12] || sid[24..]!=1000u32.to_le_bytes(){
        return Err(invalid("invalid Rust SMB identity"));
    }
    if let Some(w)=web{
        login_name(w)?;
        if w==smb{return Err(invalid("fixed protocol accounts must differ"));}
        h.update(&[w.len() as u8])?;h.update(w)?;
    }
    h.update(&[smb.len() as u8])?;h.update(smb)?;
    h.update(sid)
}
fn rust_setup_request(pool:PoolId,init:OperationId,p:&Pair,actor:[u8;16],approval:[u8;16],
    bind:[u8;16],first:[u8;16])->io::Result<[u8;32]>{
    let ids=[pool.0,init.0,p.setup.0,actor,approval,p.instance,bind,p.web.0,p.smb.0,first];
    if ids.contains(&[0;16])||init==p.setup||p.web==p.smb||p.setup_bound==[0;32]{
        return Err(invalid("invalid Rust SMB setup origin"));
    }
    let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/RUST-SMB-PAIR-1\0")?;
    for id in ids{h.update(&id)?;}
    h.update(&p.setup_bound)?;
    put_names(&mut h,Some(p.web_login.as_bytes()),p.smb_login.as_bytes(),&p.sid)?;
    Ok(h.finish())
}
fn rust_reset_request(pool:PoolId,init:OperationId,p:&Pair,op:OperationId,actor:[u8;16],approval:[u8;16],
    bind:[u8;16],previous:OperationId,prior:i64,previous_record:[u8;16],record:[u8;16])->io::Result<[u8;32]>{
    let ids=[pool.0,init.0,p.setup.0,op.0,actor,approval,p.instance,bind,p.smb.0,previous.0,previous_record,record];
    if ids.contains(&[0;16]) || [init,p.setup,previous].contains(&op) || previous==init ||
        !(1..i64::MAX).contains(&prior) || (prior==1)!=(previous==p.setup) ||
        previous_record==record || p.setup_bound==[0;32]{
        return Err(invalid("invalid Rust SMB reset origin"));
    }
    let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/RUST-SMB-RESET-1\0")?;
    for id in ids{h.update(&id)?;}
    h.update(&prior.to_be_bytes())?;h.update(&p.setup_bound)?;
    put_names(&mut h,None,p.smb_login.as_bytes(),&p.sid)?;
    Ok(h.finish())
}

pub(super) struct Pair{pub(super) setup:OperationId,pub(super) setup_bound:[u8;32],pub(super) instance:[u8;16],pub(super) web:AccountId,
    pub(super) web_login:String,pub(super) smb:AccountId,pub(super) smb_login:String,pub(super) sid:[u8;28],smb_family:SmbFamily}
#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) struct Epoch{pub(super) op:OperationId,pub(super) generation:i64}

/// One current, checked SQL selection, not a runtime grant. It contains no
/// key or secret digest and is not constructible from a credential file.
#[derive(Clone,Eq,PartialEq)]
pub(super) struct ActiveRustMeta{
    pub(super) pool:PoolId,pub(super) init:OperationId,pub(super) setup:OperationId,pub(super) install_sha:[u8;32],
    pub(super) instance:[u8;16],pub(super) bind:[u8;16],pub(super) smb:AccountId,pub(super) login:String,
    pub(super) sid:[u8;28],pub(super) epoch:Epoch,pub(super) record:[u8;16],
}
impl Pair{
    pub(super) fn is_native(&self)->bool{matches!(self.smb_family,SmbFamily::Native)}

    // Call only in the same original-S read transaction as epoch()/payload.
    pub(super) fn confirmed(db:&Connection,c:&Catalog)->io::Result<Option<Self>>{
        let s=db.prepare(c"SELECT a.birth_op,o.actor_id,o.request_sha,a.approval_ref,a.instance_id,a.setup_bound_sha,a.web_id,a.web_login,a.smb_id,a.smb_login,a.smb_sid,a.smb_family,a.smb_rust_bind_id,v.record_id FROM main.account_pair a JOIN main.operation o ON o.op_id=a.birth_op LEFT JOIN main.smb_rust_revision v ON v.credential_op=a.birth_op WHERE a.singleton=1 AND o.kind=2 AND o.state=1")?;
        match unsafe{sqlite3_step(s.0)}{101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(invalid("cannot select the original pair"));}}
        let setup=OperationId(s.array(0)?);let instance=s.array(4)?;let setup_bound=s.array(5)?;let web=AccountId(s.array(6)?);let smb=AccountId(s.array(8)?);
        let (wn,sn,sid)=(s.blob(7,64)?,s.blob(9,64)?,s.array(10)?);
        if web==smb||[web.0,smb.0].contains(&[0;16]){return Err(invalid("wrong original fixed-pair identity"));}
        let null=|i|unsafe{sqlite3_column_type(s.0,i)}==5;
        let smb_family=match s.integer(11)?{
            0 if null(12)&&null(13)=>SmbFamily::Native,
            1=>SmbFamily::RustV1{bind:s.array(12)?,first:s.array(13)?},
            _=>return Err(invalid("wrong original SMB family")),
        };
        let row=Self{setup,setup_bound,instance,web,smb,web_login:login_name(wn)?.to_owned(),smb_login:login_name(sn)?.to_owned(),sid,smb_family};
        let (actor,approval)=(s.array(1)?,s.array(3)?);
        let request=match smb_family{
            SmbFamily::Native=>setup_request(c.birth.pool,c.birth.op,setup,actor,approval,instance,setup_bound,wn,sn,&sid)?,
            SmbFamily::RustV1{bind,first}=>rust_setup_request(c.birth.pool,c.birth.op,&row,actor,approval,bind,first)?,
        };
        if request!=s.array::<32>(2)?{return Err(invalid("wrong original fixed-pair request"));}
        s.done()?;Ok(Some(row))
    }

    pub(super) fn epoch(&self,db:&Connection,c:&Catalog,role:Role)->io::Result<Option<Epoch>>{
        self.epoch_with_record(db,c,role).map(|row|row.map(|(epoch,_)|epoch))
    }

    /// Exact CURRENT Rust public epoch. A pending newer reset is never a
    /// reason to pick an older committed record or a different family.
    pub(super) fn active_rust_at(&self,db:&Connection,c:&Catalog)->io::Result<Option<ActiveRustMeta>>{
        let SmbFamily::RustV1{bind,..}=self.smb_family else{return Ok(None);};
        let Some((epoch,record))=self.epoch_with_record(db,c,Role::Smb)? else{return Ok(None);};
        let record=record.ok_or_else(||invalid("missing current Rust vault record identity"))?;
        Ok(Some(ActiveRustMeta{pool:c.birth.pool,init:c.birth.op,setup:self.setup,install_sha:self.setup_bound,
            instance:self.instance,bind,smb:self.smb,login:self.smb_login.clone(),sid:self.sid,epoch,record}))
    }

    fn epoch_with_record(&self,db:&Connection,c:&Catalog,role:Role)->io::Result<Option<(Epoch,Option<[u8;16]>)>>{
        // One indexed max(role,generation), not a scan of all older resets.
        let s=db.prepare(c"SELECT e.credential_op,e.generation,o.actor_id,o.request_sha,r.approval_ref,r.previous_op,r.bound_sha,p.generation,po.state,po.kind,v.record_id,vp.record_id
            FROM main.active_credential e JOIN main.operation o ON o.op_id=e.credential_op
            LEFT JOIN main.credential_reset r ON r.role=e.role AND r.op_id=e.credential_op
            LEFT JOIN main.credential_epoch p ON p.role=e.role AND p.credential_op=r.previous_op
            LEFT JOIN main.operation po ON po.op_id=p.credential_op
            LEFT JOIN main.smb_rust_revision v ON e.role=1 AND v.credential_op=e.credential_op
            LEFT JOIN main.smb_rust_revision vp ON e.role=1 AND vp.credential_op=r.previous_op
            WHERE e.role=?1 AND e.generation=COALESCE((SELECT max(generation) FROM main.credential_reset WHERE role=?1),1)")?;
        s.bind_integer(1,role as i64)?;
        match unsafe{sqlite3_step(s.0)}{101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(invalid("cannot select the current credential revision"));}}
        let result=Epoch{op:OperationId(s.array(0)?),generation:s.integer(1)?};
        let rust=match (role,self.smb_family){
            (Role::Smb,SmbFamily::RustV1{bind,first})=>Some((bind,first)),
            _=>None,
        };
        let null=|i|unsafe{sqlite3_column_type(s.0,i)}==5;
        let record=if rust.is_some(){
            let r=s.array::<16>(10)?;
            if r==[0;16]{return Err(invalid("invalid Rust vault record identity"));}
            Some(r)
        }else{
            if !null(10)||!null(11){return Err(invalid("credential crossed its SMB family"));}
            None
        };
        if result.generation==1{
            if result.op!=self.setup||rust.is_some_and(|(_,first)|Some(first)!=record){return Err(invalid("wrong original credential"));}
        }else{
            let previous=s.integer(7)?;
            let prev_kind=if previous==1{2}else if role==Role::Smb{7}else{8};
            if result.generation<=1||previous!=result.generation-1||s.integer(8)?!=1||s.integer(9)?!=prev_kind{
                return Err(invalid("credential has no committed original predecessor"));}
            let prior=OperationId(s.array(5)?);
            let (actor,approval)=(s.array(2)?,s.array(4)?);
            let request=if let Some((bind,first))=rust{
                let previous_record=s.array::<16>(11)?;
                if s.array::<32>(6)?!=self.setup_bound || (previous==1 && previous_record!=first){
                    return Err(invalid("Rust credential changed its original installation or first record"));
                }
                rust_reset_request(c.birth.pool,c.birth.op,self,result.op,actor,approval,bind,prior,previous,
                    previous_record,record.ok_or_else(||invalid("missing current Rust vault record identity"))?)?
            }else{
                let (account,native)=match role{
                    Role::Smb=>(self.smb,Some((self.instance,s.array(6)?))),
                    Role::Web=>{if !null(6){return Err(invalid("Web reset cannot select an SMB credential"));}(self.web,None)}
                };
                reset_request(c.birth.pool,c.birth.op,self.setup,result.op,actor,approval,account,prior,previous,native)?
            };
            if request!=s.array::<32>(3)?{return Err(invalid("wrong original credential reset request"));}
        }
        s.done()?;Ok(Some((result,record)))
    }
}

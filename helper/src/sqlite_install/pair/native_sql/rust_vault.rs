//! The shared public epoch and the two short original-S writes. No password,
//! SMB key, root vault namespace, or permission to apply is available here.
use super::{Core,Id,Ids,Original,bad,blob,collision,first_ordinary,history,one,sql,stage};
use super::super::{login,native_run::joint_material::{Binding,Epoch,Received,StagedWeb}};
use super::transaction::{AccountCut,expired};
use crate::{smb_profile::rust_first::RustPlan,sqlite_install::{originals::LockedSystem,stage::random}};
use rusqlite::{params,Transaction};
use sha2::{Digest,Sha256};
use std::{io,time::Instant};

const SID_PREFIX:[u8;12]=[1,5,0,0,0,0,0,5,21,0,0,0];
const ACTOR:&[u8]=b"ZXS-SQLITE-1/ACCOUNT-ACTOR-1\0";
const SETUP:&[u8]=b"ZXS-SQLITE-1/SHARED-SETUP-1\0";
const RESET:&[u8]=b"ZXS-SQLITE-1/SHARED-RESET-1\0";
const ACTIVATE:&str=include_str!("activate-rust.sql");
pub(in crate::sqlite_install::pair) const VAULT_MAGIC:&[u8;16]=b"ZXS-RUST-SMB-V2\0";
pub(in crate::sqlite_install::pair) const VAULT_NAME:&std::ffi::CStr=c"credential.v2";
fn wrong()->io::Error{bad("not the original shared account and its exact credential epoch")}
pub(in crate::sqlite_install::pair) fn shared_login(name:&[u8])->bool{
    login(name)&&!matches!(name,b"guest"|b"anonymous"|b"nobody"|b"root")
}
fn valid_sid(sid:&[u8;28])->bool{
    sid[..12]==SID_PREFIX&&sid[12..24]!=[0;12]&&sid[24..]==1000u32.to_le_bytes()
}
fn roles(account:Id,actor:Id)->io::Result<Ids>{
    if [account,actor].contains(&[0;16])||account==actor{return Err(wrong());}
    let make=|member:u8|->Id{
        let mut h=Sha256::new();h.update(ACTOR);h.update(account);h.update([member]);
        h.finalize()[..16].try_into().expect("fixed role key")
    };
    let ids=Ids::checked(make(0),make(1))?;
    if [ids.web,ids.smb].iter().any(|a|*a==actor||*a==account){return Err(wrong());}
    Ok(ids)
}

#[derive(Clone,Eq,PartialEq)]
pub(in crate::sqlite_install::pair) struct ActiveRustMeta{
    pub pool:Id,pub init:Id,pub setup:Id,pub instance:Id,pub bind:Id,
    pub init_sha:[u8;32],pub descriptor_sha:[u8;32],pub account:Id,pub login:Vec<u8>,pub sid:[u8;28],
    pub op:Id,pub generation:i64,pub record:Id,
}
impl ActiveRustMeta{
    fn installation(&self)->[Id;5]{[self.pool,self.init,self.setup,self.instance,self.bind]}
    fn matches(&self,p:&RustPlan)->bool{
        let o=p.origin;self.installation()==[o.pool,o.init,o.setup,o.instance,o.bind]&&self.descriptor_sha==p.sha()
    }
    fn epoch(&self)->Epoch{Epoch{op:self.op,generation:self.generation,record:self.record}}
    pub(in crate::sqlite_install::pair) fn record_prefix(&self)->io::Result<[u8;224]>{
        let fixed=self.installation();
        if [self.account,self.op,self.record].contains(&[0;16])||fixed.contains(&[0;16])||
            fixed.iter().enumerate().any(|(i,v)|fixed[..i].contains(v))||
            [self.init_sha,self.descriptor_sha].contains(&[0;32])||!shared_login(&self.login)||!valid_sid(&self.sid)||
            !match self.generation{1=>self.op==self.setup,2.. =>!fixed.contains(&self.op),_=>false}{return Err(wrong());}
        let mut out=[0u8;224];out[..16].copy_from_slice(VAULT_MAGIC);out[16]=1;out[17]=self.login.len() as u8;
        for (slot,id) in out[24..152].chunks_exact_mut(16).zip(fixed.into_iter().chain([self.account,self.op,self.record])){
            slot.copy_from_slice(&id);
        }
        out[152..160].copy_from_slice(&self.generation.to_be_bytes());
        out[160..160+self.login.len()].copy_from_slice(&self.login);
        helper_linux_evidence::first_smb::require_rust_v2_public_prefix(&out)?;
        Ok(out)
    }
    fn request(&self,domain:&[u8],actor:Id,approval:Id)->io::Result<Sha256>{
        self.record_prefix()?;roles(self.account,actor)?;
        if approval==[0;16]||approval==self.op{return Err(wrong());}
        let mut h=Sha256::new();h.update(domain);
        for id in self.installation().into_iter().chain([self.account]){h.update(id);}
        h.update(self.init_sha);h.update(self.descriptor_sha);
        h.update([self.login.len() as u8]);h.update(&self.login);h.update(self.sid);
        h.update(actor);h.update(approval);Ok(h)
    }
}
fn committed_init_digest(tx:&Transaction<'_>,plan:&RustPlan,actor:Id)->io::Result<[u8;32]>{
    let o=plan.origin;
    let mut st=tx.prepare("SELECT i.request_sha FROM main.pool_birth b JOIN main.operation i ON i.op_id=b.init_op
       WHERE b.singleton=1 AND b.pool_id=?1 AND b.init_op=?2 AND i.kind=1 AND i.state=1 AND i.actor_id=?3").map_err(sql)?;
    let mut q=st.query(params![&o.pool[..],&o.init[..],&actor[..]]).map_err(sql)?;
    let digest=blob(q.next().map_err(sql)?.ok_or_else(wrong)?,0)?;
    if digest==[0;32]||q.next().map_err(sql)?.is_some(){return Err(wrong());}
    Ok(digest)
}
fn forbid_unsettled_births(tx:&Transaction<'_>,old:Id)->io::Result<()>{
    // The reviewed view has a fixed partial index and at most 257 rows.
    let block:bool=tx.query_row("SELECT 256 < (SELECT count(*) FROM main.credential_reset_births)
           OR EXISTS(SELECT 1 FROM main.credential_reset_births WHERE auth_op IS NULL OR (aborted=0 AND auth_op=?1))",
           [&old[..]],|r|r.get(0)).map_err(sql)?;
    if block{return Err(bad("resolve the protected original pending FILE or directory birth before changing the shared account"));}
    Ok(())
}
fn no_active(tx:&Transaction<'_>)->io::Result<()>{
    let exposed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.active_credential)",[],|r|r.get(0)).map_err(sql)?;
    if exposed{return Err(wrong());}Ok(())
}
fn write_record(tx:&Transaction<'_>,m:&ActiveRustMeta)->io::Result<()>{
    one(tx.execute("INSERT INTO main.smb_rust_revision(credential_op,record_id) VALUES(?1,?2)",params![&m.op[..],&m.record[..]]))
}

/// Fresh nonsecret IDs are frozen before the single public setup consent.
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::sqlite_install::pair) struct SetupSpec{pub account:Id,pub login:Vec<u8>,pub sid:[u8;28],pub record:Id}
impl SetupSpec{
    pub(in crate::sqlite_install::pair) fn new(login:Vec<u8>)->io::Result<Self>{
        if !shared_login(&login){return Err(wrong());}
        let mut sid=[0;28];sid[..12].copy_from_slice(&SID_PREFIX);
        sid[12..24].copy_from_slice(&random::<12>()?);sid[24..].copy_from_slice(&1000u32.to_le_bytes());
        Ok(Self{account:random()?,login,sid,record:random()?})
    }
}
#[derive(Clone,Debug)]
pub(in crate::sqlite_install::pair) enum Desired{Setup(SetupSpec),Reset{previous:Id,generation:i64}}
#[derive(Clone,Debug)]
pub(in crate::sqlite_install::pair) struct RustOrder{pub actor:Id,pub approval:Id,pub op:Id,pub role:Desired}

struct Birth{scope:Core,target:ActiveRustMeta,keys:Ids,digest:[u8;32]}
impl Birth{
    fn new(plan:&RustPlan,init_sha:[u8;32],actor:Id,approval:Id,spec:&SetupSpec)->io::Result<Self>{
        let o=plan.origin;let scope=Core::new(o.pool,o.init,o.setup,actor,approval)?;
        let target=ActiveRustMeta{pool:o.pool,init:o.init,setup:o.setup,instance:o.instance,bind:o.bind,
            init_sha,descriptor_sha:plan.sha(),account:spec.account,login:spec.login.clone(),sid:spec.sid,
            op:o.setup,generation:1,record:spec.record};
        let keys=roles(spec.account,actor)?;
        let mut h=target.request(SETUP,actor,approval)?;h.update(spec.record);
        Ok(Self{scope,target,keys,digest:h.finalize().into()})
    }
    fn original(tx:&Transaction<'_>,plan:&RustPlan)->io::Result<Original<Self>>{
        let p=plan.origin;
        let mut s=tx.prepare("SELECT o.state,o.actor_id,o.request_sha,a.approval_ref,a.account_id,a.login,a.web_id,a.smb_id,a.smb_sid,r.record_id
            FROM main.account_pair a JOIN main.operation o ON o.op_id=a.birth_op AND o.kind=2
            JOIN main.smb_rust_revision r ON r.credential_op=a.birth_op
            WHERE a.singleton=1 AND a.birth_op=?1 AND a.instance_id=?2 AND a.setup_bound_sha=?3 AND a.smb_rust_bind_id=?4").map_err(sql)?;
        let mut q=s.query(params![&p.setup[..],&p.instance[..],&plan.sha()[..],&p.bind[..]]).map_err(sql)?;
        let Some(row)=q.next().map_err(sql)? else{
            let seen:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.account_pair)
                OR EXISTS(SELECT 1 FROM main.operation WHERE kind=2) OR EXISTS(SELECT 1 FROM main.web_password)
                OR EXISTS(SELECT 1 FROM main.smb_rust_revision) OR EXISTS(SELECT 1 FROM main.credential_reset)",
                [],|r|r.get(0)).map_err(sql)?;
            if seen||collision(tx,&p.setup)?{return Err(wrong());}
            return Ok(Original::Absent);
        };
        let actor=blob(row,1)?;
        let spec=SetupSpec{account:blob(row,4)?,login:row.get(5).map_err(sql)?,sid:blob(row,8)?,record:blob(row,9)?};
        let birth=Self::new(plan,committed_init_digest(tx,plan,actor)?,actor,blob(row,3)?,&spec)?;
        let state=row.get(0).map_err(sql)?;
        if birth.digest!=blob::<32>(row,2)?||[birth.keys.web,birth.keys.smb]!=[blob(row,6)?,blob(row,7)?]
            ||q.next().map_err(sql)?.is_some(){return Err(wrong());}
        history(state,birth)
    }
    fn validate_plan(&self,tx:&Transaction<'_>,plan:&RustPlan)->io::Result<()>{
        if !self.target.matches(plan)||committed_init_digest(tx,plan,self.scope.actor)?!=self.target.init_sha{return Err(wrong());}
        let r:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.volume_claim WHERE claim_op=?1 AND ordinal=1 AND role=1)",
            [&self.scope.init[..]],|r|r.get(0)).map_err(sql)?;
        if !r{return Err(wrong());}
        first_ordinary::volumes(tx,&self.scope.init,plan.folders())
    }
    fn first(&self)->first_ordinary::Authority{
        first_ordinary::Authority{pool:self.scope.pool,setup:self.scope.op,web:self.keys.web}
    }
    fn stage_identity(&self,tx:&Transaction<'_>,plan:&RustPlan)->io::Result<()>{
        self.validate_plan(tx,plan)?;
        if !matches!(Self::original(tx,plan)?,Original::Absent){return Err(wrong());}
        first_ordinary::vacant(tx,plan.folders())?;
        let c=self.scope;let t=&self.target;
        one(tx.execute("INSERT INTO main.account_pair(singleton,birth_op,approval_ref,instance_id,setup_bound_sha,
                      account_id,login,web_id,smb_id,smb_sid,smb_rust_bind_id) VALUES(1,?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![&c.op[..],&c.approval[..],&t.instance[..],&t.descriptor_sha[..],&t.account[..],&t.login,
                &self.keys.web[..],&self.keys.smb[..],&t.sid[..],&t.bind[..]]))
    }
}

struct Reset{scope:Core,prior:ActiveRustMeta,target:ActiveRustMeta,digest:[u8;32]}
impl Reset{
    fn new(prior:ActiveRustMeta,op:Id,actor:Id,approval:Id,record:Id)->io::Result<Self>{
        prior.record_prefix()?;
        let scope=Core::new(prior.pool,prior.init,op,actor,approval)?;
        let generation=prior.generation.checked_add(1).ok_or_else(wrong)?;
        if prior.record==record||prior.op==op{return Err(wrong());}
        let target=ActiveRustMeta{op,generation,record,..prior.clone()};
        let mut h=target.request(RESET,actor,approval)?;
        for v in [op,prior.op,prior.record]{h.update(v);}
        h.update(prior.generation.to_be_bytes());h.update(record);
        Ok(Self{scope,prior,target,digest:h.finalize().into()})
    }
    fn stage_identity(&self,tx:&Transaction<'_>,plan:&RustPlan)->io::Result<()>{
        if collision(tx,&self.scope.op)?||active_at(tx,plan)?!=self.prior{return Err(wrong());}
        forbid_unsettled_births(tx,self.prior.op)?;
        let t=&self.target;let c=self.scope;
        one(tx.execute("INSERT INTO main.credential_reset(op_id,previous_op,generation,approval_ref,bound_sha) VALUES(?1,?2,?3,?4,?5)",
            params![&t.op[..],&self.prior.op[..],t.generation,&c.approval[..],&t.descriptor_sha[..]]))
    }
}
fn select_reset(tx:&Transaction<'_>,birth:&Birth,op:Id,generation:i64,state:i64)->io::Result<Reset>{
    let mut s=tx.prepare("SELECT o.actor_id,o.request_sha,r.approval_ref,r.previous_op,v.record_id,pv.record_id
      FROM main.credential_reset r JOIN main.operation o ON o.op_id=r.op_id AND o.kind=9
      JOIN main.operation p ON p.op_id=r.previous_op AND p.state=1 AND p.actor_id=o.actor_id
      JOIN main.smb_rust_revision v ON v.credential_op=r.op_id
      JOIN main.smb_rust_revision pv ON pv.credential_op=r.previous_op
      LEFT JOIN main.credential_reset pr ON pr.op_id=r.previous_op
      WHERE r.op_id=?1 AND r.generation=?2 AND o.state=?3 AND r.bound_sha=?4
      AND ((r.generation=2 AND p.kind=2 AND r.previous_op=?5 AND pr.op_id IS NULL)
        OR (r.generation>2 AND p.kind=9 AND pr.generation=r.generation-1 AND pr.bound_sha=r.bound_sha))").map_err(sql)?;
    let t=&birth.target;
    let mut q=s.query(params![&op[..],generation,state,&t.descriptor_sha[..],&t.setup[..]]).map_err(sql)?;
    let row=q.next().map_err(sql)?.ok_or_else(wrong)?;
    let actor=blob(row,0)?;
    let prior=ActiveRustMeta{op:blob(row,3)?,generation:generation.checked_sub(1).ok_or_else(wrong)?,record:blob(row,5)?,..t.clone()};
    if actor!=birth.scope.actor||(prior.generation==1&&prior!=*t){return Err(wrong());}
    let reset=Reset::new(prior,op,actor,blob(row,2)?,blob(row,4)?)?;
    if reset.digest!=blob::<32>(row,1)?||q.next().map_err(sql)?.is_some(){return Err(wrong());}
    Ok(reset)
}
pub(in crate::sqlite_install::pair) fn active_at(tx:&Transaction<'_>,plan:&RustPlan)->io::Result<ActiveRustMeta>{
    let Original::Committed(birth)=Birth::original(tx,plan)? else{return Err(wrong());};
    birth.validate_plan(tx,plan)?;
    let a=birth.first();for(f,k)in plan.folders(){a.original(tx,f,*k)?;}
    let mut s=tx.prepare("SELECT credential_op,generation,record_id FROM main.active_shared_credential WHERE slot=1").map_err(sql)?;
    let mut q=s.query([]).map_err(sql)?;
    let row=q.next().map_err(sql)?.ok_or_else(wrong)?;
    let op=blob(row,0)?;let generation:i64=row.get(1).map_err(sql)?;let record=blob::<16>(row,2)?;
    if q.next().map_err(sql)?.is_some(){return Err(wrong());}
    let t=if generation==1&&op==birth.scope.op{birth.target}else if generation>=2{select_reset(tx,&birth,op,generation,1)?.target}else{return Err(wrong());};
    if t.record!=record{return Err(wrong());}Ok(t)
}

/// Pending or historical nonsecret comparison; the caller supplies the real
/// checked root/worker and never derives or uses a password for a receipt.
pub(in crate::sqlite_install::pair) struct RustChange{actual:Change}
enum Change{Setup(Birth),Reset(Reset)}
impl RustChange{
    pub(in crate::sqlite_install::pair) fn for_fresh(plan:&RustPlan,order:&RustOrder,init_sha:[u8;32])->io::Result<Self>{
        let Desired::Setup(spec)=&order.role else{return Err(wrong());};
        if order.op!=plan.origin.setup{return Err(wrong());}
        Ok(Self{actual:Change::Setup(Birth::new(plan,init_sha,order.actor,order.approval,spec)?)})
    }
    fn core(&self)->Core{match &self.actual{Change::Setup(b)=>b.scope,Change::Reset(r)=>r.scope}}
    pub(in crate::sqlite_install::pair) fn digest(&self)->[u8;32]{
        match &self.actual{Change::Setup(b)=>b.digest,Change::Reset(r)=>r.digest}
    }
    fn head(&self)->&ActiveRustMeta{match &self.actual{Change::Setup(b)=>&b.target,Change::Reset(r)=>&r.target}}
    pub(in crate::sqlite_install::pair) fn target(&self)->ActiveRustMeta{self.head().clone()}
    pub(in crate::sqlite_install::pair) fn previous(&self)->Option<&ActiveRustMeta>{
        match &self.actual{Change::Setup(_)=>None,Change::Reset(r)=>Some(&r.prior)}
    }
    fn same(&self,b:&Self)->bool{self.digest()==b.digest()&&self.head()==b.head()&&self.previous()==b.previous()}
}
pub(in crate::sqlite_install::pair) fn observed_binding(nonce:[u8;32],action:[u8;32],target:&ActiveRustMeta,previous:Option<&ActiveRustMeta>)->io::Result<Binding>{
    Binding::new(nonce,action,target.account,target.epoch(),previous.map(ActiveRustMeta::epoch),target.record_prefix()?)
}
pub(in crate::sqlite_install::pair) fn require_observed(plan:&RustPlan,order:&RustOrder,digest:[u8;32],
    target:&ActiveRustMeta,previous:Option<&ActiveRustMeta>)->io::Result<()>{
    if !target.matches(plan)||order.op!=target.op{return Err(wrong());}
    let expect=match (&order.role,previous){
        (Desired::Setup(_),None)=>RustChange::for_fresh(plan,order,target.init_sha)?,
        (Desired::Reset{previous,generation},Some(old)) if (*previous,*generation)==(old.op,old.generation)&&old.matches(plan)=>
            RustChange{actual:Change::Reset(Reset::new(old.clone(),order.op,order.actor,order.approval,target.record)?)},
        _=>return Err(wrong()),
    };
    if expect.head()!=target||expect.digest()!=digest{return Err(wrong());}Ok(())
}

pub(in crate::sqlite_install::pair) struct RustAccountCut<'s,'p>{
    cut:AccountCut<'s>,plan:&'p RustPlan,order:RustOrder,init_sha:Option<[u8;32]>,
}
impl<'s,'p> RustAccountCut<'s,'p>{
    pub(in crate::sqlite_install::pair) fn open_deployed_reset(locked:&'s LockedSystem,plan:&'p RustPlan,order:RustOrder,
        site:&super::super::fresh_issuer::SitePayload)->io::Result<Self>{
        super::super::fresh_issuer::compiled_v18()?;
        let (actor,uid,_)=site.subject.tuple();
        if !matches!(&order.role,Desired::Reset{..})||actor!=order.actor||site.origin!=plan.origin||
            site.plan_sha!=plan.sha()||locked.source().path()!=site.system||locked.source().uid()!=uid{return Err(wrong());}
        let p=plan.origin;
        let s=Self{cut:AccountCut::at(locked,p.pool,p.init)?,plan,order,init_sha:None};
        s.cut.read(|tx|s.original_origin(tx))?;Ok(s)
    }
    pub(in crate::sqlite_install::pair) fn open_new_first(locked:&'s LockedSystem,plan:&'p RustPlan,order:RustOrder,
        init_sha:[u8;32],database:[u64;3])->io::Result<Self>{
        super::super::fresh_issuer::require_v18()?;
        RustChange::for_fresh(plan,&order,init_sha)?;
        let p=plan.origin;
        let s=Self{cut:AccountCut::at(locked,p.pool,p.init)?,plan,order,init_sha:Some(init_sha)};
        s.cut.fresh_origin(database)?;
        s.cut.read(|tx|s.before_new(tx))?;Ok(s)
    }
    fn original_origin(&self,tx:&Transaction<'_>)->io::Result<()>{
        let sha=committed_init_digest(tx,self.plan,self.order.actor)?;
        if let Some(expected)=self.init_sha{if expected!=sha{return Err(wrong());}}
        else{self.cut.deployed_origin(tx,&self.order.actor)?;}
        Ok(())
    }
    fn before_new(&self,tx:&Transaction<'_>)->io::Result<()>{
        self.original_origin(tx)?;
        if let Some(sha)=self.init_sha{
            let o=self.plan.origin;super::super::observed::isolated_first_init(tx,&o.pool,&o.init,&self.order.actor,&sha)?;
        }
        if !matches!(self.original(tx)?,Original::Absent){return Err(wrong());}Ok(())
    }
    fn original(&self,tx:&Transaction<'_>)->io::Result<Original<RustChange>>{
        let r=match &self.order.role{
            Desired::Setup(_)=>match Birth::original(tx,self.plan)?{
                Original::Absent=>return Ok(Original::Absent),
                Original::Pending(b)=>(0,Change::Setup(b)),
                Original::Committed(b)=>{
                    b.validate_plan(tx,self.plan)?;let first=b.first();
                    for (f,k) in self.plan.folders(){first.original(tx,f,*k)?;}(1,Change::Setup(b))
                },
            },
            Desired::Reset{generation,..}=>{
                let op=self.order.op;
                let mut st=tx.prepare("SELECT state FROM main.operation WHERE op_id=?1 AND kind=9").map_err(sql)?;
                let mut q=st.query([&op[..]]).map_err(sql)?;
                let Some(row)=q.next().map_err(sql)? else{if collision(tx,&op)?{return Err(wrong());}return Ok(Original::Absent);};
                let state=row.get(0).map_err(sql)?;if q.next().map_err(sql)?.is_some(){return Err(wrong());}
                let Original::Committed(b)=Birth::original(tx,self.plan)? else{return Err(wrong());};
                (state,Change::Reset(select_reset(tx,&b,op,generation.checked_add(1).ok_or_else(wrong)?,state)?))
            }
        };
        let result=RustChange{actual:r.1};
        require_observed(self.plan,&self.order,result.digest(),result.head(),result.previous())?;
        history(r.0,result)
    }
    pub(in crate::sqlite_install::pair) fn observe(&self)->io::Result<Original<RustChange>>{
        self.cut.read(|tx|{self.original_origin(tx)?;self.original(tx)})
    }
    pub(in crate::sqlite_install::pair) fn propose_new(&self)->io::Result<RustChange>{
        self.cut.read(|tx|{
            self.before_new(tx)?;
            match &self.order.role{
                Desired::Setup(_)=>{
                    let c=RustChange::for_fresh(self.plan,&self.order,self.init_sha.ok_or_else(wrong)?)?;
                    let Change::Setup(b)=&c.actual else{return Err(wrong());};
                    b.validate_plan(tx,self.plan)?;first_ordinary::vacant(tx,self.plan.folders())?;Ok(c)
                },
                Desired::Reset{previous,generation}=>{
                    let old=active_at(tx,self.plan)?;
                    if (old.op,old.generation)!=(*previous,*generation){return Err(wrong());}
                    forbid_unsettled_births(tx,old.op)?;
                    Ok(RustChange{actual:Change::Reset(Reset::new(old,self.order.op,self.order.actor,self.order.approval,random()?)?)})
                }
            }
        })
    }
    pub(in crate::sqlite_install::pair) fn objects(&self)
      ->io::Result<(std::fs::File,[u8;64],crate::sqlite_install::markers::SystemObservation)>{
        self.cut.original_objects()
    }
    pub(in crate::sqlite_install::pair) fn reserve_after_staging<'a>(&'a self,change:RustChange,nonce:[u8;32],
       web:Received,until:Instant,live:impl Fn()->io::Result<()>)->io::Result<RustPending<'a,'s,'p>>{
        require_observed(self.plan,&self.order,change.digest(),change.head(),change.previous())?;
        let binding=observed_binding(nonce,change.digest(),change.head(),change.previous())?;
        let material=self.cut.write(until,&live,|tx|{
            self.before_new(tx)?;
            let w=stage_shared(tx,self.plan,&change,&binding,web)?;
            self.require_original_pending(tx,&change)?;
            Ok(w)
        })?; // any error after touching the row drops/rolls back this IMMEDIATE transaction
        live()?;self.cut.sync()?;expired(until)?;
        let p=RustPending{owner:self,change,binding,material,until};p.require()?;live()?;Ok(p)
    }
    fn require_original_pending(&self,tx:&Transaction<'_>,change:&RustChange)->io::Result<()>{
        self.original_origin(tx)?;
        if !matches!(self.original(tx)?,Original::Pending(observed) if observed.same(change)){return Err(wrong());}
        no_active(tx)
    }
}

fn stage_shared(tx:&Transaction<'_>,plan:&RustPlan,c:&RustChange,binding:&Binding,value:Received)->io::Result<StagedWeb>{
    match &c.actual{Change::Setup(b)=>b.stage_identity(tx,plan)?,Change::Reset(r)=>r.stage_identity(tx,plan)?,}
    let web=value.stage(binding,tx)?;
    write_record(tx,c.head())?;
    stage(tx,&c.core(),if c.previous().is_none(){2}else{9},&c.digest())?;
    web.require_stored(binding,tx)?;no_active(tx)?;Ok(web)
}
fn activate(tx:&Transaction<'_>,plan:&RustPlan,c:&RustChange,binding:&Binding,web:&StagedWeb)->io::Result<()>{
    let t=c.head();let s=c.core();let old=c.previous();
    let pre_op=old.map(|p|&p.op[..]);let pre_gen=old.map(|p|p.generation);let pre_rec=old.map(|p|&p.record[..]);
    if let Change::Setup(b)=&c.actual{b.validate_plan(tx,plan)?;first_ordinary::vacant(tx,plan.folders())?;}
    web.require_stored(binding,tx)?;
    {
        let mut st=tx.prepare(ACTIVATE).map_err(sql)?;
        let mut q=st.query(params![&t.pool[..],&t.init[..],&t.init_sha[..],&t.setup[..],&t.op[..],&s.actor[..],&c.digest()[..],
            &s.approval[..],&t.instance[..],&t.bind[..],&t.descriptor_sha[..],&t.account[..],&t.login,&t.sid[..],
            t.generation,&t.record[..],pre_op,pre_gen,pre_rec]).map_err(sql)?;
        if blob::<16>(q.next().map_err(sql)?.ok_or_else(wrong)?,0)?!=t.op||q.next().map_err(sql)?.is_some(){return Err(wrong());}
    }
    if let Change::Setup(b)=&c.actual{
        let a=b.first();a.require_first_web(tx)?;for(f,k)in plan.folders(){a.insert(tx,f,*k)?;}
    }
    web.require_stored(binding,tx)?;
    if active_at(tx,plan)?!=*t{return Err(wrong());}Ok(())
}

#[must_use]
pub(in crate::sqlite_install::pair) struct RustPending<'a,'s,'p>{
    owner:&'a RustAccountCut<'s,'p>,change:RustChange,binding:Binding,material:StagedWeb,until:Instant,
}
impl RustPending<'_,'_,'_>{
    pub(in crate::sqlite_install::pair) fn change(&self)->&RustChange{&self.change}
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        expired(self.until)?;
        self.owner.cut.read(|tx|{
            self.owner.require_original_pending(tx,&self.change)?;
            self.material.require_stored(&self.binding,tx)
        })
    }
    /// The original live vault check uses the actual placed O_PATH. No
    /// verifier or permission is introduced by a historic SQL observation.
    pub(in crate::sqlite_install::pair) fn finish(self,live_vault:impl Fn()->io::Result<()>)->io::Result<ActiveRustMeta>{
        self.require()?;live_vault()?;
        let owner=self.owner;
        owner.cut.write(self.until,&live_vault,|tx|{
            owner.require_original_pending(tx,&self.change)?;
            activate(tx,owner.plan,&self.change,&self.binding,&self.material)
        })?; // rollback on any failed row, vault, parent or exact readback inside the cut
        owner.cut.sync()?;
        let result=owner.cut.read(|tx|{
            owner.original_origin(tx)?;
            match owner.original(tx)?{
                Original::Committed(ref observed) if observed.same(&self.change)=>(),
                _=>return Err(wrong()),
            }
            self.material.require_stored(&self.binding,tx)?;
            active_at(tx,owner.plan)
        })?;
        expired(self.until)?;live_vault()?;
        if result!=*self.change.head(){return Err(wrong());}Ok(result)
    }
}

#[cfg(test)]
#[path = "../../../../tests/sqlite/shared_account_transactions.rs"]
mod driver_tests;

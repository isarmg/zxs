//! This is the nonsecret SQL half of the Rust vault family. Functions that
//! write a transaction are private to the offline transaction owner. They
//! do not acquire either EX lock, open a vault, or authorize maintenance.
use super::{Core,Id,Ids,Original,bad,blob,collision,distinct_roles,first_ordinary,history,login,one,require_init,sql,stage};
use super::super::web_secret::PreparedWeb;
use crate::smb_profile::rust_first::RustPlan;
use rusqlite::{params,Transaction};
use sha2::{Digest,Sha256};
use std::io;

const SID_PREFIX:[u8;12]=[1,5,0,0,0,0,0,5,21,0,0,0];
const ACTIVATE:&str=include_str!("activate-rust.sql");

#[derive(Clone,Eq,PartialEq)]
pub(in crate::sqlite_install::pair) struct ActiveRustMeta{
    pub(in crate::sqlite_install::pair) pool:Id,
    pub(in crate::sqlite_install::pair) init:Id,
    pub(in crate::sqlite_install::pair) setup:Id,
    pub(in crate::sqlite_install::pair) descriptor_sha:[u8;32],
    pub(in crate::sqlite_install::pair) instance:Id,
    pub(in crate::sqlite_install::pair) bind:Id,
    pub(in crate::sqlite_install::pair) account:Id,
    pub(in crate::sqlite_install::pair) login:Vec<u8>,
    pub(in crate::sqlite_install::pair) sid:[u8;28],
    pub(in crate::sqlite_install::pair) op:Id,
    pub(in crate::sqlite_install::pair) generation:i64,
    pub(in crate::sqlite_install::pair) record:Id,
}
impl ActiveRustMeta{
    /// Canonical nonsecret envelope for the exact selected library and
    /// root-owned staging inode. Never copy the last 16 bytes to SQL or IPC.
    pub(in crate::sqlite_install::pair) fn record_prefix(&self)->io::Result<[u8;224]>{
        let ids=[self.pool,self.init,self.setup,self.instance,self.bind,self.account,self.op,self.record];
        if !login(&self.login)||!distinct_roles(&[],&self.login)||ids.contains(&[0;16])||
            self.init==self.setup||self.op==self.init||self.generation<1||
            (self.generation==1)!=(self.op==self.setup)||!valid_sid(&self.sid){return Err(public_error());}
        let mut b=[0u8;224];b[..16].copy_from_slice(b"ZXS-RUST-SMB-V1\0");
        b[16]=1;b[17]=self.login.len() as u8;
        for (slot,id) in b[24..152].chunks_exact_mut(16).zip(ids){slot.copy_from_slice(&id);}
        b[152..160].copy_from_slice(&(self.generation as u64).to_be_bytes());
        b[160..160+self.login.len()].copy_from_slice(&self.login);
        Ok(b)
    }
}
#[derive(Clone,Copy,Eq,PartialEq)]
struct Installation{pool:Id,init:Id,setup:Id,instance:Id,bind:Id,sha:[u8;32]}
impl From<&RustPlan> for Installation{
    fn from(p:&RustPlan)->Self{
        let o=p.origin;Self{pool:o.pool,init:o.init,setup:o.setup,instance:o.instance,bind:o.bind,sha:p.sha()}
    }
}
fn valid_sid(sid:&[u8;28])->bool{
    sid[..12]==SID_PREFIX&&sid[12..24]!=[0;12]&&sid[24..]==1000u32.to_le_bytes()
}
fn public_name(h:&mut Sha256,name:&[u8]){
    h.update([name.len() as u8]);h.update(name);
}
fn public_error()->io::Error{bad("wrong exact Rust SMB account operation or public installation")}

struct Birth{
    scope:Core,install:Installation,ids:Ids,web:Vec<u8>,smb:Vec<u8>,sid:[u8;28],record:Id,
}
impl Birth{
    fn new(plan:&RustPlan,actor:Id,approval:Id,ids:Ids,web:&[u8],smb:&[u8],sid:[u8;28],record:Id)->io::Result<Self>{
        let install=Installation::from(plan);
        let scope=Core::new(install.pool,install.init,install.setup,actor,approval)?;
        if install.sha==[0;32]||record==[0;16]||!login(web)||!login(smb)||!distinct_roles(web,smb)||!valid_sid(&sid){
            return Err(public_error());}
        Ok(Self{scope,install,ids,web:web.to_vec(),smb:smb.to_vec(),sid,record})
    }
    fn sha(&self)->[u8;32]{
        let c=self.scope;let i=self.install;
        let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/RUST-SMB-PAIR-1\0");
        for id in [c.pool,c.init,c.op,c.actor,c.approval,i.instance,i.bind,self.ids.web,self.ids.smb,self.record]{h.update(id);}
        h.update(i.sha);public_name(&mut h,&self.web);public_name(&mut h,&self.smb);h.update(self.sid);
        h.finalize().into()
    }
    fn target(&self)->ActiveRustMeta{
        let i=self.install;
        ActiveRustMeta{pool:i.pool,init:i.init,setup:i.setup,descriptor_sha:i.sha,instance:i.instance,bind:i.bind,
            account:self.ids.smb,login:self.smb.clone(),sid:self.sid,op:i.setup,generation:1,record:self.record}
    }

    // The static first-plan authority is only the transaction-local right to
    // create the exact original rows. It never becomes an online Web user.
    fn first(&self)->first_ordinary::Authority{
        first_ordinary::Authority{pool:self.install.pool,setup:self.install.setup,web:self.ids.web}
    }
    fn original(tx:&Transaction<'_>,plan:&RustPlan)->io::Result<Original<Self>>{
        let i=Installation::from(plan);
        let mut s=tx.prepare("SELECT o.state,o.actor_id,o.request_sha,a.approval_ref,a.web_id,a.web_login,
                     a.smb_id,a.smb_login,a.smb_sid,r.record_id
                FROM main.account_pair a JOIN main.operation o ON o.op_id=a.birth_op AND o.kind=2
                JOIN main.smb_rust_revision r ON r.credential_op=a.birth_op
                WHERE a.singleton=1 AND a.smb_family=1 AND a.birth_op=?1 AND a.instance_id=?2
                  AND a.setup_bound_sha=?3 AND a.smb_rust_bind_id=?4").map_err(sql)?;
        let mut q=s.query(params![&i.setup[..],&i.instance[..],&i.sha[..],&i.bind[..]]).map_err(sql)?;
        let Some(row)=q.next().map_err(sql)? else{
            let seen:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.account_pair) OR
                    EXISTS(SELECT 1 FROM main.operation WHERE kind=2) OR
                    EXISTS(SELECT 1 FROM main.web_password)",[],|r|r.get(0)).map_err(sql)?;
            if seen||collision(tx,&i.setup)?{return Err(public_error());}
            return Ok(Original::Absent);
        };
        let web=row.get_ref(5).map_err(sql)?.as_blob().map_err(|_|public_error())?;
        let smb=row.get_ref(7).map_err(sql)?.as_blob().map_err(|_|public_error())?;
        let birth=Self::new(plan,blob(row,1)?,blob(row,3)?,Ids::checked(blob(row,4)?,blob(row,6)?)?,
            web,smb,blob(row,8)?,blob(row,9)?)?;
        let tag=blob::<32>(row,2)?;let state=row.get(0).map_err(sql)?;
        if birth.sha()!=tag||q.next().map_err(sql)?.is_some(){return Err(public_error());}
        require_init(tx,&birth.scope)?;history(state,birth)
    }
    fn validate_plan(&self,tx:&Transaction<'_>,plan:&RustPlan)->io::Result<()>{
        if self.install!=Installation::from(plan){return Err(public_error());}
        require_init(tx,&self.scope)?;
        let r:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.volume_claim
                   WHERE claim_op=?1 AND ordinal=1 AND role=1)",[&self.scope.init[..]],|r|r.get(0)).map_err(sql)?;
        if !r{return Err(public_error());}
        first_ordinary::volumes(tx,&self.scope.init,plan.folders())
    }
    fn reserve(&self,tx:&Transaction<'_>,plan:&RustPlan,verifier:&PreparedWeb)->io::Result<()>{
        self.validate_plan(tx,plan)?;
        if !matches!(Self::original(tx,plan)?,Original::Absent){return Err(public_error());}
        first_ordinary::vacant(tx,plan.folders())?;
        let c=self.scope;let i=self.install;
        one(tx.execute("INSERT INTO main.account_pair(singleton,birth_op,approval_ref,instance_id,setup_bound_sha,
                        web_id,web_login,smb_id,smb_login,smb_sid,smb_family,smb_rust_bind_id)
                        VALUES(1,?1,?2,?3,?4,?5,?6,?7,?8,?9,1,?10)",
            params![&c.op[..],&c.approval[..],&i.instance[..],&i.sha[..],&self.ids.web[..],&self.web,
                &self.ids.smb[..],&self.smb,&self.sid[..],&i.bind[..]]))?;
        one(verifier.insert(tx,&c.op))?;
        record(tx,&c.op,&self.record)?;
        stage(tx,&c,2,&self.sha())?;
        if !matches!(Self::original(tx,plan)?,Original::Pending(p) if p.sha()==self.sha()){return Err(public_error());}
        Ok(())
    }
    // Must be called by the trusted low writer only after the durable, exact
    // reopened vault witness. On any subsequent error roll the whole SQL cut back.
    fn finish(&self,tx:&Transaction<'_>,plan:&RustPlan)->io::Result<()>{
        self.validate_plan(tx,plan)?;
        if !matches!(Self::original(tx,plan)?,Original::Pending(p) if p.sha()==self.sha()){return Err(public_error());}
        first_ordinary::vacant(tx,plan.folders())?;
        activate(tx,self.scope,&self.target(),&self.sha(),None)?;
        let a=self.first();a.require_first_web(tx)?;
        for(f,k)in plan.folders(){a.insert(tx,f,*k)?;}
        if active_at(tx,plan)?!=self.target(){return Err(public_error());}
        Ok(())
    }
}

struct Reset{scope:Core,previous:ActiveRustMeta,record:Id}
impl Reset{
    fn with_original(previous:ActiveRustMeta,op:Id,actor:Id,approval:Id,record:Id)->io::Result<Self>{
        let c=Core::new(previous.pool,previous.init,op,actor,approval)?;
        if [previous.setup,previous.op].contains(&op)||[previous.record,[0;16]].contains(&record)||
           !(1..i64::MAX).contains(&previous.generation)||(previous.op==previous.setup)!=(previous.generation==1){
            return Err(public_error());}
        Ok(Self{scope:c,previous,record})
    }
    fn sha(&self)->[u8;32]{
        let c=self.scope;let p=&self.previous;
        let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/RUST-SMB-RESET-1\0");
        for id in [c.pool,c.init,p.setup,c.op,c.actor,c.approval,p.instance,p.bind,p.account,p.op,p.record,self.record]{h.update(id);}
        h.update(p.generation.to_be_bytes());h.update(p.descriptor_sha);
        public_name(&mut h,&p.login);h.update(p.sid);h.finalize().into()
    }
    fn target(&self)->ActiveRustMeta{
        ActiveRustMeta{op:self.scope.op,generation:self.previous.generation+1,record:self.record,..self.previous.clone()}
    }
    fn reserve(&self,tx:&Transaction<'_>,plan:&RustPlan)->io::Result<()>{
        let c=self.scope;let p=&self.previous;
        if collision(tx,&c.op)?||active_at(tx,plan)?!=*p{return Err(public_error());}
        one(tx.execute("INSERT INTO main.credential_reset(op_id,role,previous_op,generation,approval_ref,bound_sha)
                        VALUES(?1,1,?2,?3,?4,?5)",
            params![&c.op[..],&p.op[..],p.generation+1,&c.approval[..],&p.descriptor_sha[..]]))?;
        record(tx,&c.op,&self.record)?;
        stage(tx,&c,7,&self.sha())?;
        self.require(tx,plan,0)
    }
    fn require(&self,tx:&Transaction<'_>,plan:&RustPlan,state:i64)->io::Result<()>{
        let target=self.target();let c=self.scope;
        let Original::Committed(birth)=Birth::original(tx,plan)? else{return Err(public_error());};
        birth.validate_plan(tx,plan)?;
        let q=select_reset(tx,&birth,target.op,target.generation,state)?;
        if q.sha()!=self.sha()||q.previous!=self.previous||q.target()!=target||q.scope.approval!=c.approval{return Err(public_error());}
        Ok(())
    }
    // The caller retains the exact old and new public record IDs while the
    // old position is disabled. There is never fallback to the old record.
    fn finish(&self,tx:&Transaction<'_>,plan:&RustPlan)->io::Result<()>{
        self.require(tx,plan,0)?;
        activate(tx,self.scope,&self.target(),&self.sha(),Some((&self.previous.op,&self.previous.record)))?;
        self.require(tx,plan,1)?;
        if active_at(tx,plan)?!=self.target(){return Err(public_error());}
        Ok(())
    }
}

fn record(tx:&Transaction<'_>,op:&Id,id:&Id)->io::Result<()>{
    one(tx.execute("INSERT INTO main.smb_rust_revision(credential_op,record_id) VALUES(?1,?2)",
        params![&op[..],&id[..]]))
}
fn activate(tx:&Transaction<'_>,c:Core,m:&ActiveRustMeta,hash:&[u8;32],previous:Option<(&Id,&Id)>)->io::Result<()>{
    let mut s=tx.prepare(ACTIVATE).map_err(sql)?;
    let (previous_op,previous_record)=previous.map_or((None,None),|(o,r)|(Some(&o[..]),Some(&r[..])));
    let mut updated=s.query(params![&m.pool[..],&m.init[..],&m.setup[..],&m.op[..],&c.actor[..],&hash[..],
            &c.approval[..],&m.instance[..],&m.descriptor_sha[..],&m.account[..],&m.login,&m.sid[..],
            m.generation,previous_op,&m.bind[..],&m.record[..],previous_record]).map_err(sql)?;
    let row=updated.next().map_err(sql)?.ok_or_else(public_error)?;
    if blob::<16>(row,0)?!=m.op||updated.next().map_err(sql)?.is_some(){return Err(public_error());}
    Ok(())
}
fn select_reset(tx:&Transaction<'_>,birth:&Birth,op:Id,generation:i64,state:i64)->io::Result<Reset>{
    let mut s=tx.prepare("SELECT o.actor_id,o.request_sha,r.approval_ref,r.previous_op,v.record_id,pv.record_id
        FROM main.credential_reset r
        JOIN main.operation o ON o.op_id=r.op_id AND o.kind=7
        JOIN main.credential_epoch p ON p.role=1 AND p.credential_op=r.previous_op
        JOIN main.operation pred ON pred.op_id=p.credential_op AND pred.state=1 AND pred.kind=CASE WHEN p.generation=1 THEN 2 ELSE 7 END
        JOIN main.smb_rust_revision v ON v.credential_op=r.op_id
        JOIN main.smb_rust_revision pv ON pv.credential_op=r.previous_op
        WHERE r.op_id=?1 AND r.role=1 AND r.generation=?2 AND o.state=?3 AND r.bound_sha=?4
          AND p.generation=r.generation-1").map_err(sql)?;
    let mut q=s.query(params![&op[..],generation,state,&birth.install.sha[..]]).map_err(sql)?;
    let row=q.next().map_err(sql)?.ok_or_else(public_error)?;
    let prev=ActiveRustMeta{op:blob(row,3)?,generation:generation-1,record:blob(row,5)?,..birth.target()};
    if prev.record==[0;16]||(prev.generation==1&&prev.record!=birth.record){return Err(public_error());}
    let order=Reset::with_original(prev,op,blob(row,0)?,blob(row,2)?,blob(row,4)?)?;
    if order.sha()!=blob::<32>(row,1)?||q.next().map_err(sql)?.is_some(){return Err(public_error());}
    Ok(order)
}

/// One own canonical transaction only. The descriptor and this public SQL
/// stamp still need the actual trusted site and protected record FD. Never
/// pick the most recent committed older generation while a reset is pending.
pub(in crate::sqlite_install::pair) fn active_at(tx:&Transaction<'_>,plan:&RustPlan)->io::Result<ActiveRustMeta>{
    let Original::Committed(birth)=Birth::original(tx,plan)? else{return Err(public_error());};
    birth.validate_plan(tx,plan)?;
    let a=birth.first();for(f,k)in plan.folders(){a.original(tx,f,*k)?;}
    let mut s=tx.prepare("SELECT e.credential_op,e.generation FROM main.active_credential e
        WHERE e.slot=1 AND e.role=1 AND e.generation=COALESCE((SELECT max(generation) FROM main.credential_reset WHERE role=1),1)")
        .map_err(sql)?;
    let mut q=s.query([]).map_err(sql)?;
    let row=q.next().map_err(sql)?.ok_or_else(public_error)?;
    let op=blob::<16>(row,0)?;let generation:i64=row.get(1).map_err(sql)?;
    if q.next().map_err(sql)?.is_some(){return Err(public_error());}
    if generation==1&&op==birth.scope.op{return Ok(birth.target());}
    if generation<2{return Err(public_error());}
    Ok(select_reset(tx,&birth,op,generation,1)?.target())
}

use super::transaction::{AccountCut,expired};
use crate::sqlite_install::{originals::LockedSystem,stage::random};
use std::time::Instant;

/// This is the public order, not the operator's permission or a vault FD.
#[derive(Debug)]
pub(in crate::sqlite_install::pair) enum Desired{
    Setup{web:Vec<u8>,smb:Vec<u8>},
    Reset{previous:Id,generation:i64},
}
#[derive(Debug)]
pub(in crate::sqlite_install::pair) struct RustOrder{
    pub actor:Id,pub approval:Id,pub op:Id,pub role:Desired,
}
pub(in crate::sqlite_install::pair) struct RustChange{actual:Change}
enum Change{Setup(Birth),Reset(Reset)}
impl RustChange{
    pub(in crate::sqlite_install::pair) fn digest(&self)->[u8;32]{
        match &self.actual{Change::Setup(b)=>b.sha(),Change::Reset(r)=>r.sha()}
    }
    pub(in crate::sqlite_install::pair) fn target(&self)->ActiveRustMeta{
        match &self.actual{Change::Setup(b)=>b.target(),Change::Reset(r)=>r.target()}
    }
    pub(in crate::sqlite_install::pair) fn previous(&self)->Option<&ActiveRustMeta>{
        match &self.actual{Change::Setup(_)=>None,Change::Reset(r)=>Some(&r.previous)}
    }
    pub(in crate::sqlite_install::pair) fn web_account(&self)->Option<Id>{
        match &self.actual{Change::Setup(b)=>Some(b.ids.web_value()),Change::Reset(_)=>None}
    }
}

/// The actual offline worker retains the lent SYSTEM EX, original database,
/// fixed parent channel and exact descriptor. It has no vault namespace.
pub(in crate::sqlite_install::pair) struct RustAccountCut<'s,'p>{
    cut:AccountCut<'s>,plan:&'p RustPlan,order:RustOrder,init_sha:Option<[u8;32]>,
}
impl<'s,'p> RustAccountCut<'s,'p>{
    pub(in crate::sqlite_install::pair) fn open(locked:&'s LockedSystem,plan:&'p RustPlan,order:RustOrder)->io::Result<Self>{
        Self::open_inner(locked,plan,order,None)
    }
    pub(in crate::sqlite_install::pair) fn open_new_first(locked:&'s LockedSystem,plan:&'p RustPlan,order:RustOrder,
        init_sha:[u8;32],database:[u64;3])->io::Result<Self>{
        super::super::fresh_issuer::require_v17()?;
        if !matches!(&order.role,Desired::Setup{..}){return Err(public_error());}
        let s=Self::open_inner(locked,plan,order,Some(init_sha))?;
        s.cut.fresh_origin(database)?;
        s.cut.read(|tx|s.require_initial_only(tx))?;
        Ok(s)
    }
    fn open_inner(locked:&'s LockedSystem,plan:&'p RustPlan,order:RustOrder,init_sha:Option<[u8;32]>)->io::Result<Self>{
        let o=plan.origin;
        let core=Core::new(o.pool,o.init,order.op,order.actor,order.approval)?;
        match &order.role{
            Desired::Setup{web,smb} if order.op==o.setup&&login(web)&&login(smb)&&distinct_roles(web,smb)=>{},
            Desired::Reset{previous,generation} if ![[0;16],o.init,o.bind,order.op].contains(previous)&&
                ![o.setup,o.bind].contains(&order.op)&&(1..i64::MAX).contains(generation)&&(*previous==o.setup)==(*generation==1)=>{},
            _=>return Err(public_error()),
        }
        Ok(Self{cut:AccountCut::at(locked,core.pool,core.init)?,plan,order,init_sha})
    }
    fn require_initial_only(&self,tx:&Transaction<'_>)->io::Result<()>{
        if let Some(init_sha)=self.init_sha{
            let o=self.plan.origin;
            super::super::observed::isolated_first_init(tx,&o.pool,&o.init,&self.order.actor,&init_sha)?;
        }
        Ok(())
    }
    fn order_scope(&self,core:Core)->io::Result<()>{
        let o=self.plan.origin;let c=&self.order;
        if (core.pool,core.init,core.op,core.actor,core.approval)!=(o.pool,o.init,c.op,c.actor,c.approval){
            return Err(public_error());
        }
        Ok(())
    }
    fn request(&self,change:&RustChange)->io::Result<()>{
        match (&self.order.role,&change.actual){
            (Desired::Setup{web,smb},Change::Setup(b))
                if b.install==Installation::from(self.plan) && &b.web==web && &b.smb==smb=>self.order_scope(b.scope),
            (Desired::Reset{previous,generation},Change::Reset(r))
                if r.previous.op==*previous&&r.previous.generation==*generation=>self.order_scope(r.scope),
            _=>Err(public_error()),
        }
    }
    fn original_in(&self,tx:&Transaction<'_>)->io::Result<Original<RustChange>>{
        match &self.order.role{
            Desired::Setup{..}=>{
                let (state,birth)=match Birth::original(tx,self.plan)?{
                    Original::Absent=>return Ok(Original::Absent),
                    Original::Pending(b)=>(0,b),
                    Original::Committed(b)=>(1,b),
                };
                if state==1{
                    birth.validate_plan(tx,self.plan)?;
                    let first=birth.first();
                    for(f,k)in self.plan.folders(){first.original(tx,f,*k)?;}
                }
                let change=RustChange{actual:Change::Setup(birth)};
                self.request(&change)?;history(state,change)
            }
            Desired::Reset{generation,..}=>{
                let op=self.order.op;
                let mut q=tx.prepare("SELECT state FROM main.operation WHERE op_id=?1 AND kind=7").map_err(sql)?;
                let mut rows=q.query([&op[..]]).map_err(sql)?;
                let Some(row)=rows.next().map_err(sql)?else{
                    if collision(tx,&op)?{return Err(public_error());}
                    return Ok(Original::Absent);
                };
                let state:i64=row.get(0).map_err(sql)?;
                if rows.next().map_err(sql)?.is_some(){return Err(public_error());}
                let Original::Committed(b)=Birth::original(tx,self.plan)?else{return Err(public_error());};
                let reset=select_reset(tx,&b,op,*generation+1,state)?;
                let change=RustChange{actual:Change::Reset(reset)};
                self.request(&change)?;history(state,change)
            }
        }
    }
    pub(in crate::sqlite_install::pair) fn observe(&self)->io::Result<Original<RustChange>>{
        self.cut.read(|tx|self.original_in(tx))
    }
    pub(in crate::sqlite_install::pair) fn propose_new(&self)->io::Result<RustChange>{
        self.cut.read(|tx|{
            self.require_initial_only(tx)?;
            if !matches!(self.original_in(tx)?,Original::Absent){return Err(public_error());}
            let record:Id=random()?;
            let actual=match &self.order.role{
                Desired::Setup{web,smb}=>{
                    let mut sid=[0u8;28];sid[..12].copy_from_slice(&SID_PREFIX);
                    sid[12..24].copy_from_slice(&random::<12>()?);
                    sid[24..].copy_from_slice(&1000u32.to_le_bytes());
                    let birth=Birth::new(self.plan,self.order.actor,self.order.approval,Ids::checked(random()?,random()?)?,web,smb,sid,record)?;
                    birth.validate_plan(tx,self.plan)?;
                    first_ordinary::vacant(tx,self.plan.folders())?;
                    Change::Setup(birth)
                }
                Desired::Reset{previous,generation}=>{
                    let old=active_at(tx,self.plan)?;
                    if (old.op,old.generation)!=(*previous,*generation){return Err(public_error());}
                    Change::Reset(Reset::with_original(old,self.order.op,self.order.actor,self.order.approval,record)?)
                }
            };
            let result=RustChange{actual};
            self.request(&result)?;
            Ok(result)
        })
    }
    fn same_pending(&self,expected:&RustChange)->io::Result<()>{
        match self.observe()?{
            Original::Pending(current) if current.digest()==expected.digest()&&current.target()==expected.target()=>Ok(()),
            _=>Err(public_error()),
        }
    }
    pub(in crate::sqlite_install::pair) fn confirmed_pending<'a>(&'a self,change:RustChange,until:Instant)
        ->io::Result<RustPending<'a,'s,'p>>{
        expired(until)?;self.same_pending(&change)?;
        // A cold exact pending may have followed an uncertain sync. Sync
        // its original SQLite and directory and independently re-open it.
        self.cut.sync()?;self.same_pending(&change)?;expired(until)?;
        Ok(RustPending{owner:self,change,until})
    }
    /// Called only after the original protected issuer already reaped and
    /// independently re-opened the exact freshly sealed root stage.
    pub(in crate::sqlite_install::pair) fn reserve_after_staging<'a>(
        &'a self,change:RustChange,web:Option<&PreparedWeb>,until:Instant,
        parent_current:impl Fn()->io::Result<()>,
    )->io::Result<RustPending<'a,'s,'p>>{
        self.request(&change)?;
        self.cut.write(until,&parent_current,|tx|{
            self.require_initial_only(tx)?;
            if !matches!(self.original_in(tx)?,Original::Absent){return Err(public_error());}
            match (&change.actual,web){
                (Change::Setup(b),Some(v))=>b.reserve(tx,self.plan,v),
                (Change::Reset(r),None)=>r.reserve(tx,self.plan),
                _=>Err(public_error()),
            }
        })?;
        parent_current()?;
        self.confirmed_pending(change,until)
    }
}

#[must_use]
pub(in crate::sqlite_install::pair) struct RustPending<'a,'s,'p>{
    owner:&'a RustAccountCut<'s,'p>,change:RustChange,until:Instant,
}
impl RustPending<'_,'_,'_>{
    pub(in crate::sqlite_install::pair) fn change(&self)->&RustChange{&self.change}
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        expired(self.until)?;self.owner.same_pending(&self.change)
    }
    /// Only the fixed worker after authentication of the original placed
    /// O_PATH witness. The actual live guard is rechecked across both cuts.
    pub(in crate::sqlite_install::pair) fn finish(self,live_vault:impl Fn()->io::Result<()>)->io::Result<ActiveRustMeta>{
        self.require()?;
        let target=self.change.target();let owner=self.owner;
        owner.cut.write(self.until,&live_vault,|tx|match &self.change.actual{
            Change::Setup(b)=>b.finish(tx,owner.plan),
            Change::Reset(r)=>r.finish(tx,owner.plan),
        })?;
        owner.cut.sync()?;
        let active=owner.cut.read(|tx|active_at(tx,owner.plan))?;
        expired(self.until)?;live_vault()?;
        if active!=target{return Err(public_error());}
        Ok(active)
    }
}

#[cfg(test)]
pub(super) fn web_reset_fixture_pair(tx:&Transaction<'_>,plan:&RustPlan,actor:Id){
    let mut sid=[71;28];sid[..12].copy_from_slice(&SID_PREFIX);sid[24..].copy_from_slice(&1000u32.to_le_bytes());
    let birth=Birth::new(plan,actor,[81;16],Ids::checked([82;16],[83;16]).unwrap(),
        b"admin.fixture",b"smb.fixture",sid,[84;16]).unwrap();
    birth.reserve(tx,plan,&PreparedWeb::synthetic([85;16],[86;32])).unwrap();
    birth.finish(tx,plan).unwrap();
}

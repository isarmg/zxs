//! Fixed two-role control for the one Rust credential. The low SQL worker
//! has no readable vault descriptor. Only the separately reaped low
//! writer ever writes the prepared 240-byte record.
use super::{bad,host::{Worker,prerequisite},transport::{self,Channel,Kind,Payload},worker::original_parent};
use crate::{fs_tools::RuntimeFile,pinned::{Kind as PathKind,Pinned},sqlite_install::{approval,
    originals::SelectedSystem,pair::{
        domain::{Allocated,RustSite,VaultStamp,checked_vault_stamp,finish_seen_pending,give_live_approval,request_live_approval,sealed,verify_fresh_worker,verify_fresh_vault},
        native_sql::{Original,rust_vault::{ActiveRustMeta,Desired,RustAccountCut,RustChange}},
        rust_enroll::{crypto::Cryptography,manifest::{Intent,Manifest,seal_bytes},receipt::Observed,secret_order::{SecretOrder,scope as secret_scope}},
        terminal::{HeldQuiet,PublicConfirmation,SelectedInput},
    }}};
use helper_linux_evidence::diagnostic_process::{OfflineSql,account_sql_descriptors};
use rustix::{fs::{OFlags,fcntl_getfl},net::UCred,process::{DumpableBehavior,Gid,Pid,Signal,Uid,
    dumpable_behavior,getppid,parent_process_death_signal,set_dumpable_behavior,set_parent_process_death_signal}};
use std::{fs::File,io,thread,time::{Duration,Instant}};

const HELPER:&[&str]=&["usr","bin","helper"];
const LIMIT:Duration=Duration::from_secs(12*60);
fn exact_parent(fd:&File,owner:Pid)->io::Result<()>{
    original_parent(fd,owner)?;
    let image=RuntimeFile::open(HELPER,true,64<<20)?;
    image.require_running_helper()?;image.require_parent_helper(owner)
}
fn subject(m:&Manifest,parent:Pid)->io::Result<([u8;16],u32,u32)>{
    let scope=m.fresh.as_ref().ok_or_else(||bad("only the fresh v17 offline account scope is accepted"))?;
    let image=crate::sqlite_install::pair::smb_policy::release(HELPER,0o755,&scope.crypto.helper)?;
    image.require_running_helper()?;image.require_parent_helper(parent)?;
    verify_fresh_worker(&m.system,&m.plan,scope)?;image.recheck()?;
    Ok(scope.subject.tuple())
}
fn isolated(fd:File,parent:Pid,uid:u32,gid:u32,digest:[u8;32])->io::Result<(Channel,Instant)>{
    approval::drop_to_storage(uid,gid)?;
    set_parent_process_death_signal(Some(Signal::TERM))?;
    original_parent(&fd,parent)?;
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    if dumpable_behavior()?!=DumpableBehavior::NotDumpable{return Err(bad("the selected low credential worker is not isolated"));}
    request_live_approval(&fd,parent,&digest,||original_parent(&fd,parent))?;
    let until=Instant::now().checked_add(LIMIT).ok_or_else(||bad("offline Rust worker deadline overflow"))?;
    let channel=Channel::from_owned_child_session(fd,UCred{pid:parent,uid:Uid::ROOT,gid:Gid::ROOT},digest)?;
    Ok((channel,until))
}
fn live_parent(owner:Pid,until:Instant)->io::Result<()>{
    if getppid()!=Some(owner)||parent_process_death_signal()?!=Some(Signal::TERM)||Instant::now()>=until{
        return Err(bad("lost the original live Rust credential owner"));
    }
    Ok(())
}
fn body(tag:u8,id:&[u8;16])->Payload{
    let mut b=[0;65];b[0]=tag;b[1..17].copy_from_slice(id);b
}
fn observed(tag:u8,c:&RustChange)->io::Result<File>{
    let result=Observed{state:tag,action:c.digest(),target:c.target(),previous:c.previous().cloned(),web_account:c.web_account()};
    sealed(&result.encode()?)
}

struct Placed {original:File,mount:u64,stamp:VaultStamp}
impl Placed {
    fn receive(channel:&Channel,expected:&RustChange,until:Instant)->io::Result<Self>{
        let (p,[original])=channel.wait::<1>(Kind::RustVaultPlaced,&expected.digest(),until)?;
        if p[0]!=3||p[1..17]!=expected.target().record{return Err(bad("different original Rust placed vault"));}
        let u=|i:usize|u64::from_be_bytes(p[i..i+8].try_into().expect("fixed vault shape"));
        let i=|i:usize|i64::from_be_bytes(p[i..i+8].try_into().expect("fixed vault shape"));
        let n=|i:usize|u32::from_be_bytes(p[i..i+4].try_into().expect("fixed vault shape"));
        let selected=Placed{original,mount:u(17),stamp:VaultStamp{dev:u(25),ino:u(33),
            ctime_sec:i(41),ctime_ns:n(49),mtime_sec:i(53),mtime_ns:n(61)}};
        selected.require()?;Ok(selected)
    }
    fn require(&self)->io::Result<()>{
        if !fcntl_getfl(&self.original)?.contains(OFlags::PATH)||self.mount==0||
            checked_vault_stamp(&self.original,self.mount)?!=self.stamp{
            return Err(bad("the selected Rust vault witness is not the original root-owned O_PATH"));
        }
        Ok(())
    }
}
pub(crate) fn account_worker()->io::Result<()>{
    let [fd,meta]=account_sql_descriptors()?;
    approval::only_inherited_descriptors(&[3,4])?;
    let parent=getppid().ok_or_else(||bad("the offline Rust account owner is absent"))?;
    exact_parent(&fd,parent)?;
    let host=approval::host_admin()?;
    let m=Manifest::read(&meta)?;drop(meta);
    let (actor,uid,gid)=subject(&m,parent)?;
    if m.order.actor!=actor{return Err(bad("wrong original Rust maintenance principal"));}
    let scope=m.scope()?;
    let input=(m.intent==Intent::Fresh&&matches!(&m.order.role,Desired::Setup{..})).then(SelectedInput::before_drop);
    let (channel,until)=isolated(fd,parent,uid,gid,scope)?;
    let (zero,[lease])=channel.wait_checked::<1>(Kind::Lease,&scope,until,||live_parent(parent,until))?;
    if zero!=[0;65]{return Err(bad("wrong original SYSTEM lease"));}
    let pinned=Pinned::already_mounted(&m.system,PathKind::StorageRoot)?;let e=pinned.entity();
    if [e.dev,e.ino,pinned.require_same_named_mount()?.id]!=m.entity{return Err(bad("different original SYSTEM"));}
    let locked=SelectedSystem::from_origin(pinned,host,uid,gid)?.from_lent_ex(lease)?;
    let f=m.fresh.ok_or_else(||bad("no exact fresh issuance scope"))?;
    let cut=RustAccountCut::open_new_first(&locked,&m.plan,m.order,f.init_sha,f.database)?;
    let alive=||{live_parent(parent,until)?;locked.require()};
    let prior=cut.observe()?;
    if m.fresh.is_some()&&!matches!(&prior,Original::Absent){return Err(bad("this route is fresh-only; retain the original"));}
    let (tag,change)=match (m.intent,prior){
        (Intent::Fresh,Original::Absent)=>(1,cut.propose_new()?),
        (_,Original::Absent)=>{
            channel.send_original(Kind::RustInspect,&scope,&[0;65],&sealed(&[0])?)?;
            return Ok(());
        },
        (_,Original::Pending(v))=>(2,v),(_,Original::Committed(v))=>(3,v),
    };
    let target=change.target();let digest=change.digest();
    channel.send_original(Kind::RustInspect,&scope,&body(tag,&target.record),&observed(tag,&change)?)?;
    if !matches!((m.intent,tag),(Intent::Fresh,1)|(Intent::Resume,2)){return Ok(());}
    let selected=input.transpose()?;
    let prepared=if tag==1&&selected.is_some(){
        let (warm,[])=channel.wait_checked::<0>(Kind::Warm,&digest,until,&alive)?;
        if warm!=body(1,&target.record){return Err(bad("different original Web input preparation"));}
        let web=selected.as_ref().ok_or_else(||bad("no actual selected Web input"))?.reserve_web(uid,gid)?;
        channel.send(Kind::Prepared,&digest,&body(1,&target.record))?;
        Some(web)
    }else{None};
    let (ready,[])=channel.wait_checked::<0>(Kind::RustStage,&digest,until,&alive)?;
    if ready!=body(tag,&target.record){return Err(bad("wrong original Rust vault stage admission"));}
    let pending=if tag==1{
        let web=prepared.map(|p|p.read_held()).transpose()?;
        cut.reserve_after_staging(change,web.as_ref(),until,&alive)?
    }else{cut.confirmed_pending(change,until)?};
    pending.require()?;alive()?;
    channel.send(Kind::Reserved,&digest,&body(2,&target.record))?;
    let placed=Placed::receive(&channel,pending.change(),until)?;
    let active=pending.finish(||{alive()?;placed.require()})?;
    if active!=target{return Err(bad("wrong independently read back ACTIVE result"));}
    channel.send(Kind::Complete,&digest,&body(3,&target.record))
}
pub(crate) fn vault_worker()->io::Result<()>{
    let [fd,meta]=account_sql_descriptors()?;
    approval::only_inherited_descriptors(&[3,4])?;
    let parent=getppid().ok_or_else(||bad("the original Rust secret owner is absent"))?;
    exact_parent(&fd,parent)?;
    let (order,original)=SecretOrder::read(&meta)?;drop(meta);
    let (actor,uid,gid)=subject(&order.manifest,parent)?;
    if order.manifest.order.actor!=actor{return Err(bad("wrong original secret writer principal"));}
    let f=order.manifest.fresh.ok_or_else(||bad("no exact fresh credential program scope"))?;
    let policy=Cryptography::fresh(f.crypto)?;
    let lib=policy.before_dropping()?;
    let tty=SelectedInput::before_drop()?;
    let (channel,until)=isolated(fd,parent,uid,gid,original)?;
    let alive=||live_parent(parent,until);
    tty.lock_secret_memory()?;alive()?;
    let crypto=policy.for_low_writer(&lib)?;
    let target=order.selected;
    let (ready,[mut stage])=channel.wait_checked::<1>(Kind::RustStage,&target.action,until,&alive)?;
    if ready!=body(1,&target.target.record){return Err(bad("wrong original writable stage for the new Rust secret"));}
    alive()?;
    tty.rust_vault_only(crypto,&target.target.record_prefix()?,&mut stage)?;
    drop(stage);alive()?;
    channel.send(Kind::RustVaultSealed,&target.action,&body(1,&target.target.record))
}

pub(in crate::sqlite_install::pair) struct RustEnrollmentApproval<'s>{
    site:&'s RustSite,terminal:&'s PublicConfirmation,original:&'s Observed,until:Instant,
    fresh:Option<&'s dyn Fn()->io::Result<()>>,
}
impl<'s> RustEnrollmentApproval<'s> {
    pub(in crate::sqlite_install::pair) fn site(&self)->&'s RustSite{self.site}
    pub(in crate::sqlite_install::pair) fn original(&self)->&'s Observed{self.original}
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        if Instant::now()>=self.until{return Err(bad("the original Rust maintenance approval expired"));}
        if let Some(check)=self.fresh{check()?;}
        self.terminal.check()?;self.site.require()
    }
    pub(in crate::sqlite_install::pair) fn confirm_unregistered_stage(&self,review:String)->io::Result<()>{
        if self.original.state!=1||self.site.fresh_payload().is_some(){return Err(bad("no original approval to discard a stage"));}
        self.require()?;self.terminal.confirm_rust_stage_discard(review)?;self.require()
    }
    fn while_quiet(&self,quiet:&HeldQuiet<'_>)->io::Result<()>{
        if Instant::now()>=self.until{return Err(bad("the original Rust maintenance approval expired"));}
        if let Some(check)=self.fresh{check()?;}
        quiet.require()?;self.site.require()
    }
}
/// Only the confirmed original SQL worker may make the subsequent stage
/// publishable. An observed row, filename or byte prefix alone cannot do it.
pub(in crate::sqlite_install::pair) struct DurableRustPending<'a,'s>{
    approved:&'a RustEnrollmentApproval<'s>,sql:&'a Worker,
}
impl<'s> DurableRustPending<'_,'s> {
    pub(in crate::sqlite_install::pair) fn approved(&self)->&RustEnrollmentApproval<'s>{self.approved}
    pub(in crate::sqlite_install::pair) fn require(&self,site:&RustSite,meta:&ActiveRustMeta)->io::Result<()>{
        if !std::ptr::eq(self.approved.site,site)||&self.approved.original.target!=meta{return Err(bad("not the original durable Rust operation"));}
        self.approved.require()?;self.sql.live()
    }
}

fn launch(helper:&RuntimeFile,site:&RustSite,tty:&PublicConfirmation,scope:[u8;32],meta:File,mode:OfflineSql)
    ->io::Result<(Worker,Channel)>{
    let (ours,child)=transport::pair()?;
    let worker=Worker::spawn(&helper.file,&child,&meta,mode)?;
    drop(child);drop(meta);
    let (_,uid,gid)=site.subject();
    let actor=UCred{pid:worker.pid,uid:Uid::from_raw(uid),gid:Gid::from_raw(gid)};
    let until=Instant::now()+Duration::from_secs(15);
    loop{
        worker.live()?;
        if Instant::now()>=until{return Err(bad("the fixed original Rust credential worker did not become ready"));}
        if give_live_approval(&ours,actor,&scope,||{site.require()?;tty.check()?;helper.require_running_helper()})?{break;}
        thread::sleep(Duration::from_millis(20));
    }
    Ok((worker,Channel::from_owned_child_session(ours,actor,scope)?))
}
fn hex(b:&[u8])->String{b.iter().map(|v|format!("{v:02x}")).collect()}
fn preview(m:&Manifest,v:&Observed)->String{
    let site=m.plan.origin;let target=&v.target;
    let previous=v.previous.as_ref().map(|p|format!("原凭据 {} / 第 {} 代 / 正本 {}。",hex(&p.op),p.generation,hex(&p.record)))
        .unwrap_or_else(||"全新 Web 与 SMB 两种账户；不使用任何已有账户或外部账户库。".to_owned());
    let web=match (&m.order.role,v.web_account){
        (Desired::Setup{web,..},Some(id))=>format!("首设 Web 用户 {} / 账号 {}。\n",std::str::from_utf8(web).expect("checked login"),hex(&id)),
        _=>String::new(),
    };
    let action=if v.state==1{"准备新正本；遗留未登记暂存不会自动删除。"}
        else{"只恢复原待办；绝不重读密码或产生另一正本。"};
    format!("Rust 原安装 {:?} / {}，不启动 Native 或 Rust 服务。\n\
        原池 {} / INIT {} / SETUP {}；原站点 {} / Bind {}；描述 {}。\n\
        原事 {} / 批准 {} / 请求 {}。{}{}\n\
        {}唯一 SMB 用户 {} / 禁止匿名；账号 {} / 第 {} 代 / 新正本 {}。\n",
        m.system,m.plan.host(),hex(&site.pool),hex(&site.init),hex(&site.setup),hex(&site.instance),hex(&site.bind),
        hex(&m.plan.sha()),hex(&m.order.op),hex(&m.order.approval),hex(&v.action),previous,action,
        web,std::str::from_utf8(&target.login).expect("checked login"),hex(&target.account),target.generation,hex(&target.record))
}
fn expected(m:&Manifest,v:&Observed)->io::Result<()>{
    if v.target.op!=m.order.op{return Err(bad("different original action in Rust account receipt"));}
    let yes=match(&m.order.role,&v.previous){
        (Desired::Setup{smb,..},None)=>smb==&v.target.login&&v.target.generation==1,
        (Desired::Reset{previous,generation},Some(p))=>(*previous,*generation)==(p.op,p.generation),
        _=>false,
    };
    if !yes{return Err(bad("different original identity in Rust account receipt"));}
    Ok(())
}

fn transact_inner(site:&RustSite,m:Manifest,tty:&PublicConfirmation,fresh:Option<&dyn Fn()->io::Result<()>>)
    ->io::Result<Option<Observed>>{
    if let Some(check)=fresh{check()?;}
    site.require()?;tty.check()?;prerequisite()?;
    let (pin,lease)=site.original_system()?;let original=pin.entity();
    if &m.plan!=site.plan()||m.system.as_path()!=pin.path()||m.order.actor!=site.subject().0||
        m.entity!=[original.dev,original.ino,pin.require_same_named_mount()?.id]{
        return Err(bad("not this exact original offline Rust site"));
    }
    let helper=match (site.fresh_payload(),m.fresh){
        (Some(origin),Some(new)) if origin.sha()?==new.site&&origin.subject==new.subject=>
            crate::sqlite_install::pair::smb_policy::release(HELPER,0o755,&new.crypto.helper)?,
        _=>return Err(bad("not the exact new ZXS site authority")),
    };
    helper.require_running_helper()?;
    let scope=m.scope()?;
    let (mut sql,ch)=launch(&helper,site,tty,scope,sealed(&m.encode()?)?,OfflineSql::RustAccount)?;
    ch.send_original(Kind::Lease,&scope,&[0;65],lease)?;
    let initial=Instant::now()+Duration::from_secs(20);
    let (packet,[observed])=ch.wait_checked::<1>(Kind::RustInspect,&scope,initial,||{site.require()?;tty.check()})?;
    if packet==[0;65]{
        if m.fresh.is_some(){return Err(bad("this fresh issuer may not fall back to receipt lookup"));}
        if seal_bytes(&observed,1)?!=[0]{return Err(bad("uncertain original Rust operation absence"));}
        sql.finish(&ch,initial)?;site.require()?;return Ok(None);
    }
    let mut chosen=Observed::read(&observed,site.plan())?;
    drop(observed);
    if packet!=body(chosen.state,&chosen.target.record){return Err(bad("different original sealed Rust receipt"));}
    expected(&m,&chosen)?;
    if m.fresh.is_some()&&(chosen.state!=1||chosen.previous.is_some()){
        return Err(bad("the fresh issuer must not take over or continue an earlier request"));
    }
    if !matches!((m.intent,chosen.state),(Intent::Fresh,1)|(Intent::Resume,2)){
        sql.finish(&ch,initial)?;site.require()?;return Ok(Some(chosen));
    }
    let reset=chosen.previous.is_some();
    tty.confirm_rust_credential(reset,chosen.state==2,preview(&m,&chosen))?;
    let until=Instant::now()+Duration::from_secs(10*60);
    let grant=RustEnrollmentApproval{site,terminal:tty,original:&chosen,until,fresh};
    grant.require()?;
    let digest=chosen.action;let record=chosen.target.record;
    let frozen=if chosen.state==1{
        sql.live()?;
        if !reset{
            ch.send(Kind::Warm,&digest,&body(1,&record))?;
            let (p,[])=ch.wait_checked::<0>(Kind::Prepared,&digest,until,||grant.require())?;
            if p!=body(1,&record){return Err(bad("the actual separate Web workspace is not prepared"));}
        }
        let (allocated,writable)=Allocated::reserve(&grant,||sql.live())?;
        let input=SecretOrder::encode(&m,&chosen)?;
        let (mut writer,secret)=launch(&helper,site,tty,secret_scope(&input),sealed(&input)?,OfflineSql::RustVaultWriter)?;
        // Root owns terminal restoration even if a low worker must be
        // force-collected. These readers never compete for the same tty.
        let input=SelectedInput::before_drop()?;let stdin=io::stdin();
        let quiet=input.hold_for_birth(&stdin)?;
        let attempt=(||->io::Result<_>{
            grant.while_quiet(&quiet)?;
            secret.send_original(Kind::RustStage,&digest,&body(1,&record),&writable)?;
            drop(writable);
            let (written,[])=secret.wait_checked::<0>(Kind::RustVaultSealed,&digest,until,||{
                grant.while_quiet(&quiet)?;sql.live()
            })?;
            if written!=body(1,&record){return Err(bad("the original secret writer did not seal this exact stage"));}
            writer.finish(&secret,until)?;
            let frozen=allocated.reopen_after_reap()?;
            grant.while_quiet(&quiet)?;sql.live()?;
            ch.send(Kind::RustStage,&digest,&body(1,&record))?;
            let (p,[])=ch.wait_checked::<0>(Kind::Reserved,&digest,until,||grant.while_quiet(&quiet))?;
            if p!=body(2,&record){return Err(bad("the original SQL worker did not re-open this PENDING"));}
            Ok(frozen)
        })();
        if attempt.is_err(){writer.cancel();sql.cancel();}
        quiet.finish()?;
        Some(attempt?)
    }else{
        grant.require()?;sql.live()?;
        ch.send(Kind::RustStage,&digest,&body(2,&record))?;
        let (p,[])=ch.wait_checked::<0>(Kind::Reserved,&digest,until,||grant.require())?;
        if p!=body(2,&record){return Err(bad("the original SQL worker did not re-open this PENDING"));}
        None
    };
    grant.require()?;sql.live()?;
    let pending=DurableRustPending{approved:&grant,sql:&sql};
    let placed=match frozen{Some(stage)=>stage.replace_in_pending(&pending)?,None=>finish_seen_pending(&pending)?};
    let (wire,original)=placed.wire()?;
    ch.send_original(Kind::RustVaultPlaced,&digest,&wire,original)?;
    let (done,[])=ch.wait_checked::<0>(Kind::Complete,&digest,until,||{grant.require()?;placed.require()})?;
    if done!=body(3,&record){return Err(bad("uncertain Rust credential placement; query exactly the original operation"));}
    sql.finish(&ch,until)?;
    placed.require()?;grant.require()?;drop(grant);
    chosen.state=3;
    Ok(Some(chosen))
}


/// A successful return from the actual original low worker (clean exit and
/// EOF), the rather specific durable SQL readback and same original vault.
/// Raw Observed data alone is not a deployment publication capability.
#[must_use]
pub(in crate::sqlite_install::pair) struct FreshActive<'s>{
    site:&'s RustSite,original:Observed,
}
impl FreshActive<'_>{
    pub(in crate::sqlite_install::pair) fn at(&self,site:&RustSite)->io::Result<&ActiveRustMeta>{
        if !std::ptr::eq(self.site,site){return Err(bad("the fresh ACTIVE proof belongs to a different site"));}
        verify_fresh_vault(site,&self.original.target)?;
        Ok(&self.original.target)
    }
}
pub(in crate::sqlite_install::pair) fn transact_fresh<'s>(site:&'s RustSite,m:Manifest,
    tty:&PublicConfirmation,live:&dyn Fn()->io::Result<()>)->io::Result<FreshActive<'s>>{
    if site.fresh_payload().is_none()||m.fresh.is_none(){return Err(bad("fresh original ZXS request required"));}
    let Some(original)=transact_inner(site,m,tty,Some(live))? else{return Err(bad("no original fresh ZXS ACTIVE proof"));};
    if original.state!=3||original.target.generation!=1||original.target.op!=original.target.setup||
        original.previous.is_some()||original.web_account.is_none(){return Err(bad("no fresh generation-one completion"));}
    let good=FreshActive{site,original};
    good.at(site)?;Ok(good)
}

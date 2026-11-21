//! Fixed two-role control for the one Rust credential. The low SQL worker
//! has no readable vault descriptor. Only the separately reaped low
//! writer ever writes the prepared 240-byte record.
use super::{bad,host::{Worker,prerequisite},transport::{self,Channel,Kind,Payload},worker::original_parent,
    web_admin_reset::{Objects,object_body,read_object_body}};
use crate::{maintenance::HostMounts,pinned::{Kind as PathKind,Pinned,helper_elf::ForegroundElf},sqlite_install::{approval,
    originals::SelectedSystem,pair::{
        domain::{Allocated,CredentialSite,CurrentVault,RustSite,VaultStamp,checked_vault_stamp,deployed_worker_payload,
            give_live_approval,request_live_approval,sealed,verify_fresh_worker,verify_fresh_vault},
        fresh_issuer::{InitialDatabase,SitePayload},
        native_sql::{Original,rust_vault::{ActiveRustMeta,Desired,RustAccountCut,RustChange,reset_digest}},
        rust_enroll::{crypto::Cryptography,manifest::{Authority,Intent,Manifest,seal_bytes},receipt::Observed,secret_order::{SecretOrder,scope as secret_scope}},
        terminal::{HeldQuiet,PublicConfirmation,SelectedInput},
    }}};
use helper_linux_evidence::diagnostic_process::{OfflineSql,account_sql_descriptors};
use rustix::{fs::{OFlags,fcntl_getfl},net::UCred,process::{DumpableBehavior,Gid,Pid,Signal,Uid,
    dumpable_behavior,getppid,parent_process_death_signal,set_dumpable_behavior,set_parent_process_death_signal}};
use std::{fs::File,io,os::unix::fs::MetadataExt,sync::Arc,thread,time::{Duration,Instant}};

const LIMIT:Duration=Duration::from_secs(12*60);
fn exact_parent(fd:&File,owner:Pid)->io::Result<()>{
    original_parent(fd,owner)?;
    ForegroundElf::host()?.parent_is_same_image(owner)
}
type Subject=([u8;16],u32,u32);
fn subject(m:&Manifest,parent:Pid)->io::Result<(Subject,Option<SitePayload>)>{
    let image=ForegroundElf::host()?;
    image.require_independent_digest(&m.authority.helper())?;
    image.parent_is_same_image(parent)?;
    let bound=match m.authority{
        Authority::Fresh(scope)=>{
            verify_fresh_worker(&m.system,&m.plan,&scope)?;
            (scope.subject.tuple(),None)
        },
        Authority::Deployed(scope)=>{
            let actual=deployed_worker_payload(&m.plan,scope.site)?;
            if actual.subject!=scope.subject||actual.system!=m.system{
                return Err(bad("different original protected deployed subject or SYSTEM"));
            }
            (scope.subject.tuple(),Some(actual))
        },
    };
    let ((actor,_,_),_)=&bound;
    if m.order.actor!=*actor{return Err(bad("wrong original Rust maintenance principal"));}
    image.require()?;Ok(bound)
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
    let ((_,uid,gid),deployed)=subject(&m,parent)?;
    let scope=m.scope()?;
    let input=(m.intent==Intent::Fresh&&matches!(&m.order.role,Desired::Setup{..})).then(SelectedInput::before_drop);
    let (channel,until)=isolated(fd,parent,uid,gid,scope)?;
    let (zero,[lease])=channel.wait_checked::<1>(Kind::Lease,&scope,until,||live_parent(parent,until))?;
    if zero!=[0;65]{return Err(bad("wrong original SYSTEM lease"));}
    let local=if deployed.is_some(){Some(Arc::new(HostMounts::discovery_view(false)?))}else{None};
    let pinned=if let Some(view)=&local{
        Pinned::require_local_root(view)?;
        Pinned::local_storage(&m.system,view)?
    }else{Pinned::already_mounted(&m.system,PathKind::StorageRoot)?};
    let e=pinned.entity();
    if [e.dev,e.ino,pinned.require_same_named_mount()?.id]!=m.entity{return Err(bad("different original SYSTEM"));}
    let locked=SelectedSystem::from_origin(pinned,host,uid,gid)?.from_lent_ex(lease)?;
    let cut=match (m.authority,deployed.as_ref()){
        (Authority::Fresh(f),None)=>RustAccountCut::open_new_first(&locked,&m.plan,m.order,f.init_sha,f.database)?,
        (Authority::Deployed(_),Some(actual))=>RustAccountCut::open_deployed_reset(&locked,&m.plan,m.order,actual)?,
        _=>return Err(bad("no exact original Rust scope")),
    };
    let alive=||{live_parent(parent,until)?;locked.require()};
    let result=match (m.intent,cut.observe()?){
        (Intent::Fresh,Original::Absent)=>Some((1,cut.propose_new()?)),
        (Intent::Lookup,Original::Absent)=>None,
        (Intent::Lookup,Original::Pending(v))=>Some((2,v)),
        (Intent::Lookup,Original::Committed(v))=>Some((3,v)),
        _=>return Err(bad("new requires a truly absent original operation; never resumes it")),
    };
    let original=if deployed.is_some(){
        let (db,stamp,marker)=cut.objects()?;
        let observed_marker=marker.require(locked.source().dir())?;
        alive()?;
        channel.send_original(Kind::DeployedDatabase,&scope,&object_body(stamp),&db)?;
        channel.send_original(Kind::DeployedMarker,&scope,&object_body(observed_marker),marker.pin())?;
        Some((db,marker,observed_marker))
    }else{None};
    let Some((tag,change))=result else{
        alive()?;
        channel.send_original(Kind::RustInspect,&scope,&[0;65],&sealed(&[0])?)?;
        return Ok(());
    };
    let target=change.target();let digest=change.digest();
    alive()?;
    channel.send_original(Kind::RustInspect,&scope,&body(tag,&target.record),&observed(tag,&change)?)?;
    if m.intent!=Intent::Fresh{return Ok(());}
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
    let web=prepared.map(|p|p.read_held()).transpose()?;
    let pending=cut.reserve_after_staging(change,web.as_ref(),until,&alive)?;
    pending.require()?;alive()?;
    channel.send(Kind::Reserved,&digest,&body(2,&target.record))?;
    let placed=Placed::receive(&channel,pending.change(),until)?;
    let active=pending.finish(||{alive()?;placed.require()})?;
    if active!=target{return Err(bad("wrong independently read back ACTIVE result"));}
    if let Some((db,marker,expected_mark))=original{
        let (again,stamp,current_mark)=cut.objects()?;
        if InitialDatabase::scope(&again,uid)?!=InitialDatabase::scope(&db,uid)?||
            marker.require(locked.source().dir())?!=expected_mark||
            current_mark.require(locked.source().dir())?!=expected_mark{return Err(bad("original reset SYSTEM evidence changed"));}
        local.as_ref().ok_or_else(||bad("missing passive SYSTEM selection"))?.require_unchanged()?;
        alive()?;placed.require()?;
        channel.send(Kind::RustResetDurable,&digest,&object_body(stamp))?;
    }
    drop(cut);alive()?;placed.require()?;
    channel.send(Kind::Complete,&digest,&body(3,&target.record))
}

pub(crate) fn vault_worker()->io::Result<()>{
    let [fd,meta]=account_sql_descriptors()?;
    approval::only_inherited_descriptors(&[3,4])?;
    let parent=getppid().ok_or_else(||bad("the original Rust secret owner is absent"))?;
    exact_parent(&fd,parent)?;
    let (order,original)=SecretOrder::read(&meta)?;drop(meta);
    let ((_,uid,gid),deployed)=subject(&order.manifest,parent)?;
    let pins=order.manifest.authority.crypto().ok_or_else(||bad("no exact original credential provider scope"))?;
    let policy=if deployed.is_some(){Cryptography::for_deployed(pins)?}else{Cryptography::fresh(pins)?};
    let lib=policy.before_dropping()?;
    let tty=SelectedInput::before_drop()?;
    let (channel,until)=isolated(fd,parent,uid,gid,original)?;
    let alive=||live_parent(parent,until);
    tty.lock_secret_memory()?;alive()?;
    let crypto=policy.for_low_writer(&lib)?;
    let target=order.selected;
    if deployed.is_some(){
        channel.send(Kind::RustResetWriterReady,&target.action,&body(1,&target.target.record))?;
    }
    let (ready,[mut stage])=channel.wait_checked::<1>(Kind::RustStage,&target.action,until,&alive)?;
    if ready!=body(1,&target.target.record){return Err(bad("wrong original writable stage for the new Rust secret"));}
    alive()?;
    tty.rust_vault_only(crypto,&target.target.record_prefix()?,&mut stage)?;
    drop(stage);alive()?;
    channel.send(Kind::RustVaultSealed,&target.action,&body(1,&target.target.record))
}

pub(in crate::sqlite_install::pair) struct RustEnrollmentApproval<'s>{
    site:CredentialSite<'s>,terminal:&'s PublicConfirmation,original:&'s Observed,until:Instant,
    fresh:Option<&'s dyn Fn()->io::Result<()>>,
}
impl<'s> RustEnrollmentApproval<'s> {
    pub(in crate::sqlite_install::pair) fn site(&self)->CredentialSite<'s>{self.site}
    pub(in crate::sqlite_install::pair) fn original(&self)->&'s Observed{self.original}
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        if Instant::now()>=self.until{return Err(bad("the original Rust maintenance approval expired"));}
        if let Some(check)=self.fresh{check()?;}
        self.terminal.check()?;self.site.require()
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
    pub(in crate::sqlite_install::pair) fn require(&self,site:CredentialSite<'s>,meta:&ActiveRustMeta)->io::Result<()>{
        if !self.approved.site.same(site)||&self.approved.original.target!=meta{return Err(bad("not the original durable Rust operation"));}
        self.approved.require()?;self.sql.live()
    }
}

fn launch(helper:&ForegroundElf,site:CredentialSite<'_>,tty:&PublicConfirmation,scope:[u8;32],meta:File,mode:OfflineSql)
    ->io::Result<(Worker,Channel)>{
    let (ours,child)=transport::pair()?;
    let worker=Worker::spawn(helper.descriptor(),&child,&meta,mode)?;
    drop(child);drop(meta);
    let (_,uid,gid)=site.subject();
    let actor=UCred{pid:worker.pid,uid:Uid::from_raw(uid),gid:Gid::from_raw(gid)};
    let until=Instant::now()+Duration::from_secs(15);
    loop{
        worker.live()?;
        if Instant::now()>=until{return Err(bad("the fixed original Rust credential worker did not become ready"));}
        if give_live_approval(&ours,actor,&scope,||{site.require()?;tty.check()?;helper.require()})?{break;}
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
    let action=if v.previous.is_some(){"只重置已离线的原 SMB 账户，不更改 Web、卷或部署；未知暂存不采用、不删除。"}
        else{"准备新正本；遗留未登记暂存不会自动删除。"};
    format!("Rust 原安装 {:?} / {}，不启动任何服务。\n\
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
        (Desired::Reset{..},Some(p))=>reset_digest(&m.plan,&m.order,p,&v.target)?==v.action,
        _=>false,
    };
    if !yes{return Err(bad("different original identity in Rust account receipt"));}
    Ok(())
}

fn transact_inner(site:CredentialSite<'_>,m:Manifest,tty:&PublicConfirmation,fresh:Option<&dyn Fn()->io::Result<()>>)
    ->io::Result<Option<Observed>>{
    if let Some(check)=fresh{check()?;}
    site.require()?;tty.check()?;prerequisite()?;
    let (pin,lease)=site.original_system()?;let original=pin.entity();
    if &m.plan!=site.plan()||m.system.as_path()!=pin.path()||m.order.actor!=site.subject().0||
        m.entity!=[original.dev,original.ino,pin.require_same_named_mount()?.id]{
        return Err(bad("not this exact original offline Rust site"));
    }
    let deployed=match (site,m.authority){
        (CredentialSite::Fresh(f),Authority::Fresh(new))
            if f.fresh_payload().is_some_and(|p|p.subject==new.subject)&&fresh.is_some()=>{
                if f.fresh_payload().ok_or_else(||bad("missing original fresh authority"))?.sha()?!=new.site{
                    return Err(bad("different original fresh installation"));
                }
                false
            },
        (CredentialSite::Deployed(d),Authority::Deployed(current))
            if d.site_sha()==current.site&&d.payload().subject==current.subject&&fresh.is_none()=>true,
        _=>return Err(bad("not the exact original deployed or fresh ZXS authority")),
    };
    let helper=ForegroundElf::host()?;
    helper.require_independent_digest(&m.authority.helper())?;
    let scope=m.scope()?;
    let check=||{site.require()?;helper.require()?;tty.check()};
    let (mut sql,ch)=launch(&helper,site,tty,scope,sealed(&m.encode()?)?,OfflineSql::RustAccount)?;
    ch.send_original(Kind::Lease,&scope,&[0;65],lease)?;
    let initial=Instant::now()+Duration::from_secs(20);
    let (_,uid,gid)=site.subject();
    let objects=if deployed{Some(Objects::received(&ch,&scope,initial,lease,uid,&check)?)}else{None};
    let (packet,[observed])=ch.wait_checked::<1>(Kind::RustInspect,&scope,initial,&check)?;
    let evidence=observed.metadata()?;
    if (evidence.uid(),evidence.gid())!=(uid,gid){return Err(bad("observation not from the exact lowered subject"));}
    let original_cut=||->io::Result<()>{
        check()?;
        if let Some(o)=&objects{
            pin.local_scope().ok_or_else(||bad("missing passive original SYSTEM selection"))?.require_unchanged()?;
            o.require(lease,&o.initial)?;
        }
        Ok(())
    };
    if packet==[0;65]{
        if !deployed||m.intent!=Intent::Lookup||seal_bytes(&observed,1)?!=[0]{
            return Err(bad("uncertain original Rust operation absence"));
        }
        drop(observed);sql.finish(&ch,initial)?;original_cut()?;return Ok(None);
    }
    let mut chosen=Observed::read(&observed,site.plan())?;
    drop(observed);
    if packet!=body(chosen.state,&chosen.target.record){return Err(bad("different original sealed Rust receipt"));}
    expected(&m,&chosen)?;
    if m.intent==Intent::Lookup{
        if !deployed||!matches!(chosen.state,2|3)||chosen.previous.is_none(){return Err(bad("not a historical reset observation"));}
        sql.finish(&ch,initial)?;original_cut()?;return Ok(Some(chosen));
    }
    if chosen.state!=1||deployed!=chosen.previous.is_some(){return Err(bad("new requires a truly new original Rust operation"));}
    let current=chosen.previous.as_ref().map(|p|CurrentVault::select(site,p)).transpose()?;
    {
        let untouched=||->io::Result<()>{
            original_cut()?;
            if let Some(v)=&current{v.require()?;}
            sql.live()
        };
        untouched()?;
        tty.confirm_rust_credential(deployed,false,preview(&m,&chosen))?;
        untouched()?;
    }
    let until=Instant::now()+Duration::from_secs(if deployed{9*60}else{10*60});
    let grant=RustEnrollmentApproval{site,terminal:tty,original:&chosen,until,fresh};
    let digest=chosen.action;let record=chosen.target.record;
    let act=(||->io::Result<()>{
        grant.require()?;sql.live()?;
        if !deployed{
            ch.send(Kind::Warm,&digest,&body(1,&record))?;
            let (p,[])=ch.wait_checked::<0>(Kind::Prepared,&digest,until,||grant.require())?;
            if p!=body(1,&record){return Err(bad("the actual separate Web workspace is not prepared"));}
        }
        let first=if deployed{None}else{Some(Allocated::reserve(&grant,None,||sql.live())?)};
        let order=SecretOrder::encode(&m,&chosen)?;
        // Same known worker only after actual prior vault/SQL and public
        // approval. It must attest provider and memory protection before
        // a deployed reset creates even a blank stage.
        let (mut writer,secret)=launch(&helper,site,tty,secret_scope(&order),sealed(&order)?,OfflineSql::RustVaultWriter)?;
        if deployed{
            let (ready,[])=secret.wait_checked::<0>(Kind::RustResetWriterReady,&digest,until,||{
                grant.require()?;writer.live()?;sql.live()
            })?;
            if ready!=body(1,&record){return Err(bad("not the actual protected original SMB input"));}
        }
        let (allocated,writable)=match first{
            Some(first)=>first,
            None=>{
                original_cut()?;
                Allocated::reserve(&grant,current,||{sql.live()?;writer.live()})?
            }
        };
        // No process-wide cancellation handler or other PromptSignals
        // scope is active here. The parent owns terminal restoration.
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
            // From here the short original SQL reserve can make the old
            // credential unavailable. The retained DB pin gets a new time
            // stamp at final durable readback, not a newly selected inode.
            if let Some(o)=&objects{o.require(lease,&o.initial)?;}
            frozen.require_ready()?;grant.while_quiet(&quiet)?;sql.live()?;
            ch.send(Kind::RustStage,&digest,&body(1,&record))?;
            let (p,[])=ch.wait_checked::<0>(Kind::Reserved,&digest,until,||grant.while_quiet(&quiet))?;
            if p!=body(2,&record){return Err(bad("the original SQL worker did not re-open this PENDING"));}
            Ok(frozen)
        })();
        if attempt.is_err(){writer.cancel();sql.cancel();}
        quiet.finish()?;
        let frozen=attempt?;
        grant.require()?;sql.live()?;
        let pending=DurableRustPending{approved:&grant,sql:&sql};
        let placed=frozen.replace_in_pending(&pending)?;
        let (wire,original)=placed.wire()?;
        ch.send_original(Kind::RustVaultPlaced,&digest,&wire,original)?;
        let stamp=if deployed{
            let (d,[])=ch.wait_checked::<0>(Kind::RustResetDurable,&digest,until,||{grant.require()?;placed.require()})?;
            Some(read_object_body(d)?)
        }else{None};
        let (done,[])=ch.wait_checked::<0>(Kind::Complete,&digest,until,||{grant.require()?;placed.require()})?;
        if done!=body(3,&record){return Err(bad("wrong original Rust credential completion"));}
        sql.finish(&ch,until)?;
        placed.require()?;grant.require()?;helper.require()?;
        match (&objects,stamp){
            (Some(o),Some(now))=>{
                pin.local_scope().ok_or_else(||bad("missing original reset SYSTEM view"))?.require_unchanged()?;
                o.require(lease,&now)?;
            },
            (None,None)=>(),
            _=>return Err(bad("missing original reset SQLite and marker evidence")),
        }
        Ok(())
    })();
    if deployed{act.map_err(|e|reset_uncertain(&m,e))?;}else{act?;}
    chosen.state=3;
    Ok(Some(chosen))
}

pub(super) fn reset_uncertain(m:&Manifest,e:io::Error)->io::Error{
    io::Error::new(e.kind(),format!(
        "SMB 重置结果未确认；仅查询同一原操作 {} / 批准 {}，勿再发起、删除或清理。{e}",hex(&m.order.op),hex(&m.order.approval)))
}
pub(super) fn transact_deployed(site:&super::super::domain::DeployedSite,m:Manifest,tty:&PublicConfirmation)
    ->io::Result<Option<Observed>>{
    transact_inner(CredentialSite::Deployed(site),m,tty,None)
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
    if site.fresh_payload().is_none()||!matches!(m.authority,Authority::Fresh(_)){return Err(bad("fresh original ZXS request required"));}
    let Some(original)=transact_inner(CredentialSite::Fresh(site),m,tty,Some(live))? else{return Err(bad("no original fresh ZXS ACTIVE proof"));};
    if original.state!=3||original.target.generation!=1||original.target.op!=original.target.setup||
        original.previous.is_some()||original.web_account.is_none(){return Err(bad("no fresh generation-one completion"));}
    let good=FreshActive{site,original};
    good.at(site)?;Ok(good)
}

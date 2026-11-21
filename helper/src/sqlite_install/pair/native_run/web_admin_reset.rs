//! An exact original deployed v17 Web administrator operation. The worker
//! is same-ELF and private, with no account, password or media path from argv.
use super::{bad,host::{Worker,prerequisite},transport::{self,Channel,Kind},worker::original_parent};
use crate::{maintenance::HostMounts,pinned::{Pinned,helper_elf::ForegroundElf},smb_profile::rust_first::{MAX_WIRE,RustPlan},
    sqlite_install::{approval,markers::SystemObservation,originals::SelectedSystem,stage::random,pair::{
        command::{key,sha},domain::{DeployedSite,give_live_approval,request_live_approval,sealed,verify_deployed_worker},
        fresh_issuer::{InitialDatabase,MAX_SITE,SitePayload,compiled_v17},
        native_sql::web_reset::{Current,Order,Receipt,WebReset,Witness,fingerprint},rust_enroll::manifest::seal_bytes,
        terminal::{PublicConfirmation,SelectedInput},
    }}};
use helper_linux_evidence::diagnostic_process::{OfflineSql,account_sql_descriptors};
use rustix::{net::UCred,process::{DumpableBehavior,Gid,Pid,Signal,Uid,dumpable_behavior,getppid,
    parent_process_death_signal,set_dumpable_behavior,set_parent_process_death_signal}};
use sha2::{Digest,Sha256};
use std::{ffi::OsString,fs::File,io,os::unix::fs::MetadataExt,sync::Arc,time::{Duration,Instant}};

const TAG:&[u8]=b"ZXS-HELPER/WEB-ADMIN-RESET-1\0";
const CAP:usize=TAG.len()+1+32+32+32+24+4+MAX_SITE+MAX_WIRE;
const REPORT_MAX:usize=1+16+16+8+1+64+224+16+8+1+32;
fn wrong()->io::Error{bad("the original offline Web administrator operation was not confirmed")}
type Id=[u8;16];

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Action{New,Receipt}
impl Action{
    fn byte(self)->u8{match self{Self::New=>1,Self::Receipt=>2}}
    fn read(n:u8)->io::Result<Self>{match n{1=>Ok(Self::New),2=>Ok(Self::Receipt),_=>Err(wrong())}}
}
#[derive(Debug)]
pub(crate) struct Selection{action:Action,order:Order,helper:[u8;32]}
impl Selection{
    pub(crate) fn parse(mut it:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        let action=match it.next().as_deref().and_then(|s|s.to_str()){
            Some("new")=>Action::New,Some("receipt")=>Action::Receipt,
            _=>return Err("choose reset-web-admin new or receipt explicitly"),
        };
        let order=Order{op:key(&mut it,"--op")?,approval:key(&mut it,"--approval")?};
        let helper=sha(&mut it,"--helper-sha256")?;
        if it.next().is_some()||order.op==order.approval{return Err("expected only distinct operation and approval IDs and an independent Helper digest");}
        Ok(Self{action,order,helper})
    }
}
struct Question{action:Action,order:Order,helper:[u8;32],nonce:[u8;32],place:[u64;3],site:SitePayload,plan:RustPlan}
impl Question{
    fn encode(&self)->io::Result<Vec<u8>>{
        compiled_v17()?;self.order.check(&self.site)?;
        let site=self.site.encode()?;let plan=self.plan.wire();
        if self.place[1..].contains(&0)||self.helper==[0;32]||self.nonce==[0;32]||
            self.site.origin!=self.plan.origin||self.site.plan_sha!=self.plan.sha(){return Err(wrong());}
        let mut b=Vec::with_capacity(CAP);b.extend(TAG);b.push(self.action.byte());b.extend(self.helper);b.extend(self.nonce);
        b.extend(self.order.op);b.extend(self.order.approval);
        for n in self.place{b.extend(n.to_be_bytes());}
        for value in [&site[..],plan]{b.extend(u16::try_from(value.len()).map_err(|_|wrong())?.to_be_bytes());b.extend(value);}
        if b.len()>CAP{return Err(wrong());}Ok(b)
    }
    fn decode(raw:&[u8])->io::Result<Self>{
        let mut c=Read(raw.strip_prefix(TAG).filter(|_|raw.len()<=CAP).ok_or_else(wrong)?);
        let action=Action::read(c.one()?)?;let helper=c.a()?;let nonce=c.a()?;
        let order=Order{op:c.a()?,approval:c.a()?};
        let place=[u64::from_be_bytes(c.a()?),u64::from_be_bytes(c.a()?),u64::from_be_bytes(c.a()?)];
        let site=SitePayload::decode(c.counted(MAX_SITE)?)?;let plan=RustPlan::decode(c.counted(MAX_WIRE)?).map_err(bad)?;
        let q=Self{action,order,helper,nonce,place,site,plan};
        if !c.0.is_empty()||q.encode()?.as_slice()!=raw{return Err(wrong());}Ok(q)
    }
}
struct Read<'a>(&'a[u8]);
impl<'a> Read<'a>{
    fn take(&mut self,n:usize)->io::Result<&'a[u8]>{
        let (f,r)=self.0.split_at_checked(n).ok_or_else(wrong)?;self.0=r;Ok(f)
    }
    fn a<const N:usize>(&mut self)->io::Result<[u8;N]>{Ok(self.take(N)?.try_into().expect("fixed field"))}
    fn one(&mut self)->io::Result<u8>{let [n]=self.a()?;Ok(n)}
    fn counted(&mut self,max:usize)->io::Result<&'a[u8]>{
        let n=usize::from(u16::from_be_bytes(self.a()?));if !(1..=max).contains(&n){return Err(wrong());}self.take(n)
    }
}
fn digest(b:&[u8])->[u8;32]{Sha256::digest(b).into()}
#[derive(Debug,Eq,PartialEq)]
struct Observation{seen:Witness,receipt:Option<Receipt>}
impl Observation{
    fn encode(&self,q:&Question)->io::Result<Vec<u8>>{
        let w=&self.seen.web;w.check(&q.site)?;
        if q.action==Action::New&&self.receipt.is_some(){return Err(wrong());}
        let mut b=Vec::with_capacity(REPORT_MAX);b.push(q.action.byte());b.extend(w.account);b.extend(w.op);
        b.extend(w.generation.to_be_bytes());b.push(w.login.len() as u8);b.extend(&w.login);b.extend(self.seen.smb);
        if let Some(r)=self.receipt{
            if r.generation<2||r.generation>w.generation||(r.generation==w.generation)!=r.current||r.current!=(w.op==q.order.op)||
                r.digest!=fingerprint(&q.site,q.order,&Current{op:r.previous,generation:r.generation-1,..w.clone()})?{
                return Err(wrong());
            }
            b.extend(r.previous);b.extend(r.generation.to_be_bytes());b.push(u8::from(r.current));b.extend(r.digest);
        }
        Ok(b)
    }
    fn decode(b:&[u8],q:&Question)->io::Result<Self>{
        if b.len()>REPORT_MAX{return Err(wrong());}
        let mut c=Read(b);
        if c.one()?!=q.action.byte(){return Err(wrong());}
        let account=c.a()?;let op=c.a()?;let generation=i64::from_be_bytes(c.a()?);let n=usize::from(c.one()?);
        let web=Current{account,op,generation,login:c.take(n)?.to_vec()};
        let smb=c.a()?;
        let receipt=if c.0.is_empty(){None}else{Some(Receipt{previous:c.a()?,generation:i64::from_be_bytes(c.a()?),
            current:match c.one()?{0=>false,1=>true,_=>return Err(wrong())},digest:c.a()?})};
        let v=Self{seen:Witness{web,smb},receipt};
        if !c.0.is_empty()||v.encode(q)?.as_slice()!=b{return Err(wrong());}Ok(v)
    }
}
fn observation_body(q:&Question,body:&[u8])->io::Result<[u8;65]>{
    let mut r=[0;65];r[0]=q.action.byte();r[1..33].copy_from_slice(&q.site.sha()?);r[33..].copy_from_slice(&digest(body));Ok(r)
}
fn action_body(tag:u8,q:&Question,previous:&Current)->[u8;65]{
    let mut r=[0;65];r[0]=tag;r[1..17].copy_from_slice(&q.order.op);r[17..33].copy_from_slice(&previous.op);
    r[33..41].copy_from_slice(&previous.generation.to_be_bytes());r[41..57].copy_from_slice(&q.order.approval);r
}
pub(super) fn object_body(stamp:[u8;64])->[u8;65]{
    let mut frame=[0;65];frame[0]=1;frame[1..].copy_from_slice(&stamp);frame
}
pub(super) fn read_object_body(frame:[u8;65])->io::Result<[u8;64]>{
    if frame[0]!=1{return Err(wrong());}
    Ok(frame[1..].try_into().expect("fixed observation stamp"))
}
pub(super) struct Objects{db:InitialDatabase,marker:SystemObservation,pub(super) initial:[u8;64],original_mark:[u8;64]}
impl Objects{
    pub(super) fn received(channel:&Channel,scope:&[u8;32],until:Instant,system:&File,uid:u32,
        check:&impl Fn()->io::Result<()>)->io::Result<Self>{
        let (frame,[fd])=channel.wait_checked::<1>(Kind::DeployedDatabase,scope,until,check)?;
        let db=InitialDatabase::received(fd,uid)?;let initial=read_object_body(frame)?;
        let (frame,[fd])=channel.wait_checked::<1>(Kind::DeployedMarker,scope,until,check)?;
        let marker=SystemObservation::received(fd,system,uid)?;
        let result=Self{db,marker,initial,original_mark:read_object_body(frame)?};
        result.require(system,&initial)?;Ok(result)
    }
    pub(super) fn require(&self,system:&File,db:&[u8;64])->io::Result<()>{
        self.db.named(system)?;
        if self.db.observation()?!=*db||self.marker.require(system)?!=self.original_mark{return Err(wrong());}
        Ok(())
    }
}

fn check_parent(parent:Pid,until:Instant)->io::Result<()>{
    if getppid()!=Some(parent)||parent_process_death_signal()?!=Some(Signal::TERM)||Instant::now()>=until{
        return Err(wrong());
    }
    Ok(())
}

pub(crate) fn worker()->io::Result<()>{
    let [fd,meta]=account_sql_descriptors()?;
    approval::only_inherited_descriptors(&[3,4])?;
    let parent=getppid().ok_or_else(wrong)?;
    original_parent(&fd,parent)?;
    let host=approval::host_admin()?;
    let bytes=seal_bytes(&meta,CAP)?;drop(meta);
    let q=Question::decode(&bytes)?;let scope=digest(&bytes);
    let helper=ForegroundElf::host()?;
    helper.require_independent_digest(&q.helper)?;helper.parent_is_same_image(parent)?;
    verify_deployed_worker(&q.site,&q.plan)?;
    let selected=(q.action==Action::New).then(SelectedInput::before_drop).transpose()?;
    helper.require()?;drop(helper);
    let (_,uid,gid)=q.site.subject.tuple();
    approval::drop_to_storage(uid,gid)?;
    set_parent_process_death_signal(Some(Signal::TERM))?;
    original_parent(&fd,parent)?;
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    if dumpable_behavior()?!=DumpableBehavior::NotDumpable{return Err(wrong());}
    request_live_approval(&fd,parent,&scope,||original_parent(&fd,parent))?;
    let until=Instant::now()+Duration::from_secs(9*60);
    let channel=Channel::from_owned_child_session(fd,UCred{pid:parent,uid:Uid::ROOT,gid:Gid::ROOT},scope)?;
    let (body,[lease])=channel.wait_checked::<1>(Kind::Lease,&scope,until,||check_parent(parent,until))?;
    if body!=[0;65]{return Err(wrong());}
    let local=Arc::new(HostMounts::discovery_view(false)?);
    Pinned::require_local_root(&local)?;
    let named=Pinned::local_storage(&q.site.system,&local)?;
    let entity=named.entity();
    if [entity.dev,entity.ino,named.require_same_named_mount()?.id]!=q.place{return Err(wrong());}
    let locked=SelectedSystem::from_origin(named,host,uid,gid)?.from_lent_ex(lease)?;
    let cut=WebReset::open(&locked,&q.site,&q.plan,q.order)?;
    let alive=||{check_parent(parent,until)?;locked.require()};
    let (seen,receipt)=cut.observe(q.action==Action::New)?;
    let observed=Observation{seen,receipt};
    let bytes=observed.encode(&q)?;
    let (original_db,prior_stamp,original_marker)=cut.objects()?;
    let original_mark=original_marker.require(locked.source().dir())?;
    alive()?;
    channel.send_original(Kind::DeployedDatabase,&scope,&object_body(prior_stamp),&original_db)?;
    channel.send_original(Kind::DeployedMarker,&scope,&object_body(original_mark),original_marker.pin())?;
    channel.send_original(Kind::WebResetObservation,&scope,&observation_body(&q,&bytes)?,&sealed(&bytes)?)?;
    let Some(input)=selected else{return Ok(());}; // never acquire input authority on receipt
    let prior=&observed.seen.web;
    let expected=fingerprint(&q.site,q.order,prior)?;
    let (warm,[])=channel.wait_checked::<0>(Kind::Warm,&expected,until,&alive)?;
    if warm!=action_body(1,&q,prior)||cut.observe(true)?.0!=observed.seen{return Err(wrong());}
    let workspace=input.reserve_web(uid,gid)?;
    channel.send(Kind::Prepared,&expected,&action_body(2,&q,prior))?;
    let (act,[])=channel.wait_checked::<0>(Kind::WebResetAct,&expected,until,&alive)?;
    if act!=action_body(3,&q,prior){return Err(wrong());}
    let prepared=workspace.read_held()?;
    let committed=cut.apply(&observed.seen,prepared,until,&alive)?;
    if !committed.current||committed.previous!=prior.op||committed.generation!=prior.generation+1||committed.digest!=expected{return Err(wrong());}
    let (now,stamp,marker)=cut.objects()?;
    if InitialDatabase::scope(&now,uid)?!=InitialDatabase::scope(&original_db,uid)?
        ||marker.require(locked.source().dir())?!=original_mark{return Err(wrong());}
    local.require_unchanged()?;alive()?;
    channel.send(Kind::WebResetDurable,&expected,&object_body(stamp))?;
    // Only durable reopen, a normally closed worker and exact EOF let
    // the original owner report success. Parent holds both initial pins.
    drop(cut);alive()?;
    channel.send(Kind::Complete,&expected,&action_body(4,&q,prior))
}

fn hex(b:&[u8])->String{b.iter().map(|v|format!("{v:02x}")).collect()}
fn public(op:Id,r:Receipt)->String{
    format!("Web 管理员密码操作 {} 已确认（{}；原代次操作 {}，新代次 {}，原申请 SHA-256 {}）。SMB 凭据未修改；未启动或登录。",
        hex(&op),if r.current{"当前 Web 代次"}else{"只读历史回执，不是当前 Web 代次"},hex(&r.previous),r.generation,hex(&r.digest))
}
impl Selection{
    pub(crate) fn run(self)->io::Result<u8>{
        compiled_v17()?;
        let site=DeployedSite::select_for_web_reset()?; // pristine inherited 0–2 precedes retained terminal
        let tty=PublicConfirmation::select()?;
        prerequisite()?;
        let helper=ForegroundElf::host()?;
        helper.require_independent_digest(&self.helper)?;
        let (system,lease)=site.original();let e=system.entity();
        let q=Question{action:self.action,order:self.order,helper:self.helper,nonce:random()?,
            place:[e.dev,e.ino,system.require_same_named_mount()?.id],site:site.payload().clone(),plan:site.plan().clone()};
        let raw=q.encode()?;let scope=digest(&raw);
        let (ours,peer)=transport::pair()?;let order=sealed(&raw)?;
        let mut worker=Worker::spawn(helper.descriptor(),&peer,&order,OfflineSql::WebAdministratorReset)?;
        drop(peer);drop(order);
        let (_,uid,gid)=q.site.subject.tuple();
        let who=UCred{pid:worker.pid,uid:Uid::from_raw(uid),gid:Gid::from_raw(gid)};
        let initial=Instant::now()+Duration::from_secs(20);
        let check=||{site.require()?;helper.require()?;tty.check()};
        loop{
            worker.live()?;check()?;
            if Instant::now()>=initial{return Err(wrong());}
            if give_live_approval(&ours,who,&scope,&check)?{break;}
            std::thread::sleep(Duration::from_millis(20));
        }
        let channel=Channel::from_owned_child_session(ours,who,scope)?;
        channel.send_original(Kind::Lease,&scope,&[0;65],lease)?;
        let objects=Objects::received(&channel,&scope,initial,lease,uid,&check)?;
        let (reply,[fd])=channel.wait_checked::<1>(Kind::WebResetObservation,&scope,initial,&check)?;
        let m=fd.metadata()?;
        if (m.uid(),m.gid())!=(uid,gid){return Err(wrong());}
        let bytes=seal_bytes(&fd,REPORT_MAX)?;drop(fd);
        if reply!=observation_body(&q,&bytes)?{return Err(wrong());}
        let result=Observation::decode(&bytes,&q)?;
        let original_vault=site.observe_vault(&result.seen.smb)?;
        let unchanged=||{original_vault.require()?;helper.require()};
        unchanged()?;
        if self.action==Action::Receipt{
            worker.finish(&channel,initial)?;
            system.local_scope().ok_or_else(wrong)?.require_unchanged()?;
            unchanged()?;check()?;
            objects.require(lease,&objects.initial)?;
            return match result.receipt{
                Some(r)=>{tty.report(&format!("{}\n",public(q.order.op,r)))?;Ok(0)}
                None=>{tty.report("原操作未在本次已验证的原库中确认；没有数据库或凭据写入，没有补做或提示口令。\n")?;Ok(2)}
            };
        }
        if result.receipt.is_some(){return Err(wrong());}
        let prior=&result.seen.web;
        let expected=fingerprint(&q.site,q.order,prior)?;
        let o=q.site.origin;
        tty.confirm_web_administrator_reset(format!(
            "仅重置已离线的原 ZXS Web 管理员，不变更 SMB 凭据、卷或部署。\n原池 {} / INIT {} / 实例 {}，原 SYSTEM {}。\n原 Web 用户 {} / 账号 {}，从代次 {}（原操作 {}）递进至 {}。\n新操作 {}，批准参考 {}，原申请 SHA-256 {}，本次 Helper 独立摘要 {}。请留存操作和批准参考；口令仅在随后的同一终端输入两遍。\n",
            hex(&o.pool),hex(&o.init),hex(&o.instance),q.site.system.display(),std::str::from_utf8(&prior.login).map_err(|_|wrong())?,
            hex(&prior.account),prior.generation,hex(&prior.op),prior.generation+1,hex(&q.order.op),hex(&q.order.approval),hex(&expected),hex(&q.helper)))?;
        // Use defaults outside the promptly scoped confirm/quiet phases.
        // A process-wide watcher here would make HeldQuiet refuse before ACT.
        let until=Instant::now()+Duration::from_secs(6*60);
        let live=||{check()?;unchanged()?;objects.require(lease,&objects.initial)?;worker.live()};
        live()?;channel.send(Kind::Warm,&expected,&action_body(1,&q,prior))?;
        let (prepared,[])=channel.wait_checked::<0>(Kind::Prepared,&expected,until,&live)?;
        if prepared!=action_body(2,&q,prior){return Err(wrong());}
        let input=SelectedInput::before_drop()?;
        let stdin=io::stdin();
        let quiet=input.hold_for_birth(&stdin)?;
        let active=||{quiet.require()?;unchanged()};
        system.local_scope().ok_or_else(wrong)?.require_unchanged()?;
        active()?;worker.live()?;objects.require(lease,&objects.initial)?;
        let acted=(||->io::Result<[u8;64]>{
            channel.send(Kind::WebResetAct,&expected,&action_body(3,&q,prior))?;
            let (state,[])=channel.wait_checked::<0>(Kind::WebResetDurable,&expected,until,&active)?;
            let fresh_stamp=read_object_body(state)?;
            let (final_body,[])=channel.wait_checked::<0>(Kind::Complete,&expected,until,&active)?;
            if final_body!=action_body(4,&q,prior){return Err(wrong());}
            worker.finish(&channel,until)?;
            system.local_scope().ok_or_else(wrong)?.require_unchanged()?;
            unchanged()?;
            objects.require(lease,&fresh_stamp)?;Ok(fresh_stamp)
        })();
        if acted.is_err(){worker.cancel();}
        let restore=quiet.finish();
        let unknown=|e:io::Error|io::Error::new(e.kind(),format!(
            "Web 重置结果未确认；仅查询同一原操作 {} / 批准 {}，勿再发起或清理。{e}",hex(&q.order.op),hex(&q.order.approval)));
        restore.map_err(&unknown)?;let fresh_stamp=acted.map_err(&unknown)?;
        check().map_err(&unknown)?;unchanged().map_err(&unknown)?;
        objects.require(lease,&fresh_stamp).map_err(&unknown)?;
        let receipt=Receipt{previous:prior.op,generation:prior.generation+1,current:true,digest:expected};
        tty.report(&format!("{}\n",public(q.order.op,receipt))).map_err(unknown)?;
        Ok(0)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use std::path::Path;
    fn question()->Question{
        let text=include_str!("../../../../tests/smb-host/rust-install-v1.txt");
        let hex=text.lines().find_map(|s|s.strip_prefix("wire_hex=")).unwrap();
        let raw=hex.as_bytes().chunks_exact(2).map(|b|u8::from_str_radix(std::str::from_utf8(b).unwrap(),16).unwrap()).collect::<Vec<_>>();
        let plan=RustPlan::decode(&raw).unwrap();
        let site=SitePayload::new(&plan,crate::sqlite_install::pair::ExplicitSubject::checked([71;16],1001,1002).unwrap(),
            Path::new("/synthetic/S"),Path::new("/usr/bin/zxs"),[72;32]).unwrap();
        Question{action:Action::Receipt,order:Order{op:[73;16],approval:[74;16]},helper:[75;32],nonce:[76;32],place:[0,77,78],site,plan}
    }
    #[test]
    fn web_admin_reset_parser_has_no_password_path_identity_override_or_resume(){
        let parse=|a:&[&str]|Selection::parse(a.iter().map(|s|OsString::from(*s)));
        let op=hex(&[11;16]);let r=hex(&[12;16]);let d=hex(&[13;32]);
        for action in ["new","receipt"]{
            let fields=[action,"--op",&op,"--approval",&r,"--helper-sha256",&d];
            assert_eq!(parse(&fields).unwrap().action.byte(),if action=="new"{1}else{2});
            for extra in ["--password","--system","--login","--resume","--smb","--instance"]{
                let mut x=fields.to_vec();x.extend([extra,"never-used"]);assert!(parse(&x).is_err());
            }
        }
        assert!(parse(&["resume","--op",&op,"--approval",&r,"--helper-sha256",&d]).is_err());
    }
    #[test]
    fn web_admin_reset_exact_scope_cannot_turn_a_history_receipt_into_an_action(){
        let mut q=question();
        let encoded=q.encode().unwrap();let seen=Question::decode(&encoded).unwrap();
        assert_eq!((seen.action,seen.order,seen.place),(q.action,q.order,q.place));
        for n in [0,TAG.len(),encoded.len()-1]{assert!(Question::decode(&encoded[..n]).is_err());}
        let mut tail=encoded.clone();tail.push(0);assert!(Question::decode(&tail).is_err());
        let origin=Current{account:[79;16],op:q.site.origin.setup,login:b"fixture".to_vec(),generation:1};
        let report=Observation{seen:Witness{web:Current{op:q.order.op,generation:2,..origin.clone()},smb:[81;224]},
            receipt:Some(Receipt{previous:origin.op,generation:2,current:true,digest:fingerprint(&q.site,q.order,&origin).unwrap()})};
        let bytes=report.encode(&q).unwrap();assert_eq!(Observation::decode(&bytes,&q).unwrap(),report);
        let mut stale=bytes.clone();*stale.last_mut().unwrap()^=1;
        assert!(Observation::decode(&stale,&q).is_err());
        assert_ne!(observation_body(&q,&stale).unwrap(),observation_body(&q,&bytes).unwrap());
        q.action=Action::New;assert!(Observation::decode(&bytes,&q).is_err());
        assert_ne!(q.encode().unwrap(),encoded);
        assert_ne!(action_body(1,&q,&origin),action_body(3,&q,&origin));
    }
}

//! Permanently non-root SQLite branch of exactly one offline foreground
//! process. Root only verifies itself/parent and the sealed public envelope;
//! the original pool is opened under the configured storage UID and an
//! already-held original EX, never by trusting a parent-supplied SQLite path.
use super::{Intent,ResultRow,Synced,bad,manifest::Manifest,transport::{Channel,Kind,Payload}};
use crate::{fs_tools::RuntimeFile,pinned::{Kind as PathKind,Pinned},sqlite_install::{approval,
    originals::SelectedSystem,pair::{domain::request_live_approval,native_sql::{Ids,Original,
    transaction::{Order,Enrollment,Session}},terminal::SelectedInput},stage::random}};
use helper_linux_evidence::diagnostic_process::account_sql_descriptors;
use rustix::{net::{sockopt,AddressFamily,SocketType,UCred},process::{getppid,parent_process_death_signal,
    set_parent_process_death_signal,set_dumpable_behavior,dumpable_behavior,DumpableBehavior,Gid,Pid,Signal,Uid}};
use std::{fs::File,io,time::{Duration,Instant}};

const HELPER:&[&str]=&["usr","bin","helper"];
const MAX_WAIT:Duration=Duration::from_secs(12*60);

fn original_parent(fd:&File,expected:Pid)->io::Result<()>{
    let who=sockopt::socket_peercred(fd)?;
    if who.pid!=expected||!who.uid.is_root()||!who.gid.is_root()||getppid()!=Some(expected)
        ||parent_process_death_signal()?!=Some(Signal::TERM)||sockopt::socket_domain(fd)?!=AddressFamily::UNIX
        ||sockopt::socket_type(fd)?!=SocketType::SEQPACKET||!sockopt::socket_passcred(fd)?{
        return Err(bad("not the live original root account supervisor"));
    }Ok(())
}
pub(super) fn stage_body(tag:u8,row:Option<ResultRow>)->Payload{
    let mut b=[0;65];b[0]=tag;
    if let Some(row)=row{b[1..34].copy_from_slice(&row.wire());}
    b
}
pub(super) fn inspect(body:&Payload)->io::Result<(u8,Option<ResultRow>)>{
    if body[34..]!=[0;31]||body[0]>3{return Err(bad("wrong original SQL account report"));}
    Ok((body[0],if body[1..34]==[0;33]{None}else{
        Some(ResultRow::unwire(body[1..34].try_into().expect("fixed row"))?)
    }))
}
fn expected_new(order:&Order)->io::Result<ResultRow>{
    match order {
        Order::Setup(_)=>Ok(ResultRow::Setup(Ids::checked(random()?,random()?)?)),
        Order::Reset(r)=>Ok(ResultRow::Reset(r.generation())),
        Order::WebReset(_)=>Err(bad("Web reset is not a native worker action")),
    }
}
pub(crate) fn run()->io::Result<()>{
    // Require the fixed descriptors to really exist before counting: opening
    // procfs could otherwise itself temporarily occupy a missing reserved fd.
    let [fd,meta]=account_sql_descriptors()?;
    approval::only_inherited_descriptors(&[3,4])?;
    let parent=getppid().ok_or_else(||bad("no supervising parent"))?;
    original_parent(&fd,parent)?;
    let host=approval::host_admin()?;let (actor,uid,gid)=approval::profile()?;
    let release=RuntimeFile::open(HELPER,true,64<<20)?;
    release.require_running_helper()?;release.require_parent_helper(parent)?;drop(release);
    let m=Manifest::read(&meta)?;drop(meta);
    if m.actor!=actor{return Err(bad("the actual host maintenance principal changed"));}
    let input=matches!((&m.role,m.intent),(super::manifest::Role::Birth{..},Intent::New))
        .then(SelectedInput::before_drop);
    approval::drop_to_storage(uid,gid)?;
    // Linux clears the parent-death signal on the real credential transition.
    set_parent_process_death_signal(Some(Signal::TERM))?;
    original_parent(&fd,parent)?;
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    if dumpable_behavior()?!=DumpableBehavior::NotDumpable{return Err(bad("cannot isolate the SQLite credential process"));}
    let scope=m.scope()?;let order=m.order()?;let digest=order.sha();
    request_live_approval(&fd,parent,&scope,||original_parent(&fd,parent))?;
    let until=Instant::now()+MAX_WAIT;
    let channel=Channel::from_owned_child_session(fd,UCred{pid:parent,uid:Uid::ROOT,gid:Gid::ROOT},scope)?;
    let (zero,[leased])=channel.wait::<1>(Kind::Lease,&digest,until)?;
    if zero!=[0;65]{return Err(bad("invalid original SYSTEM lease handoff"));}
    let verified=Pinned::open(&m.system,PathKind::StorageRoot)?;
    let e=verified.entity();
    if [e.dev,e.ino,verified.mount()?.id]!=m.origin{return Err(bad("not the approved live SYSTEM original"));}
    let locked=SelectedSystem::from_origin(verified,host,uid,gid)?.from_lent_ex(leased)?;
    let session=Session::open(&locked,order)?;
    let original=session.original()?;
    let (tag,row)=match (m.intent,original){
        (_,Original::Committed(r))=>(3,Some(r)),
        (Intent::Lookup,Original::Absent)=>(0,None),
        (_,Original::Pending(r))=>(2,Some(r)),
        (Intent::New,Original::Absent)=>{session.check_new()?;(1,Some(expected_new(session.order())?))}
        (Intent::Resume,Original::Absent)=>(0,None),
    };
    channel.send(Kind::Inspect,&digest,&stage_body(tag,row))?;
    if m.intent==Intent::Lookup||tag==0||tag==3||tag==2&&m.intent==Intent::New{return Ok(());}
    let Some(row)=row else{return Err(bad("no original account intent"));};
    let live=||if getppid()==Some(parent)&&parent_process_death_signal()?==Some(Signal::TERM){
        Ok(())
    }else{Err(bad("lost original foreground supervisor"))};
    let selected=input.transpose()?;
    let prepared=if tag==1&&matches!(row,ResultRow::Setup(_)){
        let (warm,[])=channel.wait::<0>(Kind::Warm,&digest,until)?;
        if warm!=stage_body(tag,Some(row)){return Err(bad("wrong original scope for Web workspace preparation"));}
        live()?;
        let w=selected.as_ref().ok_or_else(||bad("no independently prepared Web input"))?.reserve_web(uid,gid)?;
        channel.send(Kind::Prepared,&digest,&stage_body(tag,Some(row)))?;
        Some(w)
    }else{None};
    let (act,[])=channel.wait::<0>(Kind::Act,&digest,until)?;
    if act!=stage_body(tag,Some(row)){return Err(bad("different approved original SQL account phase"));}
    live()?;
    let pending=match (tag,row){
        (1,ResultRow::Setup(ids))=>{
            let web=prepared.ok_or_else(||bad("no locked Web credential workspace"))?.read_held()?;
            live()?;
            session.reserve(Enrollment::Setup{ids,web:&web},until,&live)?
        }
        (1,ResultRow::Reset(_))=>session.reserve(Enrollment::Reset,until,&live)?,
        (2,_)=>session.sync_pending(row,until)?,
        _=>return Err(bad("unexpected new or original credential precondition")),
    };
    if pending.row()!=row{return Err(bad("reserved credential result changed"));}
    pending.require()?;live()?;
    channel.send(Kind::Reserved,&digest,&stage_body(2,Some(row)))?;
    let synced=Synced::receive(&channel,&digest,row,until)?;
    let row=pending.finish(&synced,live)?;
    channel.send(Kind::Complete,&digest,&stage_body(3,Some(row)))?;
    Ok(())
}

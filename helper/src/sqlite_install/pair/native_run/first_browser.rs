//! A bounded, unprivileged browser request to display the original console
//! confirmation. This never signs an INIT or accepts a path/data/secret from
//! HTTP. Only the authenticated exact process can return a request.
use super::{bad,host::Worker,manifest::read_sealed,transport::{self,Channel,Kind},worker::original_parent};
use crate::{disk_web,fs_tools::RuntimeFile,sqlite_install::{approval,pair::{
    domain::{give_live_approval,request_live_approval,sealed},terminal::PublicConfirmation},stage::random}};
use helper_linux_evidence::{diagnostic_process::{OfflineSql,account_sql_descriptors},prompt_signals::PromptSignals};
use rustix::net::UCred;
use rustix::process::{DumpableBehavior,Gid,Signal,Uid,dumpable_behavior,getppid,set_dumpable_behavior,set_parent_process_death_signal};
use sha2::{Digest,Sha256};
use std::{io,net::{Ipv4Addr,TcpListener},time::{Duration,Instant}};

const TAG:&[u8]=b"ZXS-HELPER/FIRST-BROWSER-1\0";
const FIXED:usize=TAG.len()+32+32+16+1+1;
const PANEL:Duration=Duration::from_secs(8*60);
const HELPER:&[&str]=&["usr","bin","helper"];
fn no()->io::Error{bad("the original first-install browser request is unconfirmed; no installation was started")}
pub(in crate::sqlite_install) fn offer(tty:&PublicConfirmation,scope:([u8;32],bool,u8),require:impl Fn()->io::Result<()>)->io::Result<()>{
    require()?;tty.check()?;
    let (actor,uid,gid)=approval::profile()?;
    let sig=PromptSignals::begin()?;
    let check=||{sig.check()?;tty.check()};
    let mut raw=TAG.to_vec();raw.extend(scope.0);raw.extend(random::<32>()?);raw.extend(actor);
    raw.push(u8::from(scope.1));raw.push(scope.2);
    let digest=Sha256::digest(&raw).into();let order=sealed(&raw)?;
    let helper=RuntimeFile::open(HELPER,true,64<<20)?;helper.require_running_helper()?;
    let (local,remote)=transport::pair()?;
    let mut original=Worker::spawn(&helper.file,&remote,&order,OfflineSql::FirstBrowser)?;
    drop(remote);drop(order);
    let who=UCred{pid:original.pid,uid:Uid::from_raw(uid),gid:Gid::from_raw(gid)};
    let deadline=Instant::now()+Duration::from_secs(10);
    loop{
        check()?;original.live()?;
        if Instant::now()>=deadline{return Err(no());}
        if give_live_approval(&local,who,&digest,||{require()?;helper.require_running_helper()})?{break;}
        std::thread::sleep(Duration::from_millis(20));
    }
    let channel=Channel::from_owned_child_session(local,who,digest)?;
    let (address,[])=channel.wait_checked::<0>(Kind::FirstBrowserReady,&digest,deadline,&check)?;
    let port=u16::from_be_bytes([address[0],address[1]]);
    if port==0||address[2..34]==[0;32]||address[34..]!=[0;31]{return Err(no());}
    let code=address[2..34].iter().map(|b|format!("{b:02x}")).collect::<String>();
    tty.report(&format!("仅本机首次安装请求：http://127.0.0.1:{port}/\n配对码（2 分钟有效，请勿分享）：{code}\n网页不提供存储写权限。确认之后回本终端校验原范围。\n"))?;
    drop(code);
    let end=Instant::now()+PANEL+Duration::from_secs(3);
    let (requested,[])=channel.wait_checked::<0>(Kind::FirstBrowserRequest,&digest,end,&check)?;
    if requested!=[0;65]{return Err(no());}
    // No browser/IPC work remains when the original foreground can ask.
    original.finish(&channel,end)?;
    check()?;sig.finish()?;drop(channel);drop(original);
    require()
}

pub(crate) fn worker()->io::Result<()>{
    let [fd,meta]=account_sql_descriptors()?;
    approval::only_inherited_descriptors(&[3,4])?;
    let parent=getppid().ok_or_else(no)?;original_parent(&fd,parent)?;
    approval::host_admin()?;let (actor,uid,gid)=approval::profile()?;
    let helper=RuntimeFile::open(HELPER,true,64<<20)?;
    helper.require_running_helper()?;helper.require_parent_helper(parent)?;drop(helper);
    approval::drop_to_storage(uid,gid)?;
    set_parent_process_death_signal(Some(Signal::TERM))?;original_parent(&fd,parent)?;
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    if dumpable_behavior()?!=DumpableBehavior::NotDumpable{return Err(no());}
    let raw=read_sealed(&meta,FIXED)?;drop(meta);
    let p=raw.strip_prefix(TAG).filter(|r|r.len()==82).ok_or_else(no)?;
    if p[..32]==[0;32]||p[32..64]==[0;32]||p[64..80]!=actor||p[80]>1||!(1..=62).contains(&p[81]){return Err(no());}
    let scope=Sha256::digest(&raw).into();
    request_live_approval(&fd,parent,&scope,||original_parent(&fd,parent))?;
    let channel=Channel::from_owned_child_session(fd,UCred{pid:parent,uid:Uid::ROOT,gid:Gid::ROOT},scope)?;
    let listener=TcpListener::bind((Ipv4Addr::LOCALHOST,0))?;
    listener.set_nonblocking(true)?;
    helper_linux_evidence::native_confinement::first_install_panel(&listener)?;
    let port=listener.local_addr()?.port();
    let mut code=[0u8;32];use rustix::rand::{GetRandomFlags,getrandom};
    if getrandom(&mut code[..],GetRandomFlags::NONBLOCK)?!=code.len()||code==[0;32]{return Err(no());}
    let mut message=[0u8;65];message[..2].copy_from_slice(&port.to_be_bytes());message[2..34].copy_from_slice(&code);
    channel.send(Kind::FirstBrowserReady,&scope,&message)?;
    let same=||{if getppid()!=Some(parent){return Err(no());}Ok(())};
    if !disk_web::serve_first(listener,p[80]!=0,p[81],code,Instant::now()+PANEL,same)?{return Err(no());}
    same()?;channel.send(Kind::FirstBrowserRequest,&scope,&[0;65])
}

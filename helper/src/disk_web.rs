//! Explicit local disk panel. Pairing alone never authorizes a checker or repair.
mod check;
mod http;
mod view;

use http::{Reply,Request};
use rustix::{process::{getgid,getuid},rand::{getrandom,GetRandomFlags}};
use sha2::{Digest,Sha256};
use std::{io::{self,Write},net::TcpListener,os::fd::AsFd,thread,time::{Duration,Instant}};

const PAIR:Duration=Duration::from_secs(120);
const SESSION:Duration=Duration::from_secs(600);
const HEX: &[u8;16]=b"0123456789abcdef";
fn token()->io::Result<String>{
    let mut b=[0;32];
    if getrandom(&mut b[..],GetRandomFlags::NONBLOCK)?!=b.len()||b==[0;32] {
        return Err(io::Error::other("secure panel credentials unavailable"));
    }
    let mut s=String::with_capacity(64);
    for c in b {s.push(HEX[usize::from(c>>4)] as char);s.push(HEX[usize::from(c&15)] as char);} Ok(s)
}
fn digest(s:&[u8])->[u8;32]{Sha256::digest(s).into()}
fn matches(s:&[u8],d:&[u8;32])->bool{
    s.len()==64 && s.iter().all(|&c|c.is_ascii_digit()||(b'a'..=b'f').contains(&c)) && digest(s)==*d
}
struct Session{bearer:[u8;32],csrf:[u8;32]}
struct Panel {
    host:String, origin:String, pairing:Option<[u8;32]>, failed:u8, session:Option<Session>,
    expires:Instant, closed:bool, check:Option<check::Check>,
}
impl Panel {
    fn new(host:String,code:&[u8],now:Instant)->Self{
        Self{origin:format!("http://{host}"),host,pairing:Some(digest(code)),failed:0,
            session:None,expires:now+PAIR,closed:false,check:None}
    }
    fn handle(&mut self,r:Request,now:Instant)->Reply{
        if self.closed||now>=self.expires{return Reply::plain(403,"会话已结束，请重新在终端启动。");}
        if r.host!=self.host{return Reply::plain(403,"需要本机准确的回环地址和端口。");}
        if r.method=="GET" && r.body.is_empty(){
            return match r.path.as_str(){
                "/"=>Reply::asset("text/html; charset=utf-8",include_str!("disk_web/index.html")),
                "/panel.js"=>Reply::asset("text/javascript; charset=utf-8",include_str!("disk_web/panel.js")),
                "/panel.css"=>Reply::asset("text/css; charset=utf-8",include_str!("disk_web/panel.css")),
                _=>Reply::plain(404,"未提供此页面。"),
            };
        }
        if r.method!="POST"{return Reply::plain(405,"不支持此请求。");}
        if r.origin.as_deref()!=Some(self.origin.as_str()){return Reply::plain(403,"来源不匹配。");}
        if r.path=="/api/pair"{
            let Some(expect)=self.pairing.as_ref()else{return Reply::plain(403,"配对已结束。");};
            if r.content_type.as_deref()!=Some("application/octet-stream")||r.auth.is_some()||r.csrf.is_some(){
                return Reply::plain(400,"不支持的配对请求。");
            }
            if !matches(&r.body,expect){
                self.failed+=1;
                if self.failed>=6{self.closed=true;return Reply::plain(429,"配对尝试已用完，请重新启动。");}
                return Reply::plain(403,"配对失败。");
            }
            let (Ok(bearer),Ok(csrf))=(token(),token())else{self.closed=true;return Reply::plain(503,"无法建立会话。");};
            if bearer==csrf || matches(bearer.as_bytes(),expect) || matches(csrf.as_bytes(),expect) {
                self.closed=true;return Reply::plain(503,"无法建立独立凭据。");
            }
            self.pairing=None;self.expires=now+SESSION;
            self.session=Some(Session{bearer:digest(bearer.as_bytes()),csrf:digest(csrf.as_bytes())});
            return Reply::plain(200,format!("{bearer}\n{csrf}"));
        }
        let Some(ref session)=self.session else{return Reply::plain(401,"请先在本机终端配对。");};
        if !r.auth.as_deref().and_then(|v|v.strip_prefix("Bearer ")).is_some_and(|v|matches(v.as_bytes(),&session.bearer))
            || !r.csrf.as_deref().is_some_and(|v|matches(v.as_bytes(),&session.csrf)){
            return Reply::plain(403,"会话凭据无效。");
        }
        if matches!(r.path.as_str(),"/api/copy/check"|"/api/copy/result"){
            if r.content_type.as_deref()!=Some("application/octet-stream")||r.body.len()!=64{
                return Reply::plain(400,"只接受本会话的固定长度预览票。");
            }
            return match self.check.as_mut(){Some(c)=>c.action(r.path=="/api/copy/check",&r.body,now),
                None=>Reply::plain(403,"终端未为本次面板授权任何镜像检查。")};
        }
        if !r.body.is_empty()||r.content_type.is_some(){return Reply::plain(400,"不接受路径、设备号、选项或请求正文。");}
        match r.path.as_str() {
            "/api/copy/preview"=>match self.check.as_mut(){Some(c)=>c.preview(now),
                None=>Reply::plain(403,"本次终端只开放观察，没有预选卷镜像。")},
            "/api/disks"=>view::disks(false),"/api/rates"=>view::disks(true),"/api/tools"=>view::tools(),
            "/api/close"=>{self.closed=true;Reply::plain(200,"不再接受操作；终端可能仍在等工作进程收尾。此回复不能作为升级许可。")},
            _=>Reply::plain(404,"没有该操作。"),
        }
    }
}

// Verify the already-held descriptor, not a mutable pathname or a generic
// isatty. Writing to a PTY master would feed input to its slave.
fn supported_terminal(mode:u32,dev:u64,magic:u64)->bool{
    let (major,minor)=(rustix::fs::major(dev),rustix::fs::minor(dev));
    mode & 0o170000 == 0o020000 && ((major,minor)==(5,0) ||
        (major==4 && (1..=63).contains(&minor)) ||
        ((136..=143).contains(&major) && magic==0x1cd1)) // Linux devpts slave
}
fn foreground(tty_sid:i32,our_sid:i32,tty_group:i32,our_group:i32)->bool{
    tty_sid>0 && tty_group>0 && tty_sid==our_sid && tty_group==our_group
}
pub(crate) fn require_terminal(fd:impl AsFd)->io::Result<()>{
    use rustix::{fs::{fstat,fstatfs},process::{getpgrp,getsid},termios::{tcgetpgrp,tcgetsid}};
    let deny=||io::Error::new(io::ErrorKind::PermissionDenied,"disk-web requires stderr on this operator's supported foreground controlling terminal");
    let stat=fstat(&fd)?;
    if !supported_terminal(stat.st_mode,stat.st_rdev,fstatfs(&fd)?.f_type as u64){return Err(deny());}
    if !foreground(tcgetsid(&fd)?.as_raw_pid(),getsid(None)?.as_raw_pid(),
        tcgetpgrp(&fd)?.as_raw_pid(),getpgrp().as_raw_pid()){return Err(deny());}
    Ok(())
}

pub(crate) fn serve(image:Option<(crate::fat32_check::Format,crate::fat32_check::Request)>)->io::Result<()>{
    // This command owns a fresh single-threaded process. No privileged Web
    // worker, inherited admin groups, ambient capabilities or fallback listener.
    rustix::thread::set_no_new_privs(true)?;
    crate::maintenance::diagnostic_worker(getuid().as_raw(),getgid().as_raw())?;
    let stderr=io::stderr();let mut tty=stderr.lock();
    require_terminal(stderr.as_fd())?;
    let selected=if let Some((format,job))=image{
        writeln!(tty,"正在读取终端指定的镜像并核对完整 SHA-256。仅保存封存副本；浏览器确认前不会运行检查器。")?;tty.flush()?;
        Some(check::Check::new(crate::fat32_check::panel::Operand::capture(format,job)?))
    }else{None};
    let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0))?;
    listener.set_nonblocking(true)?;
    let host=listener.local_addr()?.to_string();
    let code=token()?;let mut panel=Panel::new(host,code.as_bytes(),Instant::now());panel.check=selected;
    require_terminal(stderr.as_fd())?;
    writeln!(tty,"磁盘工具（本机只读）：{}\n一次性配对码（2 分钟有效，请勿分享）：{}",panel.origin,code)?;
    tty.flush()?;drop(code);drop(tty);
    // No listener thread, automatic probe or timer
    // refresh. Each connection has its own total read and write budgets.
    let transport=(|| -> io::Result<()>{
    let mut served=0usize;
    while !panel.closed && Instant::now()<panel.expires && served<512{
        if let Some(c)=&mut panel.check{c.poll();}
        match listener.accept() {
            Ok((mut socket,peer))=>{
                if !peer.ip().is_loopback(){continue;}
                served+=1;socket.set_nonblocking(false)?;
                let reply=match http::read(&mut socket) {Ok(r)=>panel.handle(r,Instant::now()),
                    Err(_)=>Reply::plain(400,"请求无效或未在时限内完成。")};
                let _=reply.send(&mut socket); // a disconnected browser does not extend the session
            },
            Err(e) if e.kind()==io::ErrorKind::WouldBlock=>thread::sleep(Duration::from_millis(40)),
            Err(e) if e.kind()==io::ErrorKind::Interrupted=>{},
            Err(e)=>return Err(e),
        }
    }
    Ok(())
    })();
    let disposition=panel.check.take().map_or(Ok(()),check::Check::finish);
    disposition.and(transport)
}

#[cfg(test)]
#[path="../tests/disk_web/session.rs"]
mod tests;

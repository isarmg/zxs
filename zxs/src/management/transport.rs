//! One bounded HTTP/1.1 origin-form request per connection. No re-use,
//! transfer codings, upstream user claims, file serving or forwarding trust.
//! The optional scoped proxy authenticates its own hop, never a Web user.
use super::{access::HOP_HEADER,manual,App,BAD,REJECT,Reply,Request,Token,WebAccess};
use crate::fs::invalid;
use std::{io::{self,Read,Write},net::{IpAddr,SocketAddr,TcpListener,TcpStream},os::fd::AsRawFd,
    sync::{OnceLock,atomic::{compiler_fence,AtomicBool,Ordering},mpsc},time::{Duration,Instant}};

pub(super) struct PrivateBytes(pub(super) Vec<u8>);
impl Drop for PrivateBytes{
    fn drop(&mut self){for b in self.0.iter_mut(){unsafe{std::ptr::write_volatile(b,0)}}compiler_fence(Ordering::SeqCst);}
}
const HEAD:usize=8192;
const BODY:usize=4096;
const MAX_FIELDS:usize=64;
const WORKERS:usize=8;
const IO_TIME:Duration=Duration::from_secs(4);
fn bad()->io::Error{invalid("malformed management request")}
fn number(s:&str)->io::Result<usize>{
    if s.is_empty()||s.len()>4||!s.bytes().all(|c|c.is_ascii_digit())||(s.len()>1&&s.starts_with('0')){return Err(bad());}
    s.parse::<usize>().ok().filter(|&v|v<=BODY).ok_or_else(bad)
}
fn cookie(s:&str,name:&str)->io::Result<Option<Token>>{
    if s.len()>2048{return Err(bad());}let mut found=None;
    for part in s.split(';'){
        let Some((n,v))=part.trim().split_once('=')else{if part.trim().is_empty(){continue;}return Err(bad());};
        if n==name{if found.is_some(){return Err(bad());}found=Some(Token::parse(v).ok_or_else(bad)?);}
    }Ok(found)
}
fn field_byte(c:u8)->bool{c.is_ascii_alphanumeric()||b"!#$%&'*+-.^_`|~".contains(&c)}
fn parse(mut input:impl Read,local:SocketAddr,peer:IpAddr,access:&WebAccess)->io::Result<Request>{
    // In proxy-only mode an actual non-loopback socket cannot even read
    // HTTP bytes, including a plausible Host or a guessed proxy header.
    if !access.permits_socket(local,peer){return Err(io::ErrorKind::PermissionDenied.into());}
    let mut raw=PrivateBytes(vec![0;HEAD]);let mut have=0;
    let end=loop{
        if have==HEAD{return Err(bad());}
        let size=input.read(&mut raw.0[have..])?;
        if size==0{return Err(io::ErrorKind::UnexpectedEof.into());}
        let from=have.saturating_sub(3);have+=size;
        if let Some(i)=raw.0[from..have].windows(4).position(|b|b==b"\r\n\r\n"){break from+i+4;}
    };
    let header=std::str::from_utf8(&raw.0[..end-4]).map_err(|_|bad())?;
    let mut lines=header.split("\r\n");let first=lines.next().ok_or_else(bad)?;
    let mut r=first.split(' ');
    let method=r.next().ok_or_else(bad)?;let target=r.next().ok_or_else(bad)?;
    if !matches!(method,"GET"|"POST")||r.next()!=Some("HTTP/1.1")||r.next().is_some()||
       target.len()>128||!target.starts_with('/')||target.starts_with("//")||
       !target.bytes().all(|c|(b'!'..=b'~').contains(&c)&&!matches!(c,b'\\'|b'#')){return Err(bad());}
    let(mut host,mut origin,mut cookies,mut ty,mut length,mut hop)=(None,None,None,None,None,None);
    for(i,line)in lines.enumerate(){
        if i>=MAX_FIELDS{return Err(bad());}
        let (k,v)=line.split_once(':').ok_or_else(bad)?;
        if k.is_empty()||!k.bytes().all(field_byte)||!v.bytes().all(|c|c==b'\t'||(32..127).contains(&c)){return Err(bad());}
        let v=v.trim_matches([' ','\t']);
        let slot=if k.eq_ignore_ascii_case("host"){&mut host}
            else if k.eq_ignore_ascii_case("origin"){&mut origin}
            else if k.eq_ignore_ascii_case("cookie"){&mut cookies}
            else if k.eq_ignore_ascii_case("content-type"){&mut ty}
            else if k.eq_ignore_ascii_case("content-length"){&mut length}
            else if k.eq_ignore_ascii_case(HOP_HEADER){&mut hop}
            else if ["transfer-encoding","expect","trailer","upgrade","content-encoding"].iter().any(|s|k.eq_ignore_ascii_case(s)){return Err(bad());}
            else{continue}; // includes Forwarded, X-Forwarded-* and Remote-User: never interpreted.
        if slot.replace(v).is_some(){return Err(bad());}
    }
    let host=host.ok_or_else(bad)?;
    let audience=access.audience(local,host,hop).ok_or_else(||io::Error::from(io::ErrorKind::PermissionDenied))?;
    let len=match (method,length){("GET",None)=>0,(_,Some(v))=>number(v)?,_=>return Err(bad())};
    if method=="GET"&&len!=0{return Err(bad());}
    let mut body=PrivateBytes(vec![0;len]);let already=(have-end).min(len);
    body.0[..already].copy_from_slice(&raw.0[end..end+already]);
    input.read_exact(&mut body.0[already..])?;
    Ok(Request{method:method.to_owned(),target:target.to_owned(),origin:origin.map(str::to_owned),
        content_type:ty.unwrap_or("").to_owned(),cookie:cookie(cookies.unwrap_or(""),audience.cookie())?,audience,body})
}
fn decode(buf:&mut [u8])->Option<&str>{
    let nibble=|x:u8|match x{b'0'..=b'9'=>Some(x-b'0'),b'a'..=b'f'=>Some(x-b'a'+10),b'A'..=b'F'=>Some(x-b'A'+10),_=>None};
    let(mut i,mut o)=(0,0);
    while i<buf.len(){let byte=match buf[i]{b'+'=>b' ',b'%'=>{let n=nibble(*buf.get(i+1)?)?*16+nibble(*buf.get(i+2)?)?;i+=2;n},b=>b};
        buf[o]=byte;o+=1;i+=1;
    }
    std::str::from_utf8(&buf[..o]).ok()
}
pub(super) fn fields<'a,const N:usize>(buf:&'a mut [u8],keys:[&str;N])->Option<[&'a str;N]>{
    let mut out=[None;N];for field in buf.split_mut(|b|*b==b'&'){
        let at=field.iter().position(|&c|c==b'=')?;let (name,value)=field.split_at_mut(at);
        let n=keys.iter().position(|k|k.as_bytes()==name)?;
        if out[n].is_some(){return None;}out[n]=Some(decode(&mut value[1..])?);
    }
    if out.iter().any(Option::is_none){return None;}Some(out.map(|v|v.unwrap()))
}
struct Timed<'a>(&'a TcpStream,Instant);
impl Timed<'_>{fn remaining(&self)->io::Result<Duration>{
    self.1.checked_duration_since(Instant::now()).filter(|t|!t.is_zero())
        .ok_or_else(||io::Error::new(io::ErrorKind::TimedOut,"management transport deadline"))
}}
impl Read for Timed<'_>{fn read(&mut self,buf:&mut[u8])->io::Result<usize>{
    self.0.set_read_timeout(Some(self.remaining()?))?;self.0.read(buf)
}}
impl Write for Timed<'_>{fn write(&mut self,p:&[u8])->io::Result<usize>{
    self.0.set_write_timeout(Some(self.remaining()?))?;self.0.write(p)
}fn flush(&mut self)->io::Result<()>{self.0.flush()}}
pub(super) fn respond(stream:TcpStream,access:&WebAccess,handle:impl FnOnce(Request)->io::Result<Reply>)->io::Result<()>{
    stream.set_nonblocking(false)?;
    let q=parse(Timed(&stream,Instant::now()+IO_TIME),stream.local_addr()?,stream.peer_addr()?.ip(),access);
    let result=match q{Ok(q)=>handle(q)?,Err(e)if e.kind()==io::ErrorKind::PermissionDenied=>REJECT(),
        Err(e)if matches!(e.kind(),io::ErrorKind::InvalidData|io::ErrorKind::InvalidInput)=>BAD(),Err(e)=>return Err(e)};
    result.write(Timed(&stream,Instant::now()+IO_TIME))
}
enum Job{Call(Request,mpsc::SyncSender<Reply>),Fault(io::Error)}
unsafe extern "C"{
    fn zxs_web_retry_accept(err:std::os::raw::c_int)->std::os::raw::c_int;
    fn zxs_web_readable(fd:std::os::raw::c_int)->std::os::raw::c_int;
}
fn retry_accept(e:&io::Error)->bool{e.raw_os_error().is_some_and(|n|unsafe{zxs_web_retry_accept(n)!=0})}
fn worker_fault(closing:&AtomicBool,jobs:&mpsc::SyncSender<Job>,e:io::Error){
    // Close the admission gate before another Call can bypass a queued Fault.
    closing.store(true,Ordering::Release);let _=jobs.send(Job::Fault(e));
}
fn admitted(closing:&AtomicBool,stopping:&impl Fn()->bool)->bool{
    // This sample is admission, not TCP accept or insertion in the queue.
    if stopping(){closing.store(true,Ordering::Release);}
    !closing.load(Ordering::Acquire)
}
fn admission(closing:&AtomicBool,stopping:&impl Fn()->bool,run:impl FnOnce()->Reply)->Reply{
    if admitted(closing,stopping){run()}else{stopped()}
}
fn stopped()->Reply{Reply::text("503 Service Unavailable","管理服务正在停止。")}
fn worker(ls:&TcpListener,access:&WebAccess,closing:&AtomicBool,accepting:&AtomicBool,jobs:mpsc::SyncSender<Job>){
    while !closing.load(Ordering::Acquire){
        if !accepting.load(Ordering::Acquire){
            std::thread::sleep(Duration::from_millis(10));continue;
        }
        let s=match unsafe{zxs_web_readable(ls.as_raw_fd())}{
            0=>continue,1=>match ls.accept(){Ok((s,_))=>s,Err(e)if retry_accept(&e)=>continue,
                Err(e)=>{worker_fault(closing,&jobs,e);break;}},
            _=>{worker_fault(closing,&jobs,io::Error::last_os_error());break;}
        };
        if closing.load(Ordering::Acquire){break;}
        let _=respond(s,access,|q|{
            let(tx,rx)=mpsc::sync_channel(1);
            jobs.send(Job::Call(q,tx)).map_err(|_|io::ErrorKind::BrokenPipe)?;
            rx.recv().map_err(|_|io::ErrorKind::BrokenPipe.into())
        });
    }
}
#[cfg(test)]
pub(super) fn serve(ls:&TcpListener,access:&WebAccess,app:&mut App<'_>,stopping:impl Fn()->bool+Sync)->io::Result<()>{
    let accepting=AtomicBool::new(true); // Only a fixture's private listener.
    serve_after(ls,access,app,stopping,&accepting,||Ok(()))
}
pub(super) fn serve_ready<F:Fn()->bool+Send+Sync>(ls:&TcpListener,access:&WebAccess,app:&mut App<'_>,
    ready:impl FnOnce()->io::Result<F>)->io::Result<()>{
    let accepting=AtomicBool::new(false);
    let original=OnceLock::<F>::new();
    let stopping=||original.get().is_some_and(|lost|lost());
    serve_after(ls,access,app,stopping,&accepting,||
        original.set(ready()?).map_err(|_|io::Error::other("the same Web original was already admitted")))
}
fn serve_after(ls:&TcpListener,access:&WebAccess,app:&mut App<'_>,stopping:impl Fn()->bool+Sync,
    accepting:&AtomicBool,ready:impl FnOnce()->io::Result<()>)->io::Result<()>{
    ls.set_nonblocking(true)?;let closing=AtomicBool::new(false);
    std::thread::scope(|scope|{
        let(tx,rx)=mpsc::sync_channel(WORKERS);let mut failure=None;
        let (manual,orders)=mpsc::sync_channel(1);let origin=app.source;let site=origin.manual_site();let closed=&closing;
        let original_stop=&stopping;
        std::thread::Builder::new().name("zxs-manual-work".into()).spawn_scoped(scope,move||manual::run(site,closed,original_stop,orders))?;
        for _ in 0..WORKERS{let jobs=tx.clone();let closed=&closing;
            if let Err(e)=std::thread::Builder::new().name("zxs-http-io".into()).spawn_scoped(scope,move||worker(ls,access,closed,accepting,jobs)){
                closing.store(true,Ordering::Release);failure=Some(e);break;
            }
        }drop(tx);
        if !accepting.load(Ordering::Acquire)&&failure.is_none(){
            // No worker accepted a tuple from the kernel before this cut.
            // Publish the actual readiness only after both the business
            // dispatcher and every bounded I/O worker have been created.
            let opened=ready().and_then(|()|if stopping(){Err(io::ErrorKind::Interrupted.into())}else{Ok(())});
            match opened{
                Ok(())=>accepting.store(true,Ordering::Release),
                Err(e)=>{closing.store(true,Ordering::Release);failure=Some(e);}
            }
        }
        loop{
            if stopping(){closing.store(true,Ordering::Release);}
            match rx.recv_timeout(Duration::from_millis(100)){
                Ok(Job::Call(q,reply))=>{
                    if q.method=="POST" && q.target=="/versions/commit" {
                        if !admitted(&closing,&stopping){let _=reply.send(stopped());continue;}
                        match app.manual_order(q,reply,Instant::now()){
                            Err((reply,rejected))=>{let _=reply.send(rejected);},
                            Ok(order)=>if let Err(err)=manual.try_send(order){
                                let order=match err{mpsc::TrySendError::Full(o)|mpsc::TrySendError::Disconnected(o)=>o};
                                let _=order.reply.send(Reply::text("503 Service Unavailable","已有手动提交正在核验，请稍后重送原表单。"));
                            },
                        }
                    }else{let _=reply.send(admission(&closing,&stopping,||app.handle(q,Instant::now())));}
                }
                Ok(Job::Fault(e))=>{closing.store(true,Ordering::Release);failure.get_or_insert(e);}
                Err(mpsc::RecvTimeoutError::Timeout)=>continue,Err(mpsc::RecvTimeoutError::Disconnected)=>break,
            }
        }
        drop(manual);
        failure.map_or(Ok(()),Err)
    })
}

#[cfg(test)]
mod tests{
    use super::*;
    fn read(wire:&str)->io::Result<Request>{parse(wire.as_bytes(),"192.0.2.10:8443".parse().unwrap(),"192.0.2.11".parse().unwrap(),&WebAccess::Direct)}
    const OK:&str="GET /login HTTP/1.1\r\nHost: 192.0.2.10:8443\r\n";
    #[test]fn fixed_http_framing_and_separate_authority(){
        assert!(read(&format!("{OK}Accept: */*\r\n\r\n")).is_ok());
        let mut q=read("POST /login HTTP/1.1\r\nhOsT: 192.0.2.10:8443\r\nContent-Length: 17\r\n\r\nlogin=a&password=").unwrap();
        assert_eq!(fields(&mut q.body.0,["login","password"]),Some(["a",""]));assert!(!q.audience.secure);
        for header in ["Host: 192.0.2.10:8443","Content-Length: 0\r\nContent-Length: 0","Content-Length : 0",
            " folded","Transfer-Encoding: chunked","Upgrade: h2c","Expect: 100-continue","Content-Length: 4097","Origin: null\r\nOrigin: null",
            "Cookie: zxs-http=bad","Trailer: Origin","X-Test: value\nnext", "Content-Encoding: gzip"]{
            assert!(read(&format!("{OK}{header}\r\n\r\n")).is_err());}
        for w in ["GET / HTTP/1.0\r\nHost: 192.0.2.10:8443\r\n\r\n","POST /login HTTP/1.1\r\nHost: 192.0.2.10:8443\r\n\r\n",
            "GET / HTTP/1.1\r\nHost: 0.0.0.0:8443\r\n\r\n","GET / HTTP/1.1\r\nHost: arbitrary.example\r\n\r\n"]{assert!(read(w).is_err());}
        assert!(read(&format!("{OK}{}\r\n","A: b\r\n".repeat(MAX_FIELDS))).is_err());
        assert!(read(&format!("{OK}A: {}\r\n\r\n","a".repeat(HEAD))).is_err());
        assert_eq!(read(&format!("{OK}\r\nGET /next HTTP/1.1\r\n\r\n")).unwrap().target,"/login");
        let mut a=b"password=%26%2B%25&login=manager".to_vec();assert_eq!(fields(&mut a,["login","password"]),Some(["manager","&+%"]));
        for b in [b"password=a&password=b".as_slice(),b"login=a&password=%GG",b"login=a&password=ok&extra=q"]{
            assert!(fields(&mut b.to_vec(),["login","password"]).is_none());}
    }

    #[test]
    fn proxy_only_checks_the_actual_socket_before_read_and_never_falls_back(){
        struct NoRead;
        impl Read for NoRead{fn read(&mut self,_:&mut [u8])->io::Result<usize>{panic!("foreign socket read HTTP");}}
        const ORIGIN:&str="https://admin.example:8444";
        const KEY:&str=concat!("a5a5a5a5a5a5a5a5","a5a5a5a5a5a5a5a5","a5a5a5a5a5a5a5a5","a5a5a5a5a5a5a5a5");
        // Independent SHA-256 of the published fixture's 64 ASCII characters,
        // deliberately NOT SHA-256 of the 32 decoded bytes.
        const SUM:&str="sha256:6eaea290aba480e118080ad627adff12fee3152d396b1f90807c6b527d4ac91f";
        let mode=WebAccess::external_confirmation(ORIGIN,SUM).unwrap();
        let local="127.0.0.1:8443".parse().unwrap();let peer="127.0.0.1".parse().unwrap();
        for (bind,other) in [(local,"192.0.2.10"),(local,"127.0.0.2"),(local,"::ffff:127.0.0.1"),
               ("192.0.2.10:8443".parse().unwrap(),"127.0.0.1"),("127.0.0.2:8443".parse().unwrap(),"127.0.0.1")]{
            assert_eq!(parse(NoRead,bind,other.parse().unwrap(),&mode).err().unwrap().kind(),io::ErrorKind::PermissionDenied);
        }
        let get=|host:&str,fields:&str|format!("GET /login HTTP/1.1\r\nHost: {host}\r\n{fields}\r\n");
        let read=|host:&str,fields:&str|parse(get(host,fields).as_bytes(),local,peer,&mode);
        let proof=format!("{HOP_HEADER}: {KEY}\r\n");
        let unrelated="Forwarded: proto=http;host=wrong.example\r\nX-Forwarded-Proto: http\r\nX-Forwarded-Host: wrong.example\r\nRemote-User: wrong\r\n";
        let accepted=read("admin.example:8444",&format!("{proof}{unrelated}")).unwrap();
        assert!(accepted.audience.secure);assert_eq!(accepted.audience.origin,ORIGIN);
        assert_eq!(accepted.audience.cookie(),"__Host-zxs");
        assert!(accepted.cookie.is_none()&&accepted.origin.is_none()&&accepted.body.0.is_empty());
        assert_eq!(accepted.audience.set_cookie("fixture",false),"__Host-zxs=fixture; Path=/; Secure; HttpOnly; SameSite=Strict");
        for wrong_host in ["127.0.0.1:8443","admin.example","Admin.example:8444","attacker.example:8444"]{
            assert!(read(wrong_host,&proof).is_err());
        }
        for denied in ["".to_owned(),unrelated.to_owned(),format!("{HOP_HEADER}: {}\r\n","A5".repeat(32)),
            format!("{HOP_HEADER}: {}\r\n", "a5".repeat(31)),format!("{HOP_HEADER}: {}\r\n", "a4".repeat(32)),
            format!("{HOP_HEADER}: {}\r\n",&SUM[7..]),format!("{proof}zxs-proxy-key: {KEY}\r\n"),
            format!("{proof}hOsT: admin.example:8444\r\n"),format!("{proof}Origin: {ORIGIN}\r\norigin: null\r\n")]{
            assert!(read("admin.example:8444",&denied).is_err());
        }
        let nonscope=get("127.0.0.1:8443","");
        assert!(parse(nonscope.as_bytes(),local,peer,&mode).is_err());
        let direct=WebAccess::Direct;
        assert!(parse(nonscope.as_bytes(),local,peer,&direct).is_ok());
        assert!(parse(get("127.0.0.1:8443",&proof).as_bytes(),local,peer,&direct).is_err());
        let binary_digest=WebAccess::external_confirmation(ORIGIN,
            "sha256:fc8b64001c5fdd0f2f40fb67dae4a865a2c5bd17836676d6d5b58b7917e33717").unwrap();
        assert!(parse(get("admin.example:8444",&proof).as_bytes(),local,peer,&binary_digest).is_err());
        let post=format!("POST /logout HTTP/1.1\r\nHost: admin.example:8444\r\nContent-Length: 0\r\nOrigin: null\r\n{proof}\r\n");
        // Transport preserves the original Origin verbatim, so the
        // existing App-level immutable-Origin/CSRF checks can reject it.
        assert_eq!(parse(post.as_bytes(),local,peer,&mode).unwrap().origin.as_deref(),Some("null"));
    }

    #[test]fn queued_call_is_not_admitted_on_stop_or_a_fault_still_in_the_channel(){
        let closed=AtomicBool::new(false);let stop=AtomicBool::new(false);
        let(tx,rx)=mpsc::sync_channel(2);let(out,back)=mpsc::sync_channel(1);
        assert!(tx.send(Job::Call(read(&format!("{OK}\r\n")).unwrap(),out)).is_ok());
        worker_fault(&closed,&tx,io::Error::other("listener fault"));
        let Job::Call(_,reply)=rx.recv().unwrap()else{panic!("queued call missing")};
        assert!(reply.send(admission(&closed,&||false,||panic!("a queued Call entered the application"))).is_ok());
        assert_eq!(back.recv().unwrap().status,"503 Service Unavailable");assert!(matches!(rx.recv(),Ok(Job::Fault(_))));
        let closed=AtomicBool::new(false);
        // Already admitted work may finish; the next call must not start.
        assert_eq!(admission(&closed,&||stop.load(Ordering::Acquire),||{
            stop.store(true,Ordering::Release);Reply::text("200 OK","finished")
        }).status,"200 OK");
        assert_eq!(admission(&closed,&||stop.load(Ordering::Acquire),||panic!("ran after stop")).status,"503 Service Unavailable");
    }
    #[cfg(all(target_os="linux",target_arch="x86_64"))]
    #[test]fn linux_tcp_accept_pending_faults_are_per_connection_not_fatal(){
        for err in [4,11,71,92,95,100,101,102,103,104,110,112,113]{assert!(retry_accept(&io::Error::from_raw_os_error(err)));}
        for err in [9,22,24,88]{assert!(!retry_accept(&io::Error::from_raw_os_error(err)));}
    }
}

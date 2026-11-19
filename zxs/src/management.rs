//! Fixed management application. Only the HTTPS terminator's protected local
//! transport can submit a request. No business CLI, Web FILE I/O or fallback.
use crate::{OperationId,PoolId,VolumeId,fs::invalid,identity::ShareId,system::reader::{InitialReader,account::WebAdmin}};
use std::{borrow::Cow,collections::{BTreeMap,VecDeque},fmt::Write as _,io::{self,Write},os::unix::net::UnixListener,
    path::Path,time::{Duration,Instant}};
mod pages;
mod transport;

const COOKIE:&str="__Host-zxs";
const IDLE:Duration=Duration::from_secs(15*60);
const ABSOLUTE:Duration=Duration::from_secs(8*3600);
const MAX_SESSIONS:usize=32;
const LOGIN_WINDOW:Duration=Duration::from_secs(60);
const MAX_ATTEMPTS:usize=12;

unsafe extern "C"{fn zxs_random(bytes:*mut u8,count:usize)->std::os::raw::c_int;}
#[derive(Eq,Ord,PartialEq,PartialOrd)]
struct Token([u8;32]);
impl Token{
    fn fresh()->io::Result<Self>{
        let mut v=[0u8;32];
        if unsafe{zxs_random(v.as_mut_ptr(),v.len())}!=0{return Err(io::Error::last_os_error());}
        if v==[0;32]{return Err(io::Error::other("missing session entropy"));}Ok(Self(v))
    }
    fn parse(s:&str)->Option<Self>{
        if s.len()!=64{return None;}
        let nibble=|c:u8|match c{b'0'..=b'9'=>Some(c-b'0'),b'a'..=b'f'=>Some(c-b'a'+10),_=>None};
        let mut v=[0;32];for (out,p) in v.iter_mut().zip(s.as_bytes().chunks_exact(2)){*out=nibble(p[0])?*16+nibble(p[1])?;}
        Some(Self(v))
    }
    fn hex(&self)->String{let mut s=String::with_capacity(64);for x in self.0{write!(s,"{x:02x}").unwrap();}s}
    fn matches(&self,other:&Self)->bool{self.0.iter().zip(other.0).fold(0u8,|n,(a,b)|n|(a^b))==0}
}
struct Session<'s>{admin:WebAdmin<'s>,csrf:Token,born:Instant,last:Instant}
struct App<'s>{source:&'s InitialReader,origin:String,sessions:BTreeMap<Token,Session<'s>>,attempts:VecDeque<Instant>}
struct Request{method:String,target:String,host:String,https:bool,origin:Option<String>,content_type:String,
    cookie:Option<Token>,body:transport::PrivateBytes}
struct Reply{status:&'static str,ty:&'static str,body:Cow<'static,str>,location:Option<&'static str>,set_cookie:Option<String>}
impl Reply{
    fn text(status:&'static str,body:&'static str)->Self{
        Self{status,ty:"text/plain; charset=utf-8",body:Cow::Borrowed(body),location:None,set_cookie:None}
    }
    fn html(status:&'static str,body:String)->Self{let mut r=Self::text(status,"");r.ty="text/html; charset=utf-8";r.body=Cow::Owned(body);r}
    fn redirect(to:&'static str)->Self{let mut r=Self::text("303 See Other","");r.location=Some(to);r}
    fn write(self,mut out:impl Write)->io::Result<()>{
        write!(out,"Status: {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: same-origin\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nContent-Security-Policy: default-src 'none'; style-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'\r\n",self.status,self.ty,self.body.len())?;
        if let Some(v)=self.location{write!(out,"Location: {v}\r\n")?;}
        if let Some(v)=self.set_cookie{write!(out,"Set-Cookie: {v}\r\n")?;}
        out.write_all(b"\r\n")?;out.write_all(self.body.as_bytes())?;out.flush()
    }
}
const REJECT:fn()->Reply=||Reply::text("403 Forbidden","请求未获准。");
const UNAVAILABLE:fn()->Reply=||Reply::text("503 Service Unavailable","当前无法核对管理状态，请稍后重试。");
const BAD:fn()->Reply=||Reply::text("400 Bad Request","请求格式不正确。");

impl<'s> App<'s>{
    fn new(source:&'s InitialReader,origin:&str)->io::Result<Self>{
        // One explicit HTTPS DNS/IPv4 authority (optional decimal port). No
        // inferred host, alternate origin, open redirect or wildcard.
        let auth=origin.strip_prefix("https://").ok_or_else(||invalid("expected canonical management HTTPS origin"))?;
        let(host,port)=auth.split_once(':').map_or((auth,None),|(h,p)|(h,Some(p)));
        if host.is_empty()||host.len()>253||host.split('.').any(|l|l.is_empty()||l.len()>63||l.starts_with('-')||l.ends_with('-')||
            !l.bytes().all(|c|c.is_ascii_lowercase()||c.is_ascii_digit()||c==b'-'))||
            port.is_some_and(|p|p.parse::<u16>().ok().is_none_or(|v|v==0||v==443||v.to_string()!=p)){
            return Err(invalid("invalid canonical HTTPS origin")); // default 443 is omitted by browsers
        }
        Ok(Self{source,origin:origin.to_owned(),sessions:BTreeMap::new(),attempts:VecDeque::new()})
    }
    fn handle(&mut self,mut q:Request,now:Instant)->Reply{
        if !q.https||self.origin.strip_prefix("https://")!=Some(q.host.as_str()){return REJECT();}
        self.sessions.retain(|_,s|now.saturating_duration_since(s.born)<ABSOLUTE&&now.saturating_duration_since(s.last)<IDLE);
        if q.method=="GET"{
            if !q.body.0.is_empty(){return BAD();}
            if q.target=="/login"{return Reply::html("200 OK",pages::sign_in(false));}
            if q.target=="/management.css"{
                let mut r=Reply::text("200 OK",pages::CSS);r.ty="text/css; charset=utf-8";return r;
            }
            let after=if q.target=="/"{None}else if let Some(s)=q.target.strip_prefix("/?after="){
                match s.parse::<ShareId>(){Ok(k)=>Some(k),Err(_)=>return BAD()}
            }else{return Reply::text("404 Not Found","没有此管理页面。");};
            let Some(k)=q.cookie.as_ref()else{return Reply::redirect("/login");};
            let Some(s)=self.sessions.get_mut(k)else{return Reply::redirect("/login");};
            return match s.admin.overview(after){
                Ok(data)=>match pages::overview(&data,&s.csrf.hex()){Ok(page)=>{s.last=now;Reply::html("200 OK",page)},Err(_)=>UNAVAILABLE()},
                Err(e) if e.kind()==io::ErrorKind::PermissionDenied=>{self.sessions.remove(k);Reply::redirect("/login")},
                Err(_)=>UNAVAILABLE()
            };
        }
        if q.method!="POST"||!matches!(q.target.as_str(),"/login"|"/logout"|"/shares/publish"|"/shares/mode"){return Reply::text("404 Not Found","没有此管理页面。");}
        if q.origin.as_deref()!=Some(self.origin.as_str()){return REJECT();}
        if q.content_type!="application/x-www-form-urlencoded"{return BAD();}
        if q.target=="/logout"{
            let Some([csrf])=transport::fields(&mut q.body.0,["csrf"])else{return BAD();};
            let Some(k)=q.cookie.as_ref()else{return REJECT();};
            let Some(s)=self.sessions.get(k)else{return REJECT();};
            if Token::parse(csrf).is_none_or(|c|!s.csrf.matches(&c)){return REJECT();}
            self.sessions.remove(k);let mut r=Reply::redirect("/login");
            r.set_cookie=Some(format!("{COOKIE}=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0"));return r;
        }
        if q.target.starts_with("/shares/"){
            let Some(k)=q.cookie.as_ref()else{return REJECT();};
            let Some(session)=self.sessions.get_mut(k)else{return REJECT();};
            let allowed=|v:&str|Token::parse(v).is_some_and(|t|session.csrf.matches(&t));
            let changed=if q.target=="/shares/publish"{
                let Some([csrf,op,volume,label])=transport::fields(&mut q.body.0,["csrf","op","volume","label"])else{return BAD();};
                if !allowed(csrf){return REJECT();}
                let(Ok(op),Ok(volume))=(op.parse::<OperationId>(),volume.parse::<VolumeId>())else{return BAD();};
                session.admin.publish_root(op,volume,label).map(|_|())
            }else{
                use crate::system::reader::account::share_mode::Mode;
                let Some([csrf,op,share,previous,mode])=transport::fields(&mut q.body.0,["csrf","op","share","previous","mode"])else{return BAD();};
                if !allowed(csrf){return REJECT();}
                let(Ok(op),Ok(share),Ok(previous))=(op.parse::<OperationId>(),share.parse::<ShareId>(),previous.parse::<OperationId>())else{return BAD();};
                let m=match mode{"off"=>Mode::Disabled,"ro"=>Mode::ReadOnly,"rw"=>Mode::ReadWrite,_=>return BAD()};
                session.admin.set_share_mode(op,share,previous,m).map(|_|())
            };
            return match changed{Ok(())=>{session.last=now;Reply::redirect("/")},
                Err(e)if e.kind()==io::ErrorKind::PermissionDenied=>REJECT(),
                Err(e)if e.kind()==io::ErrorKind::InvalidInput=>Reply::text("409 Conflict","参数、共享设置或原操作不符。请先核对当前状态。"),
                Err(_)=>Reply::text("503 Service Unavailable","本次结果未确认。原会话仍有效时请只重送原表单；若已过期，请重新登录先核对当前设置。")};
        }
        let Some([login,password])=transport::fields(&mut q.body.0,["login","password"])else{return BAD();};
        while self.attempts.front().is_some_and(|t|now.saturating_duration_since(*t)>=LOGIN_WINDOW){self.attempts.pop_front();}
        if self.attempts.len()>=MAX_ATTEMPTS{return Reply::text("429 Too Many Requests","登录尝试过多，请稍后再试。");}
        self.attempts.push_back(now);
        let admin=match self.source.authenticate_web(login,password){Ok(a)=>a,
            Err(e)if e.kind()==io::ErrorKind::PermissionDenied=>return Reply::html("403 Forbidden",pages::sign_in(true)),
            Err(_)=>return UNAVAILABLE()};
        let(Ok(key),Ok(csrf))=(Token::fresh(),Token::fresh())else{return UNAVAILABLE();};
        if self.sessions.contains_key(&key)||key==csrf{return UNAVAILABLE();}
        if let Some(old)=q.cookie.as_ref(){self.sessions.remove(old);}
        if self.sessions.len()>=MAX_SESSIONS{return Reply::text("429 Too Many Requests","管理会话已满，请稍后再试。");}
        let mut r=Reply::redirect("/");
        r.set_cookie=Some(format!("{COOKIE}={}; Path=/; Secure; HttpOnly; SameSite=Strict",key.hex()));
        self.sessions.insert(key,Session{admin,csrf,born:now,last:now});r
    }
}

/// Host-provided local Unix listener; this function neither binds a public
/// TCP port nor deploys a TLS terminator. The real S stays shared even idle.
/// A disconnect aborts this connection; no second response/replayed POST.
pub fn serve(system:&Path,pool:PoolId,https_origin:&str,proxy_uid:u32,listener:UnixListener,stopping:impl Fn()->bool)->io::Result<()>{
    if proxy_uid==0||proxy_uid==u32::MAX||proxy_uid==crate::fs::local_uid(){return Err(invalid("use a dedicated, non-root HTTPS terminator uid"));}
    let source=InitialReader::open(system,pool)?;let mut app=App::new(&source,https_origin)?;
    use std::os::fd::AsRawFd;
    unsafe extern "C"{fn zxs_web_readable(fd:std::os::raw::c_int)->std::os::raw::c_int;}
    listener.set_nonblocking(true)?;
    while !stopping(){
        match unsafe{zxs_web_readable(listener.as_raw_fd())}{0=>continue,1=>{},_=>return Err(io::Error::last_os_error())}
        let stream=match listener.accept(){Ok((s,_))=>s,Err(e)if matches!(e.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::Interrupted)=>continue,Err(e)=>return Err(e)};
        if stopping(){break;}
        stream.set_nonblocking(false)?;let _=transport::respond(&mut app,stream,proxy_uid);
    }
    Ok(())
}

#[cfg(test)]
#[path="../tests/unit/web_management.rs"]
mod tests;

//! ZXS management HTTP. Authentication and authorization stay with the original site.
use crate::{OperationId,identity::ShareId,system::reader::{InitialReader,account::{WebAdmin,credentials_refused}}};
use std::{borrow::Cow,collections::{BTreeMap,VecDeque},fmt::{self,Write as _},io::{self,Write},net::TcpListener,
    sync::{Arc,atomic::{AtomicBool,Ordering}},time::{Duration,Instant}};
mod pages;
mod transport;
mod manual;
mod directory;
mod browser;
use transport::Audience;

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
#[derive(Debug)]
struct InactiveSession;
impl fmt::Display for InactiveSession{
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("Web session is no longer active")}
}
impl std::error::Error for InactiveSession{}
fn inactive_session()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,InactiveSession)}
fn access_refused(error:&io::Error)->bool{
    credentials_refused(error)||error.get_ref().is_some_and(|cause|cause.is::<InactiveSession>())
}
struct Session<'s>{audience:Audience,admin:WebAdmin<'s>,csrf:Token,born:Instant,last:Instant,
    permit:Arc<AtomicBool>,chosen:manual::Chosen,directory_pages:directory::HeldPages<'s>}
impl Drop for Session<'_>{fn drop(&mut self){self.permit.store(false,Ordering::Release);}}
impl Session<'_>{
    fn finish_read<T>(&mut self,prepared:io::Result<T>)->io::Result<T>{
        finish_read_at(self.born,&mut self.last,Instant::now(),prepared)
    }
}
fn session_live_at(born:Instant,last:Instant,now:Instant)->bool{
    now.saturating_duration_since(born)<ABSOLUTE&&now.saturating_duration_since(last)<IDLE
}
fn finish_read_at<T>(born:Instant,last:&mut Instant,checked:Instant,prepared:io::Result<T>)->io::Result<T>{
    // Even a refusal can reveal metadata, such as whether a FILE exists.
    // Check the unrefreshed session after preparing the response, before
    // returning it or adopting a new form or continuation.
    if !session_live_at(born,*last,checked){return Err(inactive_session());}
    let prepared=prepared?;
    *last=checked;
    Ok(prepared)
}
struct App<'s>{source:&'s InitialReader,sessions:BTreeMap<Token,Session<'s>>,attempts:VecDeque<Instant>}
struct Request{method:String,target:String,audience:Audience,origin:Option<String>,content_type:String,
    cookie:Option<Token>,body:transport::PrivateBytes}
impl Request{
    fn same_origin_if_present(&self)->bool{self.origin.as_deref().is_none_or(|o|o==self.audience.origin)}
}
#[derive(Clone,Copy)]
enum Csp{
    Scriptless,
    BrowserOverview,
}
impl Csp{
    fn value(self)->&'static str{
        match self{
            Self::Scriptless=>"default-src 'none'; style-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
            Self::BrowserOverview=>"default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; worker-src 'none'; style-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        }
    }
}
struct Reply{status:&'static str,ty:&'static str,body:Cow<'static,[u8]>,csp:Csp,location:Option<&'static str>,set_cookie:Option<String>}
impl Reply{
    fn content(status:&'static str,ty:&'static str,body:Cow<'static,[u8]>)->Self{
        Self{status,ty,body,csp:Csp::Scriptless,location:None,set_cookie:None}
    }
    fn text(status:&'static str,body:&'static str)->Self{Self::content(status,"text/plain; charset=utf-8",Cow::Borrowed(body.as_bytes()))}
    fn html(status:&'static str,body:String)->Self{Self::content(status,"text/html; charset=utf-8",Cow::Owned(body.into_bytes()))}
    fn redirect(to:&'static str)->Self{let mut r=Self::text("303 See Other","");r.location=Some(to);r}
    fn write(self,mut out:impl Write)->io::Result<()>{
        write!(out,"HTTP/1.1 {}\r\nConnection: close\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: same-origin\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nContent-Security-Policy: {}\r\n",self.status,self.ty,self.body.len(),self.csp.value())?;
        if let Some(v)=self.location{write!(out,"Location: {v}\r\n")?;}
        if let Some(v)=self.set_cookie{write!(out,"Set-Cookie: {v}\r\n")?;}
        out.write_all(b"\r\n")?;out.write_all(&self.body)?;out.flush()
    }
}
const LOGIN_REQUIRED:fn()->Reply=||Reply::text("401 Unauthorized","请先登录。");
const REJECT:fn()->Reply=||Reply::text("403 Forbidden","请求未获准。");
const UNAVAILABLE:fn()->Reply=||Reply::text("503 Service Unavailable","当前无法核对管理状态，请稍后重试。");
const BAD:fn()->Reply=||Reply::text("400 Bad Request","请求格式不正确。");

impl<'s> App<'s>{
    fn new(source:&'s InitialReader)->Self{Self{source,sessions:BTreeMap::new(),attempts:VecDeque::new()}}
    fn expire(&mut self,now:Instant){
        self.sessions.retain(|_,s|session_live_at(s.born,s.last,now));
    }
    fn handle(&mut self,mut q:Request,now:Instant)->Reply{
        if let Some(k)=q.cookie.as_ref(){
            // A guessed/replayed token from a different accepted origin must
            // not inspect, refresh, log out, or replace the other session.
            if self.sessions.get(k).is_some_and(|s|s.audience!=q.audience){return REJECT();}
        }
        self.expire(now);
        if q.method=="GET"{
            if !q.body.0.is_empty(){return BAD();}
            if q.target=="/login"{return Reply::html("200 OK",pages::sign_in(false));}
            if q.target=="/management.css"{
                return Reply::content("200 OK","text/css; charset=utf-8",Cow::Borrowed(pages::CSS.as_bytes()));
            }
            if let Some(reply)=self.browser_asset(&q){return reply;}
            if let Some(reply)=self.directory_metadata(&q,now){return reply;}
            if let Some(args)=q.target.strip_prefix("/versions/history?file="){return self.manual_history(&q,args);}
            if let Some(id)=q.target.strip_prefix("/versions?file="){return self.manual_page(&q,id);}
            if q.target=="/versions"{return self.manual_index(&q,None);}
            if let Some(after)=q.target.strip_prefix("/versions?after="){return self.manual_index(&q,Some(after));}
            let after=if q.target=="/"{None}else if let Some(s)=q.target.strip_prefix("/?after="){
                match s.parse::<ShareId>(){Ok(k)=>Some(k),Err(_)=>return BAD()}
            }else{return Reply::text("404 Not Found","没有此管理页面。");};
            if !q.same_origin_if_present(){return REJECT();}
            let Some(k)=q.cookie.as_ref()else{return Reply::redirect("/login");};
            let Some(s)=self.sessions.get_mut(k)else{return Reply::redirect("/login");};
            let prepared=s.admin.overview(after).and_then(|data|browser::overview(&data,&s.csrf.hex()));
            return match s.finish_read(prepared){
                Ok(page)=>page,
                Err(e) if access_refused(&e)=>{self.sessions.remove(k);Reply::redirect("/login")},
                _=>UNAVAILABLE()
            };
        }
        if q.method=="POST"&&q.target==manual::RECEIPT{return self.manual_receipt(q,now);}
        if q.method!="POST"||!matches!(q.target.as_str(),"/login"|"/logout"|"/shares/mode"){return Reply::text("404 Not Found","没有此管理页面。");}
        if q.origin.as_deref()!=Some(q.audience.origin.as_str()){return REJECT();}
        if q.content_type!="application/x-www-form-urlencoded"{return BAD();}
        if q.target=="/logout"{
            let Some([csrf])=transport::fields(&mut q.body.0,["csrf"])else{return BAD();};
            let Some(k)=q.cookie.as_ref()else{return REJECT();};
            let Some(s)=self.sessions.get(k)else{return REJECT();};
            if Token::parse(csrf).is_none_or(|c|!s.csrf.matches(&c)){return REJECT();}
            self.sessions.remove(k);let mut r=Reply::redirect("/login");
            r.set_cookie=Some(transport::session_cookie("",true));return r;
        }
        if q.target.starts_with("/shares/"){
            let Some(k)=q.cookie.as_ref()else{return REJECT();};
            let Some(session)=self.sessions.get_mut(k)else{return REJECT();};
            let allowed=|v:&str|Token::parse(v).is_some_and(|t|session.csrf.matches(&t));
            use crate::system::reader::account::share_mode::Mode;
            let Some([csrf,op,share,previous,mode])=transport::fields(&mut q.body.0,["csrf","op","share","previous","mode"])else{return BAD();};
            if !allowed(csrf){return REJECT();}
            let(Ok(op),Ok(share),Ok(previous))=(op.parse::<OperationId>(),share.parse::<ShareId>(),previous.parse::<OperationId>())else{return BAD();};
            let m=match mode{"off"=>Mode::Disabled,"ro"=>Mode::ReadOnly,"rw"=>Mode::ReadWrite,_=>return BAD()};
            let changed=session.admin.set_installed_mode(op,share,previous,m).map(|_|());
            return match changed{Ok(())=>{session.last=now;Reply::redirect("/")},
                Err(e)if credentials_refused(&e)=>REJECT(),
                Err(e)if e.kind()==io::ErrorKind::InvalidInput=>Reply::text("409 Conflict","参数、共享设置或原操作不符。请先核对当前状态。"),
                Err(_)=>Reply::text("503 Service Unavailable","本次结果未确认。原会话仍有效时请只重送原表单；若已过期，请重新登录先核对当前设置。")};
        }
        let Some([login,password])=transport::fields(&mut q.body.0,["login","password"])else{return BAD();};
        while self.attempts.front().is_some_and(|t|now.saturating_duration_since(*t)>=LOGIN_WINDOW){self.attempts.pop_front();}
        if self.attempts.len()>=MAX_ATTEMPTS{return Reply::text("429 Too Many Requests","登录尝试过多，请稍后再试。");}
        self.attempts.push_back(now);
        let admin=match self.source.authenticate_web(login,password){Ok(a)=>a,
            Err(e)if credentials_refused(&e)=>return Reply::html("403 Forbidden",pages::sign_in(true)),
            Err(_)=>return UNAVAILABLE()};
        let(Ok(key),Ok(csrf))=(Token::fresh(),Token::fresh())else{return UNAVAILABLE();};
        if self.sessions.contains_key(&key)||key==csrf{return UNAVAILABLE();}
        if let Some(old)=q.cookie.as_ref(){self.sessions.remove(old);}
        if self.sessions.len()>=MAX_SESSIONS{return Reply::text("429 Too Many Requests","管理会话已满，请稍后再试。");}
        let mut r=Reply::redirect("/");
        r.set_cookie=Some(transport::session_cookie(&key.hex(),false));
        self.sessions.insert(key,Session{audience:q.audience,admin,csrf,born:now,last:now,
            permit:Arc::new(AtomicBool::new(true)),chosen:None,directory_pages:directory::HeldPages::default()});r
    }
}

/// The verified same-ELF Web role supplies its already qualified original
/// S, listener and READY cut. No selection, bind or settings fallback here.
pub(crate) fn serve_ready_original<F:Fn()->bool+Send+Sync>(source:&InitialReader,listener:&TcpListener,
    ready:impl FnOnce()->io::Result<F>)->io::Result<()>{
    transport::serve_ready(listener,&mut App::new(source),ready)
}

#[cfg(test)]
#[path="../tests/unit/web_management.rs"]
mod tests;

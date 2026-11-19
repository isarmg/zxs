//! One bounded HTTP/1.1 request per connection. No proxy, upgrade, compression,
//! request trailers, pipelining, chunked bodies or configurable request target.
use std::{io::{self,Read,Write},net::TcpStream,time::{Duration,Instant}};

const HEAD:usize=8192;
const BODY:usize=64;
pub(super) struct Request {
    pub method:String, pub path:String, pub host:String, pub origin:Option<String>,
    pub auth:Option<String>, pub csrf:Option<String>, pub content_type:Option<String>, pub body:Vec<u8>,
}
fn invalid()->io::Error{io::Error::new(io::ErrorKind::InvalidData,"invalid local panel request")}

fn parse(head:&[u8])->io::Result<(Request,usize)>{
    if head.len()>HEAD || head.windows(2).any(|p|p[0]==b'\r'&&p[1]!=b'\n'||p[1]==b'\n'&&p[0]!=b'\r')
        || head.starts_with(b"\r")||head.starts_with(b"\n"){return Err(invalid());}
    let mut hs=[httparse::EMPTY_HEADER;40];let mut r=httparse::Request::new(&mut hs);
    if r.parse(head).map_err(|_|invalid())?!=httparse::Status::Complete(head.len())||r.version!=Some(1){return Err(invalid());}
    let value=|name:&str|->io::Result<Option<String>>{
        let mut v=r.headers.iter().filter(|h|h.name.eq_ignore_ascii_case(name));
        let one=v.next().map(|v|std::str::from_utf8(v.value).map(str::to_owned).map_err(|_|invalid())).transpose()?;
        if v.next().is_some(){return Err(invalid());} Ok(one)
    };
    if ["Transfer-Encoding","Expect","Upgrade","Content-Encoding","Forwarded","X-Forwarded-Host",
        "X-Forwarded-For","X-Forwarded-Proto"].iter().any(|k|r.headers.iter().any(|h|h.name.eq_ignore_ascii_case(k))){
        return Err(invalid());
    }
    let size=match value("Content-Length")? {
        None=>0,Some(s) if !s.is_empty()&&s.bytes().all(|c|c.is_ascii_digit())=>s.parse::<usize>().map_err(|_|invalid())?,
        _=>return Err(invalid()),
    };
    if size>BODY{return Err(invalid());}
    Ok((Request{method:r.method.ok_or_else(invalid)?.into(),path:r.path.ok_or_else(invalid)?.into(),
        host:value("Host")?.ok_or_else(invalid)?,origin:value("Origin")?,auth:value("Authorization")?,
        csrf:value("X-Helper-CSRF")?,content_type:value("Content-Type")?,body:Vec::new()},size))
}

fn bounded_read(s:&mut TcpStream,buf:&mut[u8],until:Instant)->io::Result<usize>{
    let remain=until.checked_duration_since(Instant::now()).filter(|d|!d.is_zero()).ok_or_else(invalid)?;
    s.set_read_timeout(Some(remain))?;
    let n=s.read(buf)?;if n==0{Err(invalid())}else{Ok(n)}
}

pub(super) fn read(s:&mut TcpStream)->io::Result<Request>{
    let until=Instant::now()+Duration::from_secs(3);
    let mut data=Vec::with_capacity(1024);let mut buf=[0;1024];
    let end=loop{
        let n=bounded_read(s,&mut buf,until)?;data.extend_from_slice(&buf[..n]);
        if let Some(pos)=data.windows(4).position(|w|w==b"\r\n\r\n"){break pos+4;}
        if data.len()>=HEAD{return Err(invalid());}
    };
    let (mut r,size)=parse(&data[..end])?;
    if data.len()-end>size{return Err(invalid());}
    r.body.extend_from_slice(&data[end..]);
    while r.body.len()<size{let left=size-r.body.len();let n=bounded_read(s,&mut buf[..left],until)?;r.body.extend_from_slice(&buf[..n]);}
    Ok(r)
}

pub(super) struct Reply { pub code:u16,pub mime:&'static str,pub body:String }
impl Reply {
    pub fn plain(code:u16,s:impl Into<String>)->Self{Self{code,mime:"text/plain; charset=utf-8",body:s.into()}}
    pub fn asset(mime:&'static str,s:&'static str)->Self{Self{code:200,mime,body:s.into()}}
    pub fn send(self,s:&mut TcpStream)->io::Result<()>{
        let reason=match self.code{200=>"OK",400=>"Bad Request",401=>"Unauthorized",403=>"Forbidden",
            404=>"Not Found",405=>"Method Not Allowed",429=>"Too Many Requests",_=>"Service Unavailable"};
        let head=format!("HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: same-origin\r\nX-Frame-Options: DENY\r\nContent-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; form-action 'none'; base-uri 'none'\r\n\r\n",self.code,reason,self.mime,self.body.len());
        let stop=Instant::now()+Duration::from_secs(3);
        for piece in [head.as_bytes(),self.body.as_bytes()] {
            let mut left=piece;
            while !left.is_empty(){
                let remain=stop.checked_duration_since(Instant::now()).filter(|d|!d.is_zero()).ok_or_else(invalid)?;
                s.set_write_timeout(Some(remain))?;let n=s.write(left)?;if n==0{return Err(invalid());} left=&left[n..];
            }
        } Ok(())
    }
}
#[cfg(test)]
#[path="../../tests/disk_web/http.rs"]
mod tests;

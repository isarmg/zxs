//! SCGI/1 netstring from exactly one already-identified local TLS frontend.
//! Fixed fields, lengths, deadline, one request and one response per socket.
//! No HTTP-forwarded user, path, database, upstream retry or generic RPC.
use super::{App,BAD,COOKIE,Reply,Request,Token};
use crate::fs::invalid;
use std::{io::{self,Read,Write},os::{fd::AsRawFd,unix::net::UnixStream},sync::atomic::{compiler_fence,Ordering},time::{Duration,Instant}};

pub(super) struct PrivateBytes(pub(super) Vec<u8>);
impl Drop for PrivateBytes{
    fn drop(&mut self){for b in self.0.iter_mut(){unsafe{std::ptr::write_volatile(b,0)}}compiler_fence(Ordering::SeqCst);}
}
const HEAD:usize=8192;
const BODY:usize=4096;
unsafe extern "C"{fn zxs_web_peer(fd:std::os::raw::c_int,uid:u32)->std::os::raw::c_int;}
fn bad()->io::Error{invalid("malformed management request")}
fn number(p:&[u8],max:usize)->io::Result<usize>{
    if p.is_empty()||p.len()>max.to_string().len()||(p.len()>1&&p[0]==b'0'){return Err(bad());}
    let mut n=0usize;for &c in p{if !c.is_ascii_digit(){return Err(bad());}n=n*10+usize::from(c-b'0');}
    if n>max{return Err(bad());}Ok(n)
}
fn cookie(s:&str)->io::Result<Option<Token>>{
    if s.len()>2048{return Err(bad());}let mut found=None;
    for part in s.split(';'){
        let Some((n,v))=part.trim().split_once('=')else{if part.trim().is_empty(){continue;}return Err(bad());};
        if n==COOKIE{if found.is_some(){return Err(bad());}found=Some(Token::parse(v).ok_or_else(bad)?);}
    }
    Ok(found)
}
fn parse(mut input:impl Read)->io::Result<Request>{
    let mut size=[0u8;4];let mut used=0;
    loop{let mut c=[0];input.read_exact(&mut c)?;if c[0]==b':'{break;}
        if used==size.len()||!c[0].is_ascii_digit(){return Err(bad());}size[used]=c[0];used+=1;}
    let bytes=number(&size[..used],HEAD)?;let mut raw=PrivateBytes(vec![0;bytes]);input.read_exact(&mut raw.0)?;
    let mut comma=[0];input.read_exact(&mut comma)?;if comma!=[b',']||raw.0.last()!=Some(&0){return Err(bad());}
    const NAMES:[&str;9]=["CONTENT_LENGTH","SCGI","REQUEST_METHOD","REQUEST_URI","HTTPS","HTTP_HOST","HTTP_ORIGIN","HTTP_COOKIE","CONTENT_TYPE"];
    let mut fields:[Option<&str>;9]=[None;9];let mut parts=raw.0[..bytes-1].split(|&b|b==0);
    while let Some(name)=parts.next(){
        let pos=NAMES.iter().position(|n|n.as_bytes()==name).ok_or_else(bad)?;
        if fields[pos].is_some()||(fields[0].is_none()&&pos!=0){return Err(bad());}
        let val=parts.next().ok_or_else(bad)?;
        if !val.iter().all(|b|matches!(*b,0x20..=0x7e)){return Err(bad());}
        fields[pos]=Some(std::str::from_utf8(val).map_err(|_|bad())?);
    }
    let len=number(fields[0].ok_or_else(bad)?.as_bytes(),BODY)?;
    if fields[1]!=Some("1"){return Err(bad());}
    let method=fields[2].ok_or_else(bad)?;let target=fields[3].ok_or_else(bad)?;
    if method.len()>12||target.len()>128{return Err(bad());}
    let mut body=PrivateBytes(vec![0;len]);input.read_exact(&mut body.0)?;
    Ok(Request{method:method.to_owned(),target:target.to_owned(),https:fields[4]==Some("on"),
        host:fields[5].ok_or_else(bad)?.to_owned(),origin:fields[6].filter(|s|!s.is_empty()).map(str::to_owned),
        cookie:cookie(fields[7].unwrap_or(""))?,content_type:fields[8].unwrap_or("").to_owned(),body})
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
struct Timed<'a>(&'a UnixStream,Instant);
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
pub(super) fn respond(app:&mut App<'_>,stream:UnixStream,proxy_uid:u32)->io::Result<()>{
    if unsafe{zxs_web_peer(stream.as_raw_fd(),proxy_uid)}!=0{return Err(io::Error::last_os_error());}
    let q=parse(Timed(&stream,Instant::now()+Duration::from_secs(4)));
    let reply=match q{Ok(q)=>app.handle(q,Instant::now()),Err(e)if e.kind()==io::ErrorKind::InvalidData=>BAD(),
        Err(e)if e.kind()==io::ErrorKind::InvalidInput=>BAD(),Err(e)=>return Err(e)};
    reply.write(Timed(&stream,Instant::now()+Duration::from_secs(4)))
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn bounded_scgi_and_exact_forms(){
        fn frame(h:&[u8],b:&[u8])->Vec<u8>{let mut v=format!("{}:",h.len()).into_bytes();v.extend(h);v.push(b',');v.extend(b);v}
        let h=b"CONTENT_LENGTH\x0017\0SCGI\x001\0REQUEST_METHOD\0POST\0REQUEST_URI\0/login\0HTTPS\0on\0HTTP_HOST\0admin.example\0";
        let mut q=parse(frame(h,b"login=a&password=").as_slice()).unwrap();
        assert!(q.https);assert_eq!(fields(&mut q.body.0,["login","password"]),Some(["a",""]));
        let mut in_place=b"password=%26%2B%25&login=manager".to_vec();
        assert_eq!(fields(&mut in_place,["login","password"]),Some(["manager","&+%"]));
        for bad in [b"password=a&password=b".as_slice(),b"login=a&password=%GG",b"login=a&password=ok&extra=q"]{
            assert!(fields(&mut bad.to_vec(),["login","password"]).is_none());}
        for bad in [b"SCGI\x001\0CONTENT_LENGTH\x000\0".as_slice(),b"CONTENT_LENGTH\x000\0SCGI\x001\0SCGI\x001\0",
                    b"CONTENT_LENGTH\x004097\0SCGI\x001\0",b"CONTENT_LENGTH\x000\0REMOTE_USER\0manager\0"]{
            assert!(parse(frame(bad,b"").as_slice()).is_err());}
    }
}

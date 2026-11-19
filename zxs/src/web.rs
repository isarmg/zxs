//! Read-only Web response adapter for an explicitly selected published
//! representation. This is not an HTTP parser, network listener or login.
//! The service must authenticate, authorize the share and entry, and process
//! other HTTP preconditions *before* reaching this representation handler.
use crate::{hash,PublishedVersion};
use std::{fmt::Write as _,io::{self,Write}};

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum ReadMethod{Get,Head}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Body{Full,Part(u64,u64),Unsatisfied,Head}

fn text_field(raw:&[u8])->Option<&str>{
    // Transport-level request/header size limits belong before dispatch.
    if raw.len()>4096{return None;}
    Some(std::str::from_utf8(raw).ok()?.trim_matches([' ','\t']))
}
fn number(s:&str)->Option<u64>{
    if s.is_empty()||!s.bytes().all(|x|x.is_ascii_digit()){return None;}
    // All actual representation sizes fit in signed native file offsets.
    // Huge starts are unsatisfiable; huge lengths/ends clamp at actual EOF.
    Some(s.bytes().fold(0u64,|n,b|n.saturating_mul(10).saturating_add((b-b'0') as u64)))
}
fn single(v:&str,total:u64)->Body{
    // RFC 9110 allows ignoring ranges for an empty representation. In
    // particular, never invent a negative last position for an empty suffix.
    if total==0{return Body::Full;}
    let Some((unit,range))=v.split_once('=')else{return Body::Full};
    if !unit.eq_ignore_ascii_case("bytes")||range.contains(','){return Body::Full;}
    let Some((first,last))=range.trim_matches([' ','\t']).split_once('-')else{return Body::Full};
    if first.is_empty(){
        return match number(last){None=>Body::Full,Some(0)=>Body::Unsatisfied,Some(n)=>Body::Part(total.saturating_sub(n),total-1)};
    }
    let Some(start)=number(first)else{return Body::Full};
    let end=if last.is_empty(){u64::MAX}else{let Some(n)=number(last)else{return Body::Full};n};
    // Comparison must also be exact for two decimal positions both beyond
    // u64::MAX. Saturating is safe only once a reversed range is excluded.
    if !last.is_empty(){
        let a=first.trim_start_matches('0');let b=last.trim_start_matches('0');
        if (b.len(),b)<(a.len(),a){return Body::Full;}
    }
    if start>=total{return Body::Unsatisfied;}
    Body::Part(start,end.min(total-1))
}
fn plan(method:ReadMethod,ranges:&[&[u8]],if_ranges:&[&[u8]],tag:&str,total:u64)->Body{
    if method==ReadMethod::Head{return Body::Head;}
    let [raw]=ranges else{return Body::Full};
    match if_ranges{
        []=>{},[one] if text_field(one)==Some(tag)=>{},
        // This representation advertises a strong tag, no Last-Modified. A
        // weak tag, a date or a stale validator cannot authorize range reuse.
        _=>return Body::Full,
    }
    text_field(raw).map_or(Body::Full,|v|single(v,total))
}
/// RFC 6266 / RFC 8187: the ASCII fallback is constant. The extended value
/// encodes the node's verified creation-time business name, never its native
/// storage leaf, current display alias or an untrusted request component.
fn disposition(creation_name:&str)->String{
    const HEX:&[u8;16]=b"0123456789ABCDEF";
    let mut s=String::from("attachment; filename=\"download.bin\"; filename*=UTF-8''");
    for b in creation_name.bytes(){
        if b.is_ascii_alphanumeric()||matches!(b,b'-'|b'.'|b'_'|b'~'){s.push(char::from(b));}
        else{s.push('%');s.push(char::from(HEX[(b>>4) as usize]));s.push(char::from(HEX[(b&15) as usize]));}
    }
    s
}
fn etag(open:&PublishedVersion)->io::Result<String>{
    let n=open.receipt();let mut b=b"ZXS/HTTP/EXPLICIT-NODE/IDENTITY\0".to_vec();
    b.extend_from_slice(&n.file.0);b.extend_from_slice(&n.node.0);b.extend_from_slice(&n.version.0);
    b.extend_from_slice(&n.body.bytes.to_be_bytes());b.extend_from_slice(&n.body.sha256);
    let mut tag=String::with_capacity(66);tag.push('"');
    for x in hash::digest(&b)?{write!(&mut tag,"{x:02x}").expect("formatting into a String");}
    tag.push('"');Ok(tag)
}

/// Begin exactly one response and stream its body directly to the transport.
/// Header slices are the parsed Range / If-Range occurrences. Single byte
/// ranges are supported; other units, lists and malformed forms are ignored.
/// `begin` must publish these status/fields without changing the identity
/// coding. If body verification or the sink then fails, abort that response;
/// never append an error page or retry bytes after the response has begun.
pub fn send_published<W:Write>(
    open:&PublishedVersion,method:ReadMethod,ranges:&[&[u8]],if_ranges:&[&[u8]],
    begin:impl FnOnce(u16,&[(&'static str,&str)])->io::Result<W>,
)->io::Result<()>{
    open.require_held()?;
    let tag=etag(open)?;let total=open.receipt().body.bytes;
    let choice=plan(method,ranges,if_ranges,&tag,total);
    let(status,count,range)=match choice{
        Body::Full|Body::Head=>(200,total,None),
        Body::Part(a,b)=>(206,b-a+1,Some(format!("bytes {a}-{b}/{total}"))),
        Body::Unsatisfied=>(416,0,Some(format!("bytes */{total}"))),
    };
    let length=count.to_string();let name=disposition(&open.receipt().creation_name);
    let mut fields=vec![("ETag",tag.as_str()),("Accept-Ranges","bytes"),("Content-Length",length.as_str()),
        ("Cache-Control","private, no-store, no-transform"),("Content-Type","application/octet-stream"),
        ("Content-Disposition",name.as_str()),("X-Content-Type-Options","nosniff")];
    if let Some(value)=range.as_deref(){fields.push(("Content-Range",value));}
    let mut out=begin(status,&fields)?;
    match choice{
        Body::Full=>open.read_to(out),Body::Part(a,_)=>open.read_range_to(a,count,out),
        Body::Head|Body::Unsatisfied=>open.finish_headers(&mut out),
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn disposition_has_only_the_two_fixed_parameters(){
        let prefix="attachment; filename=\"download.bin\"; filename*=UTF-8''";
        for (name,suffix) in [("方案.xlsx","%E6%96%B9%E6%A1%88.xlsx"),("a;b%'c.xlsx","a%3Bb%25%27c.xlsx"),
            ("x\"\r\nX:y","x%22%0D%0AX%3Ay"),("Report.xlsx","Report.xlsx"),("report.xlsx","report.xlsx"),
            ("backup-0123456789abcdef0123456789abcdef.xlsx","backup-0123456789abcdef0123456789abcdef.xlsx")]{
            assert_eq!(disposition(name),format!("{prefix}{suffix}"));
        }
    }
    #[test]
    fn byte_ranges_are_inclusive_bounded_and_only_apply_to_get(){
        const TAG:&str="\"original\"";
        let decide=|s:&[u8]|plan(ReadMethod::Get,&[s],&[],TAG,10);
        assert_eq!(decide(b"bytes=2-4"),Body::Part(2,4));assert_eq!(decide(b"BYTES=8-19"),Body::Part(8,9));
        assert_eq!(decide(b"bytes=-4"),Body::Part(6,9));assert_eq!(decide(b"bytes=4-"),Body::Part(4,9));
        assert_eq!(decide(b"bytes=-999999999999999999999999999"),Body::Part(0,9));
        assert_eq!(decide(b"bytes=99999999999999999999999999-"),Body::Unsatisfied);
        assert_eq!(decide(b"bytes=99999999999999999999999999-88888888888888888888888888"),Body::Full);
        assert_eq!(decide(b"bytes=-0"),Body::Unsatisfied);assert_eq!(decide(b"bytes=10-"),Body::Unsatisfied);
        for x in [b"items=1-3".as_slice(),b"bytes=5-3",b"bytes=-",b"bytes=0-1,4-5",b"bytes=+1-2",b"bytes=\xff-1"]{
            assert_eq!(decide(x),Body::Full);
        }
        let r:&[&[u8]]=&[b"bytes=2-3"];
        assert_eq!(plan(ReadMethod::Head,r,&[],TAG,10),Body::Head);
        assert_eq!(plan(ReadMethod::Get,r,&[b"\"original\""],TAG,10),Body::Part(2,3));
        for v in [b"W/\"original\"".as_slice(),b"\"different\"",b"Sun, 06 Nov 1994 08:49:37 GMT"]{
            assert_eq!(plan(ReadMethod::Get,r,&[v],TAG,10),Body::Full);
        }
        assert_eq!(plan(ReadMethod::Get,&[b"bytes=-5"],&[],TAG,0),Body::Full);
    }
}

//! Wire adapter for the same authenticated, metadata-only directory read.
use super::{App,BAD,REJECT,Reply,Request,Session,UNAVAILABLE,session_live_at};
use crate::{DirectoryId,OperationId,identity::ShareId,system::reader::account::navigation::{Cursor,Directory,Entry,Page}};
use std::{borrow::Cow,collections::VecDeque,fmt::Write as _,io,time::Instant};

const PREFIX:&str="/ui/v1/d/";
const PAGE_HANDLES:usize=8;
const MAX_JSON:usize=128*1024;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Handle(OperationId);
#[derive(Debug,Eq,PartialEq)]
struct Query{share:ShareId,parent:Option<DirectoryId>,after:Option<Handle>}
impl Query{
    fn parse(parts:&str)->Option<Self>{
        let mut segments=parts.split('/');
        let share=segments.next()?.parse().ok()?;
        let parent=match segments.next()?{"-"=>None,id=>Some(id.parse().ok()?)};
        let after=match segments.next()?{"-"=>None,id=>Some(Handle(id.parse().ok()?))};
        segments.next().is_none().then_some(Self{share,parent,after})
    }
}

#[derive(Default)]
pub(super) struct HeldPages<'s>{pages:VecDeque<(Handle,Cursor<'s>)>}
fn remember<T>(held:&mut VecDeque<T>,next:T){
    if held.len()==PAGE_HANDLES{held.pop_front();}
    held.push_back(next);
}
impl<'s> HeldPages<'s>{
    fn find(&self,id:Handle)->Option<&Cursor<'s>>{self.pages.iter().find(|(key,_)|*key==id).map(|(_,cursor)|cursor)}
    fn reserve(&self)->io::Result<Handle>{
        let id=Handle(OperationId::new()?);
        if self.find(id).is_some(){return Err(io::Error::other("directory continuation entropy collision"));}
        Ok(id)
    }
}

fn string(out:&mut String,text:&str){
    out.push('"');
    for c in text.chars(){match c{
        '"'=>out.push_str("\\\""),
        '\\'=>out.push_str("\\\\"),
        '\u{0}'..='\u{1f}'|'\u{2028}'|'\u{2029}'=>write!(out,"\\u{:04x}",u32::from(c)).unwrap(),
        _=>out.push(c),
    }}
    out.push('"');
}
fn node(out:&mut String,kind:Option<&str>,id:impl std::fmt::Display,name:&str){
    out.push('{');
    if let Some(kind)=kind{out.push_str("\"kind\":");string(out,kind);out.push(',');}
    write!(out,"\"id\":\"{id}\",\"name\":").unwrap();string(out,name);out.push('}');
}
fn metadata(page:&Page<'_>,next:Option<Handle>)->io::Result<Reply>{
    let mut out=String::from("{\"ancestors\":[");
    for (i,Directory{id,name}) in page.ancestors.iter().enumerate(){
        if i!=0{out.push(',');}
        node(&mut out,None,id,name);
    }
    out.push_str("],\"entries\":[");
    for (i,entry) in page.entries.iter().enumerate(){
        if i!=0{out.push(',');}
        match entry{
            Entry::Directory(Directory{id,name})=>node(&mut out,Some("directory"),id,name),
            Entry::File{id,name}=>node(&mut out,Some("file"),id,name),
        }
    }
    out.push_str("],\"next\":");
    if let Some(id)=next{write!(out,"\"{}\"",id.0).unwrap();}else{out.push_str("null");}
    out.push('}');
    if out.len()>MAX_JSON{return Err(io::Error::other("directory metadata exceeds its response bound"));}
    let mut reply=Reply::text("200 OK","");
    reply.ty="application/json; charset=utf-8";
    reply.body=Cow::Owned(out);
    Ok(reply)
}
fn live_reply<T>(born:Instant,last:Instant,checked:Instant,prepared:io::Result<T>)->io::Result<T>{
    if session_live_at(born,last,checked){prepared}else{Err(io::ErrorKind::PermissionDenied.into())}
}
fn page(s:&mut Session<'_>,query:Query)->io::Result<Reply>{
    let prepared=(||{
        let after=match query.after{
            None=>None,
            Some(k)=>Some(s.directory_pages.find(k).ok_or_else(||io::Error::from(io::ErrorKind::WouldBlock))?),
        };
        let mut page=s.admin.navigation(query.share,query.parent,after)?;
        let next=page.next.take().map(|cursor|Ok::<_,io::Error>((s.directory_pages.reserve()?,cursor))).transpose()?;
        let reply=metadata(&page,next.as_ref().map(|(id,_)|*id))?;
        Ok((reply,next))
    })();
    let checked=Instant::now();
    let (reply,next)=live_reply(s.born,s.last,checked,prepared)?;
    if let Some(next)=next{remember(&mut s.directory_pages.pages,next);}
    s.last=checked;
    Ok(reply)
}
fn login_required()->Reply{Reply::text("401 Unauthorized","请先登录。")}
impl App<'_>{
    /// The UI composition selects this route; the stock dispatcher has no entry.
    pub(super) fn directory_metadata(&mut self,q:&Request,now:Instant)->Option<Reply>{
        let parts=q.target.strip_prefix(PREFIX)?;
        if q.method!="GET"||!q.body.0.is_empty(){return Some(BAD());}
        let Some(query)=Query::parse(parts)else{return Some(BAD());};
        if q.origin.as_deref().is_some_and(|origin|origin!=q.audience.origin){return Some(REJECT());}
        let Some(key)=q.cookie.as_ref()else{return Some(login_required());};
        if self.sessions.get(key).is_some_and(|s|s.audience!=q.audience){return Some(REJECT());}
        self.expire(now);
        let Some(s)=self.sessions.get_mut(key)else{return Some(login_required());};
        Some(match page(s,query){
            Ok(reply)=>reply,
            Err(e)=>match e.kind(){
                io::ErrorKind::PermissionDenied=>{self.sessions.remove(key);login_required()},
                io::ErrorKind::NotFound=>Reply::text("404 Not Found","没有此目录。"),
                io::ErrorKind::WouldBlock=>Reply::text("409 Conflict","请重新打开目录。"),
                _=>UNAVAILABLE(),
            },
        })
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::FileId;

    #[test]
    fn directory_metadata_accepts_only_three_canonical_segments(){
        let share="ab".repeat(16);let parent="cd".repeat(16);let next="ef".repeat(16);
        let complete=format!("{share}/{parent}/{next}");
        assert_eq!(format!("{PREFIX}{complete}").len(),107); // existing request target cap is 128.
        assert_eq!(Query::parse(&complete),Some(Query{
            share:share.parse().unwrap(),parent:Some(parent.parse().unwrap()),after:Some(Handle(next.parse().unwrap())),
        }));
        assert_eq!(Query::parse(&format!("{share}/-/-")),Some(Query{share:share.parse().unwrap(),parent:None,after:None}));
        for x in [format!("{share}/-"),format!("{share}/-/-/"),format!("{share}//-"),format!("{share}/-/-?page=2"),
            format!("{share}/{}/-",parent.to_uppercase()),"-/-/-".to_owned(),format!("{share}/-/{:032}",0)]{
            assert!(Query::parse(&x).is_none(),"{x}");
        }
    }

    #[test]
    fn directory_metadata_writes_the_typed_json_on_the_existing_response_boundary(){
        let p=Page{
            ancestors:vec![Directory{id:DirectoryId([0x12;16]),name:"上级".into()}],
            entries:vec![
                Entry::Directory(Directory{id:DirectoryId([0x13;16]),name:"第一层".into()}),
                Entry::File{id:FileId([0x14;16]),name:"页 \"一\"\n".into()},
            ],next:None,
        };
        let mut wire=Vec::new();
        metadata(&p,Some(Handle(OperationId([0x15;16])))).unwrap().write(&mut wire).unwrap();
        let wire=String::from_utf8(wire).unwrap();let (header,body)=wire.split_once("\r\n\r\n").unwrap();
        assert!(header.starts_with("HTTP/1.1 200 OK"));
        assert!(header.contains("Content-Type: application/json; charset=utf-8\r\n"));
        assert!(header.contains(&format!("Content-Length: {}\r\n",body.len())));
        assert!(header.contains("Cache-Control: no-store\r\n"));
        assert!(header.contains("Content-Security-Policy: default-src 'none'; style-src 'self';"));
        assert_eq!(body,concat!("{\"ancestors\":[{\"id\":\"12121212121212121212121212121212\",\"name\":\"上级\"}],",
            "\"entries\":[{\"kind\":\"directory\",\"id\":\"13131313131313131313131313131313\",\"name\":\"第一层\"},",
            "{\"kind\":\"file\",\"id\":\"14141414141414141414141414141414\",\"name\":\"页 \\\"一\\\"\\u000a\"}],",
            "\"next\":\"15151515151515151515151515151515\"}"));
        assert!(metadata(&p,None).unwrap().body.ends_with("\"next\":null}"));
        let empty=Page{ancestors:Vec::new(),entries:Vec::new(),next:None};
        assert_eq!(metadata(&empty,None).unwrap().body,"{\"ancestors\":[],\"entries\":[],\"next\":null}");
    }

    #[test]
    fn directory_metadata_obeys_both_original_expiry_boundaries(){
        use super::super::{ABSOLUTE,IDLE};
        use std::time::Duration;
        let born=Instant::now();
        let second=Duration::from_secs(1);
        let last=born+second;
        assert!(session_live_at(born,last,last+IDLE-second));
        assert!(!session_live_at(born,last,last+IDLE));
        let last=born+ABSOLUTE-second;
        let missing=||Err::<(),_>(io::ErrorKind::NotFound.into());
        assert!(live_reply(born,last,last,Ok(())).is_ok());
        assert_eq!(live_reply(born,last,last,missing()).unwrap_err().kind(),io::ErrorKind::NotFound);
        assert_eq!(live_reply(born,last,born+ABSOLUTE,Ok(())).unwrap_err().kind(),io::ErrorKind::PermissionDenied);
        assert_eq!(live_reply(born,last,born+ABSOLUTE,missing()).unwrap_err().kind(),io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn directory_metadata_keeps_only_the_eight_recently_minted_slots(){
        let mut slots=VecDeque::new();
        for i in 0..(PAGE_HANDLES+2){
            remember(&mut slots,i);
            assert!(slots.len()<=PAGE_HANDLES);
        }
        assert_eq!(slots.into_iter().collect::<Vec<_>>(),(2..PAGE_HANDLES+2).collect::<Vec<_>>());
    }
}

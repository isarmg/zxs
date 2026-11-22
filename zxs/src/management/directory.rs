//! Wire adapter for the same authenticated, metadata-only directory read.
use super::{App,BAD,LOGIN_REQUIRED,REJECT,Reply,Request,Session,UNAVAILABLE,access_refused};
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
    Ok(Reply::content("200 OK","application/json; charset=utf-8",Cow::Owned(out.into_bytes())))
}
fn page(s:&mut Session<'_>,query:&Query)->io::Result<Reply>{
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
    let (reply,next)=s.finish_read(prepared)?;
    if let Some(next)=next{remember(&mut s.directory_pages.pages,next);}
    Ok(reply)
}
impl App<'_>{
    pub(super) fn directory_metadata(&mut self,q:&Request,now:Instant)->Option<Reply>{
        let parts=q.target.strip_prefix(PREFIX)?;
        if q.method!="GET"||!q.body.0.is_empty(){return Some(BAD());}
        let Some(query)=Query::parse(parts)else{return Some(BAD());};
        if !q.same_origin_if_present(){return Some(REJECT());}
        let Some(key)=q.cookie.as_ref()else{return Some(LOGIN_REQUIRED());};
        if self.sessions.get(key).is_some_and(|s|s.audience!=q.audience){return Some(REJECT());}
        self.expire(now);
        let Some(s)=self.sessions.get_mut(key)else{return Some(LOGIN_REQUIRED());};
        Some(match page(s,&query){
            Ok(reply)=>reply,
            Err(e) if access_refused(&e)=>{self.sessions.remove(key);LOGIN_REQUIRED()},
            Err(e)=>match e.kind(){
                io::ErrorKind::NotFound=>Reply::text("404 Not Found","没有此目录。"),
                io::ErrorKind::WouldBlock=>Reply::text("409 Conflict","请重新打开目录。"),
                _=>UNAVAILABLE(),
            },
        })
    }
}

#[cfg(test)]
#[path = "../../tests/unit/attached/management/directory/tests.rs"]
mod tests;

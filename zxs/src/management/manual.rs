//! Only same-origin, CSRF-checked and positively displayed manual work
//! may be handed to the one bounded worker. No automatic publication path.
use super::{App,BAD,REJECT,UNAVAILABLE,Reply,Request,Session,Token,pages,transport};
use crate::{FileId,OperationId,system::reader::account::manual_work::{Site,Target,Who}};
use std::{io,sync::{Arc,atomic::{AtomicBool,Ordering},mpsc},time::Instant};

pub(super) type Chosen=Option<(OperationId,Target)>;
pub(super) const RECEIPT:&str="/versions/receipt";

pub(super) struct Work {
    op:OperationId,
    target:Target,
    who:Who,
    session:Arc<AtomicBool>,
    before:Instant,
    pub(super) reply:mpsc::SyncSender<Reply>,
}
fn conflict()->Reply{Reply::text("409 Conflict","工作稿已变化、正在打开写入或尚未确认。请先关闭写入程序，再核对当前工作稿。")}
fn stale_form()->Reply{Reply::text("409 Conflict","表单已失效：这次没有提交，也未核对上一次的结果。如果没看到带版本 ID 的成功页，不要用新表单重试上一次提交。")}
pub(super) fn live(stopping:&impl Fn()->bool,closing:&AtomicBool,session:&AtomicBool,before:Instant)->io::Result<()>{
    if stopping(){closing.store(true,Ordering::Release);}
    if closing.load(Ordering::Acquire)||!session.load(Ordering::Acquire)||Instant::now()>=before{
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    }else{Ok(())}
}
impl Work{
    fn run(&self,site:&Site<'_>,closing:&AtomicBool,stopping:&impl Fn()->bool)->Reply{
        let qualifies=||live(stopping,closing,&self.session,self.before);
        match site.freeze(self.who,self.op,self.target,qualifies) {
            Ok(r)=>pages::frozen(r),
            Err(e)if e.kind()==io::ErrorKind::PermissionDenied=>REJECT(),
            Err(e)if matches!(e.kind(),io::ErrorKind::InvalidInput|io::ErrorKind::WouldBlock)=>conflict(),
            Err(e)if e.kind()==io::ErrorKind::NotFound=>Reply::text("503 Service Unavailable","文件已登记，但暂时无法核对原数据盘和文件。"),
            Err(_)=>Reply::text("503 Service Unavailable","未确认本次结果。原会话与表单仍有效时，可用“只查本次回执”核对；勿用新表单重发。"),
        }
    }
}
pub(super) fn run(site:Site<'_>,closing:&AtomicBool,stopping:&impl Fn()->bool,jobs:mpsc::Receiver<Work>){
    for job in jobs{
        let reply=job.run(&site,closing,stopping);
        let _=job.reply.send(reply);
    }
}
impl<'s> App<'s>{
    pub(super) fn manual_index(&mut self,q:&Request,after:Option<&str>)->Reply{
        let Some(key)=q.cookie.as_ref()else{return Reply::redirect("/login")};
        let Some(s)=self.sessions.get_mut(key)else{return Reply::redirect("/login")};
        let after=match after.map(str::parse::<FileId>).transpose(){Ok(k)=>k,Err(_)=>return BAD()};
        let prepared=s.admin.manual_files(after).map(|(items,next)|pages::manual_index(&items,next));
        match s.finish_read(prepared){
            Ok(reply)=>reply,
            Err(e)if e.kind()==io::ErrorKind::PermissionDenied=>{self.sessions.remove(key);Reply::redirect("/login")},
            Err(_)=>UNAVAILABLE(),
        }
    }
    pub(super) fn manual_history(&mut self,q:&Request,target:&str)->Reply{
        let Some(key)=q.cookie.as_ref()else{return Reply::redirect("/login")};
        let Some(s)=self.sessions.get_mut(key)else{return Reply::redirect("/login")};
        let (id,upper)=match target.split_once("&before="){
            Some((id,raw))=>{
                if raw.is_empty()||raw.starts_with('0')||!raw.as_bytes().iter().all(u8::is_ascii_digit){return BAD();}
                let Ok(cursor)=raw.parse::<i64>() else{return BAD()};(id,Some(cursor))
            },
            None=>(target,None),
        };
        let Ok(file)=id.parse::<FileId>() else{return BAD()};
        let prepared=s.admin.manual_timeline(file,upper).map(|history|pages::version_history(&history));
        match s.finish_read(prepared){
            Ok(reply)=>reply,
            Err(e) if e.kind()==io::ErrorKind::PermissionDenied=>{self.sessions.remove(key);Reply::redirect("/login")},
            Err(e) if e.kind()==io::ErrorKind::NotFound=>Reply::text("404 Not Found","没有此文件。"),
            Err(_)=>UNAVAILABLE(),
        }
    }
    pub(super) fn manual_page(&mut self,q:&Request,id:&str)->Reply{
        let Ok(file)=id.parse::<FileId>() else{return BAD()};
        let Some(key)=q.cookie.as_ref()else{return Reply::redirect("/login")};
        let Some(s)=self.sessions.get_mut(key)else{return Reply::redirect("/login")};
        s.chosen=None;
        let prepared=s.admin.manual_preview(file).and_then(|(target,item)|{
            let op=OperationId::new().map_err(io::Error::other)?;
            Ok((op,target,pages::manual(&item,target,op,&s.csrf.hex())))
        });
        match s.finish_read(prepared){
            Ok((op,target,reply))=>{s.chosen=Some((op,target));reply},
            Err(e)if e.kind()==io::ErrorKind::PermissionDenied=>{self.sessions.remove(key);Reply::redirect("/login")},
            Err(e)if e.kind()==io::ErrorKind::NotFound=>Reply::text("404 Not Found","没有此文件。"),
            Err(e)if e.kind()==io::ErrorKind::WouldBlock=>conflict(),
            Err(_)=>UNAVAILABLE(),
        }
    }
    pub(super) fn manual_order(&mut self,mut q:Request,reply:mpsc::SyncSender<Reply>,now:Instant)->Result<Work,(mpsc::SyncSender<Reply>,Reply)>{
        if q.method!="POST"||q.target!="/versions/commit"{return Err((reply,REJECT()));}
        let (s,op,target)=match self.original_form(&mut q,now){Ok(form)=>form,Err(e)=>return Err((reply,e))};
        s.last=now;
        Ok(Work{op,target,who:s.admin.manual_author(),session:s.permit.clone(),
            before:(s.born+super::ABSOLUTE).min(s.last+super::IDLE),reply})
    }
    pub(super) fn manual_receipt(&mut self,mut q:Request,now:Instant)->Reply{
        if q.method!="POST"||q.target!=RECEIPT{return REJECT();}
        let (s,op,target)=match self.original_form(&mut q,now){Ok(form)=>form,Err(e)=>return e};
        let prepared=s.admin.manual_receipt(op,target).map(|result|match result{
            Some(receipt)=>pages::manual_receipt(receipt),
            None=>Reply::text("200 OK","截至本次核对，未找到这份表单的已确认提交回执。原请求仍可能在处理。本查询没有提交或重试。"),
        });
        match s.finish_read(prepared){
            Ok(reply)=>reply,
            Err(e)if e.kind()==io::ErrorKind::PermissionDenied=>{
                if let Some(key)=q.cookie.as_ref(){self.sessions.remove(key);}
                REJECT()
            },
            Err(_)=>Reply::text("503 Service Unavailable","目前无法核对这份表单的提交回执，结果未知。本查询没有提交或重试。"),
        }
    }
    fn original_form(&mut self,q:&mut Request,now:Instant)->Result<(&mut Session<'s>,OperationId,Target),Reply>{
        if q.origin.as_deref()!=Some(q.audience.origin.as_str()){return Err(REJECT());}
        if q.content_type!="application/x-www-form-urlencoded"{return Err(BAD());}
        let Some([csrf,op])=transport::fields(&mut q.body.0,["csrf","op"])else{return Err(BAD())};
        let (Some(csrf),Ok(op))=(Token::parse(csrf),op.parse::<OperationId>())else{return Err(REJECT())};
        self.expire(now);
        let Some(s)=q.cookie.as_ref().and_then(|key|self.sessions.get_mut(key))else{return Err(REJECT())};
        if s.audience!=q.audience||!s.csrf.matches(&csrf){return Err(REJECT());}
        let Some((selected,target))=s.chosen else{return Err(stale_form())};
        if selected!=op{return Err(stale_form());}
        Ok((s,op,target))
    }
}

#[cfg(test)]
#[path = "../../tests/unit/attached/management/manual/tests.rs"]
mod tests;

//! Only same-origin, CSRF-checked and positively displayed manual work
//! may be handed to the one bounded worker. No automatic publication path.
use super::{App,BAD,REJECT,UNAVAILABLE,Reply,Request,Token,pages,transport};
use crate::{FileId,OperationId,system::reader::account::manual_work::{Site,Target,Who}};
use std::{io,sync::{Arc,atomic::{AtomicBool,Ordering},mpsc},time::Instant};

pub(super) type Chosen=Option<(OperationId,Target)>;

pub(super) struct Work {
    op:OperationId,
    target:Target,
    who:Who,
    session:Arc<AtomicBool>,
    before:Instant,
    pub(super) reply:mpsc::SyncSender<Reply>,
}
fn conflict()->Reply{Reply::text("409 Conflict","工作稿已变化、正在打开写入或尚未确认。请先关闭写入程序，再核对当前工作稿。")}
fn live(stopping:&impl Fn()->bool,closing:&AtomicBool,session:&AtomicBool,before:Instant)->io::Result<()>{
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
            Err(_)=>Reply::text("503 Service Unavailable","未确认本次结果。原会话仍有效时请只重送原表单，勿另发提交。"),
        }
    }
}
pub(super) fn run(site:Site<'_>,closing:&AtomicBool,stopping:&impl Fn()->bool,jobs:mpsc::Receiver<Work>){
    for job in jobs{
        let reply=job.run(&site,closing,stopping);
        let _=job.reply.send(reply);
    }
}
impl App<'_>{
    pub(super) fn manual_index(&mut self,q:&Request,after:Option<&str>,now:Instant)->Reply{
        let Some(key)=q.cookie.as_ref()else{return Reply::redirect("/login")};
        let Some(s)=self.sessions.get_mut(key)else{return Reply::redirect("/login")};
        let after=match after.map(str::parse::<FileId>).transpose(){Ok(k)=>k,Err(_)=>return BAD()};
        match s.admin.manual_files(after){
            Ok((items,next))=>{s.last=now;pages::manual_index(&items,next)},
            Err(e)if e.kind()==io::ErrorKind::PermissionDenied=>{self.sessions.remove(key);Reply::redirect("/login")},
            Err(_)=>UNAVAILABLE(),
        }
    }
    pub(super) fn manual_page(&mut self,q:&Request,id:&str,now:Instant)->Reply{
        let Ok(file)=id.parse::<FileId>() else{return BAD()};
        let Some(key)=q.cookie.as_ref()else{return Reply::redirect("/login")};
        let Some(s)=self.sessions.get_mut(key)else{return Reply::redirect("/login")};
        s.chosen=None;
        match s.admin.manual_preview(file){
            Ok((target,item))=>match OperationId::new(){
                Ok(op)=>{s.chosen=Some((op,target));s.last=now;pages::manual(&item,target,op,&s.csrf.hex())},
                Err(_)=>UNAVAILABLE()
            },
            Err(e)if e.kind()==io::ErrorKind::PermissionDenied=>{self.sessions.remove(key);Reply::redirect("/login")},
            Err(e)if e.kind()==io::ErrorKind::NotFound=>Reply::text("404 Not Found","没有此文件。"),
            Err(e)if e.kind()==io::ErrorKind::WouldBlock=>conflict(),
            Err(_)=>UNAVAILABLE(),
        }
    }
    pub(super) fn manual_order(&mut self,mut q:Request,reply:mpsc::SyncSender<Reply>,now:Instant)->Result<Work,(mpsc::SyncSender<Reply>,Reply)>{
        if q.method!="POST"||q.target!="/versions/commit"||q.origin.as_deref()!=Some(q.audience.origin.as_str()){
            return Err((reply,REJECT()));
        }
        if q.content_type!="application/x-www-form-urlencoded"{return Err((reply,BAD()));}
        let Some([csrf,op])=transport::fields(&mut q.body.0,["csrf","op"])else{return Err((reply,BAD()))};
        let (Some(csrf),Ok(op))=(Token::parse(csrf),op.parse::<OperationId>())else{return Err((reply,REJECT()))};
        self.expire(now);
        let Some(s)=q.cookie.as_ref().and_then(|key|self.sessions.get_mut(key))else{return Err((reply,REJECT()))};
        if s.audience!=q.audience||!s.csrf.matches(&csrf){return Err((reply,REJECT()));}
        let Some((selected,target))=s.chosen else{return Err((reply,conflict()))};
        if selected!=op{return Err((reply,conflict()));}
        s.last=now;
        Ok(Work{op,target,who:s.admin.manual_author(),session:s.permit.clone(),
            before:(s.born+super::ABSOLUTE).min(s.last+super::IDLE),reply})
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn actual_stop_is_consulted_even_before_owner_mirrors_it(){
        let original=AtomicBool::new(false);
        let owner_mirror=AtomicBool::new(false);
        let session=AtomicBool::new(true);
        let expires=Instant::now()+std::time::Duration::from_secs(60);
        let stop=||original.load(Ordering::Acquire);
        assert!(live(&stop,&owner_mirror,&session,expires).is_ok());
        original.store(true,Ordering::Release);
        assert!(!owner_mirror.load(Ordering::Acquire));
        assert_eq!(live(&stop,&owner_mirror,&session,expires).unwrap_err().kind(),io::ErrorKind::PermissionDenied);
        assert!(owner_mirror.load(Ordering::Acquire));
    }
}

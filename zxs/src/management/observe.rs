//! The on-demand volume status lane is read-only and separate from commits.
use super::{App,BAD,REJECT,UNAVAILABLE,Reply,Request,Token,manual,pages,transport};
use crate::{VolumeId,system::reader::account::manual_work::{Site,Who}};
use std::{io,sync::{Arc,atomic::AtomicBool,mpsc},time::{Duration,Instant}};

pub(super) const TARGET:&str="/volumes/observe";
pub(super) const WAIT:Duration=Duration::from_secs(10);
pub(super) fn unconfirmed()->Reply{
    Reply::text("503 Service Unavailable","未能完成本次磁盘核对，请稍后再试。")
}
pub(super) struct Work{
    id:VolumeId,
    who:Who,
    session:Arc<AtomicBool>,
    before:Instant,
    until:Instant,
    pub(super) reply:mpsc::SyncSender<Reply>,
}
impl Work{
    fn run(&self,site:&Site<'_>,closing:&AtomicBool,stopping:&impl Fn()->bool)->Reply{
        let live=||{
            manual::live(stopping,closing,&self.session,self.before)?;
            if Instant::now()>=self.until{return Err(io::ErrorKind::TimedOut.into());}
            Ok(())
        };
        match site.observe(self.who,self.id,&live){
            Ok(report)=>match live(){
                Ok(())=>pages::volume_status(&report),
                Err(e)if e.kind()==io::ErrorKind::PermissionDenied=>REJECT(),
                Err(_)=>unconfirmed(),
            },
            Err(e)if e.kind()==io::ErrorKind::PermissionDenied=>REJECT(),
            Err(e)if e.kind()==io::ErrorKind::NotFound=>Reply::text("404 Not Found","没有此登记卷。"),
            Err(e)if e.kind()==io::ErrorKind::TimedOut=>unconfirmed(),
            Err(_)=>UNAVAILABLE(),
        }
    }
}
pub(super) fn run(site:Site<'_>,closing:&AtomicBool,stopping:&impl Fn()->bool,jobs:mpsc::Receiver<Work>){
    for job in jobs{
        let r=job.run(&site,closing,stopping);
        let _=job.reply.send(r);
    }
}
impl App<'_>{
    pub(super) fn volume_order(&mut self,mut q:Request,reply:mpsc::SyncSender<Reply>,now:Instant,until:Instant)
        ->Result<Work,(mpsc::SyncSender<Reply>,Reply)>{
        if q.method!="POST"||q.target!=TARGET||q.origin.as_deref()!=Some(q.audience.origin.as_str()){
            return Err((reply,REJECT()));
        }
        if q.content_type!="application/x-www-form-urlencoded"{return Err((reply,BAD()));}
        let Some([csrf,volume])=transport::fields(&mut q.body.0,["csrf","volume"])else{return Err((reply,BAD()));};
        let (Some(csrf),Ok(id))=(Token::parse(csrf),volume.parse::<VolumeId>())else{return Err((reply,BAD()));};
        self.expire(now);
        let Some(s)=q.cookie.as_ref().and_then(|key|self.sessions.get_mut(key))else{return Err((reply,REJECT()));};
        if s.audience!=q.audience||!s.csrf.matches(&csrf){return Err((reply,REJECT()));}
        if now>=until{return Err((reply,unconfirmed()));}
        s.last=now;
        Ok(Work{id,who:s.admin.manual_author(),session:s.permit.clone(),
            before:(s.born+super::ABSOLUTE).min(s.last+super::IDLE),until,reply})
    }
}

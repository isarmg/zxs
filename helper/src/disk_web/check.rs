//! An optional single terminal grant. Pairing on its own creates none.
//! This state belongs to one panel with exactly one non-renewable paired session.
use super::{Reply,matches,token};
use crate::fat32_check::panel::{Info,Operand,Receipt,Running};
use std::{fmt::Write,time::{Duration,Instant}};

pub(super) const WINDOW:Duration=Duration::from_secs(90);
enum Stage{Prepared(Operand),Running(Running),Done(Receipt),Incomplete}
struct Ticket{plain:String,issued:Instant}
impl Ticket{
    fn authorizes(&self,offered:&[u8],now:Instant,prepared:bool)->bool{
        matches(offered,&super::digest(self.plain.as_bytes())) &&
            (!prepared || now.saturating_duration_since(self.issued)<WINDOW)
    }
}
pub(super) struct Check{info:Info,stage:Stage,ticket:Option<Ticket>}
impl Check {
    pub(super) fn new(copy:Operand)->Self{Self{info:copy.info(),stage:Stage::Prepared(copy),ticket:None}}
    pub(super) fn preview(&mut self,now:Instant)->Reply{
        if self.ticket.is_none()||matches!(self.stage,Stage::Prepared(_))
            &&self.ticket.as_ref().is_some_and(|t|now.saturating_duration_since(t.issued)>=WINDOW){
            match token(){Ok(plain)=>self.ticket=Some(Ticket{plain,issued:now}),
                Err(_)=>return Reply::plain(503,"无法为本次副本签发预览票。")}
        }
        let Some(t)=&self.ticket else{return Reply::plain(503,"预览票不可用。");};
        let mut hex=String::with_capacity(64);for v in self.info.digest{write!(&mut hex,"{v:02x}").expect("String");}
        let state=if matches!(self.stage,Stage::Prepared(_)){"prepared"}else{"claimed"};
        // The ticket goes only in this authenticated response, never in URL, disk or logs.
        let (format,detail)=self.info.format_fields();
        let sector=self.info.sector.map_or_else(||"-".into(),|s|s.to_string());
        Reply::plain(200,format!("{}\n{hex}\n{format}\n{}\n{sector}\n{detail}\n{state}",t.plain,self.info.bytes))
    }
    pub(super) fn action(&mut self,submit:bool,offered:&[u8],now:Instant)->Reply{
        let Some(t)=self.ticket.as_ref()else{return Reply::plain(403,"请先预览这次由终端选定的副本。");};
        if !t.authorizes(offered,now,matches!(self.stage,Stage::Prepared(_))){
            return Reply::plain(403,"预览票无效或已过期，请重新核对。");
        }
        if matches!(self.stage,Stage::Prepared(_)){
            if !submit{return Reply::plain(200,"尚未开始检查。");}
            // Claim BEFORE any fallible spawn. Retrying this same ticket (including
            // when the browser lost the response) cannot create a second process.
            if let Stage::Prepared(image)=std::mem::replace(&mut self.stage,Stage::Incomplete){
                if let Ok(worker)=image.launch(){self.stage=Stage::Running(worker);}
            }
        }
        self.poll();
        match &self.stage{
            Stage::Prepared(_)=>Reply::plain(200,"尚未开始检查。"),
            Stage::Running(_)=>Reply::plain(200,"检查进行中（可能仍在核对完整摘要）；请点击“查看同次结果”，无需再次启动。"),
            Stage::Incomplete=>Reply::plain(503,"未取得完整检查结果。可能是副本、摘要、日志或租约未通过，工具/隔离不可用，或检查中断。不会自动重试。"),
            Stage::Done(r)=>Reply::plain(200,r.text()),
        }
    }
    pub(super) fn finish(self)->std::io::Result<()>{
        match self.stage{
            Stage::Prepared(_)|Stage::Done(_)=>Ok(()),
            Stage::Running(mut w)=>w.finish(),
            Stage::Incomplete=>Err(std::io::Error::other("check incomplete; no verified completion or drain receipt")),
        }
    }
    pub(super) fn poll(&mut self){
        if let Stage::Running(worker)=&mut self.stage{
            match worker.poll(){Ok(None)=>{},Ok(Some(r))=>self.stage=Stage::Done(r),Err(_)=>self.stage=Stage::Incomplete}
        }
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::fat32_check::{Outcome,Report,panel::Detail};
    #[test]
    fn preapproval_needs_this_exact_ticket_before_deadline_and_retries_return_the_same_receipt(){
        let now=Instant::now();let t=Ticket{plain:"a".repeat(64),issued:now};
        assert!(t.authorizes(&[b'a';64],now,true));assert!(!t.authorizes(&[b'a';64],now+WINDOW,true));
        assert!(!t.authorizes(&[b'b';64],now,true));assert!(!t.authorizes(&[b'a';32],now,true));
        let info=Info{bytes:34097152,sector:Some(512),detail:Detail::Fat32Clusters(65530),digest:[0x11;32]};
        let mut c=Check{info,stage:Stage::Done(Receipt::Fat32(Report{state:Outcome::ReportedMessages,files:2,used:3,total:65530})),ticket:Some(t)};
        assert_eq!(c.action(true,&[b'b';64],now).code,403);
        let original=c.action(false,&[b'a';64],now).body;assert!(original.contains("2；已用簇 / 总簇：3 / 65530"));
        assert_eq!(c.action(true,&[b'a';64],now+WINDOW).body,original);
        assert!(matches!(c.stage,Stage::Done(_)));
        c.stage=Stage::Incomplete;
        assert_eq!(c.action(true,&[b'a';64],now).code,503);assert!(matches!(c.stage,Stage::Incomplete));
    }

    #[test]
    fn large_ext_preview_contains_only_supplied_digest_length_and_fixed_format_not_fake_sector_geometry(){
        let now=Instant::now();
        let t=Ticket{plain:"a".repeat(64),issued:now};
        let info=Info{bytes:1<<44,sector:None,detail:Detail::Ext4,digest:[0x11;32]};
        let mut c=Check{info,stage:Stage::Incomplete,ticket:Some(t)};
        let answer=c.preview(now);
        assert_eq!(answer.code,200);
        assert_eq!(answer.body,format!("{}\n{}\next4\n17592186044416\n-\n-\nclaimed","a".repeat(64),"11".repeat(32)));
        c.info.detail=Detail::Xfs;
        assert_eq!(c.preview(now).body,answer.body.replace("\next4\n","\nXFS\n"));
        c.info.detail=Detail::Ntfs;
        assert_eq!(c.preview(now).body,answer.body.replace("\next4\n","\nNTFS\n"));
    }
}

//! Explicit terminal-only pool installation and original-operation resumption. Separate local pair enrollment; no remote requests or legacy-format fallback.
mod request;
mod engine;
mod originals;
mod approval;
mod reservation;
mod stage;
mod catalog;
mod markers;
mod complete;
mod receipt;
mod pair;
pub(crate) use pair::{InstallWebUnit,WebUnitControl, Setup,exec_child as sam_worker,exec_sql_child as native_sql_worker,exec_layout_sql_child as layout_sql_worker,exec_first_browser,Online,StartSmb};

pub(crate) use pair::{confirm_copy_repair,exec_copy_repair_browser};
pub(crate) use pair::RetirementCheck;

const DATABASE:&str="SYSTEM.sqlite3";
const MARKER:&[u8]=b".zxs-new-ffffffffffffffffffffffffffffffff-0";
fn bad(message:&'static str)->std::io::Error{std::io::Error::new(std::io::ErrorKind::InvalidData,message)}
fn unsupported(message:&'static str)->std::io::Error{std::io::Error::new(std::io::ErrorKind::Unsupported,message)}

use std::{ffi::OsString,io,path::PathBuf};
use request::Id;

#[derive(Clone,Copy,Debug)]
enum Purpose{Fresh,Original}
#[derive(Clone,Copy,Debug)]
enum InitAction{Apply(Purpose),Receipt}
#[derive(Debug)]
pub(crate) struct Installation{action:InitAction,browser:bool,op:Id,approval:Id,paths:Vec<PathBuf>}
fn id(s:OsString)->Option<Id>{
    let text=s.to_str()?;
    if text.len()!=32 || !text.bytes().all(|c|c.is_ascii_digit()||(b'a'..=b'f').contains(&c)){return None;}
    let mut out=[0u8;16];for(i,byte)in out.iter_mut().enumerate(){*byte=u8::from_str_radix(&text[i*2..i*2+2],16).ok()?;}
    (out!=[0;16]).then_some(out)
}
impl Installation{
    pub(crate) fn parse(mut args:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        let mut first=args.next().and_then(|s|s.into_string().ok());
        let browser=first.as_deref()==Some("web");
        if browser{first=args.next().and_then(|s|s.into_string().ok());}
        let action=match first.as_deref(){
            Some("new")=>InitAction::Apply(Purpose::Fresh),Some("resume")=>InitAction::Apply(Purpose::Original),
            Some("receipt") if !browser=>InitAction::Receipt,
            _=>return Err("choose new, resume or receipt explicitly; never automatically switch")};
        let mut tag=|label:&str|->Result<Id,&'static str>{
            if args.next().as_deref()!=Some(std::ffi::OsStr::new(label)){return Err("expected --op ID --approval ID");}
            args.next().and_then(id).ok_or("expected 32 lowercase nonzero hex digits")
        };
        let op=tag("--op")?;let approval=tag("--approval")?;drop(tag);
        let mut paths=Vec::new();
        for label in ["--system","--recycle"]{
            if args.next().as_deref()!=Some(std::ffi::OsStr::new(label)){return Err("expected explicit --system DIR --recycle DIR");}
            paths.push(PathBuf::from(args.next().ok_or("missing selected directory")?));
        }
        while let Some(flag)=args.next(){
            if flag!="--data"||paths.len()>=64{return Err("expected 1–62 explicitly selected --data DIR entries");}
            paths.push(PathBuf::from(args.next().ok_or("missing selected data directory")?));
        }
        if paths.len()<3{return Err("at least one ordinary data directory is required");}
        // The wire hashes F in raw byte order; neither path normalization nor a
        // local filename encoding is used. Request::new rejects duplicate/nested roots.
        paths[2..].sort_by(|a,b|a.as_os_str().as_encoded_bytes().cmp(b.as_os_str().as_encoded_bytes()));
        Ok(Self{action,browser,op,approval,paths})
    }
    pub(crate) fn run(self)->io::Result<(u8,String)>{
        let hex=|v:Id|v.iter().map(|b|format!("{b:02x}")).collect::<String>();
        match self.action{
            InitAction::Apply(purpose)=>{
                let proposal=approval::Proposal::select(purpose,self.op,self.approval,self.paths)?;
                let terminal=pair::PublicConfirmation::select()?;
                if self.browser{
                    pair::offer_init_web(&terminal,proposal.browser_scope(),||proposal.require())?;
                }
                let result=stage::install(proposal.authorize(terminal)?)?;
                Ok((0,format!("{} pool={} operation={}; neither network login nor current disk health is certified",
                    if result.previous{"original INIT was already committed (historical receipt only)"}else{"original INIT committed after all initial volume markers were synchronized"},
                    hex(result.pool),hex(result.op))))
            }
            InitAction::Receipt=>{
                let result=receipt::foreground(self.op,self.approval,self.paths)?;
                let (code,tag,meaning)=if result.committed{(0,"confirmed","historical receipt only")}
                    else{(1,"pending","not completed; this query did not resume or repair it")};
                Ok((code,format!("init={tag} pool={} operation={}; {meaning}; original SYSTEM only, recycle and data volumes not observed",
                    hex(result.pool),hex(self.op))))
            }
        }
    }
}

#[cfg(test)]
mod command_cases{
    use super::*;
    fn args(s:&str)->impl Iterator<Item=OsString>+'_ {s.split(' ').map(OsString::from)}
    #[test]fn selecting_a_new_or_existing_operation_is_explicit_and_binds_all_roles(){
        let fields="--op 01010101010101010101010101010101 --approval 02020202020202020202020202020202 --system /s --recycle /r --data /z --data /a";
        for (v,new) in [("new",true),("resume",false)]{
            let x=Installation::parse(args(&format!("{v} {fields}"))).unwrap();
            assert_eq!(matches!(x.action,InitAction::Apply(Purpose::Fresh)),new);
            assert_eq!(x.paths,[PathBuf::from("/s"),"/r".into(),"/a".into(),"/z".into()]);
        }
        let via=Installation::parse(args(&format!("web new {fields}"))).unwrap();
        assert!(via.browser&&matches!(via.action,InitAction::Apply(Purpose::Fresh)));
        assert!(Installation::parse(args(&format!("web receipt {fields}"))).is_err());
        let history=Installation::parse(args(&format!("receipt {fields}"))).unwrap();
        assert!(matches!(history.action,InitAction::Receipt));
        assert_eq!(history.paths,[PathBuf::from("/s"),"/r".into(),"/a".into(),"/z".into()]);
        for bad in [format!("auto {fields}"),fields.into(),format!("new {fields} --force"),format!("new {}",fields.replace("0101","0000"))]{
            assert!(Installation::parse(args(&bad)).is_err());
        }
    }
}

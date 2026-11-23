//! Explicit terminal-only pool installation and original-operation resumption. Separate local pair enrollment; no remote requests or legacy-format fallback.
pub(crate) use crate::product_layout;
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
#[cfg(feature="embedded-zxs")]
pub(crate) use pair::EmbeddedInstall;
#[cfg(test)]
mod discovery;
pub(crate) use pair::{RetirementCheck,ImportCheck,Setup,check_deployment,exec_layout_sql_child as layout_sql_worker,exec_first_browser,rust_account_worker,rust_vault_worker,SharedAccountReset};

const DATABASE:&str="SYSTEM.sqlite3";
const MARKER:&[u8]=b".zxs-new-ffffffffffffffffffffffffffffffff-0";
fn bad(message:&'static str)->std::io::Error{std::io::Error::new(std::io::ErrorKind::InvalidData,message)}
fn unsupported(message:&'static str)->std::io::Error{std::io::Error::new(std::io::ErrorKind::Unsupported,message)}

use std::{ffi::OsString,fmt::Write as _,io,path::PathBuf};
use request::Id;

#[derive(Clone,Copy,Debug)]
enum Purpose{Fresh,Original}
#[derive(Clone,Copy,Debug)]
enum InitAction{Apply(Purpose),Receipt}
#[derive(Debug)]
pub(crate) struct Installation{
    action:InitAction,browser:bool,op:Id,approval:Id,paths:Vec<PathBuf>,explicit:pair::ExplicitSubject,private:bool,
}
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
        let private=first.as_deref()==Some("new-deployment")&&cfg!(feature="private-fresh-install");
        let action=match first.as_deref(){
            Some("new-deployment") if private=>InitAction::Apply(Purpose::Fresh),
            Some("new")=>InitAction::Apply(Purpose::Fresh),Some("resume")=>InitAction::Apply(Purpose::Original),
            Some("receipt") if !browser=>InitAction::Receipt,
            _=>return Err("choose new, resume or receipt explicitly; never automatically switch")};
        let explicit=pair::ExplicitSubject::parse(&mut args)?;
        let mut tag=|label:&str|->Result<Id,&'static str>{
            if args.next().as_deref()!=Some(std::ffi::OsStr::new(label)){return Err("expected --op ID --approval ID");}
            args.next().and_then(id).ok_or("expected 32 lowercase nonzero hex digits")
        };
        let op=tag("--op")?;let approval=tag("--approval")?;drop(tag);
        let mut args=args.peekable();
        let mut paths=Vec::new();
        for (label,default) in [("--system",product_layout::SYSTEM),("--recycle",product_layout::RECYCLE)]{
            let path=if args.peek().is_some_and(|s|s==label){
                args.next();PathBuf::from(args.next().ok_or("missing selected directory")?)
            }else{PathBuf::from(default)};
            product_layout::media(&path)?;
            paths.push(path);
        }
        while let Some(flag)=args.next(){
            if paths.len()>=64{return Err("expected 1–62 explicitly selected --data DIR or --data-label LABEL entries");}
            let path=match flag.to_str(){
                Some("--data")=>PathBuf::from(args.next().ok_or("missing selected data directory")?),
                Some("--data-label")=>product_layout::data_label(args.next().ok_or("missing selected DATA label")?)?,
                _=>return Err("expected 1–62 explicitly selected --data DIR or --data-label LABEL entries"),
            };
            product_layout::media(&path)?;
            paths.push(path);
        }
        if paths.len()<3{return Err("at least one ordinary data directory is required");}
        // The wire hashes F in raw byte order; neither path normalization nor a
        // local filename encoding is used. Request::new rejects duplicate/nested roots.
        paths[2..].sort_by(|a,b|a.as_os_str().as_encoded_bytes().cmp(b.as_os_str().as_encoded_bytes()));
        Ok(Self{action,browser,op,approval,paths,explicit,private})
    }
    pub(crate) fn run(self)->io::Result<(u8,String)>{
        match self.action{
            InitAction::Apply(purpose)=>{
                let proposal=approval::Proposal::select(purpose,self.op,self.approval,self.paths,self.explicit,self.private)?;
                let terminal=pair::PublicConfirmation::select()?;
                if self.browser{
                    pair::offer_init_web(&terminal,proposal.browser_scope(),||proposal.require())?;
                }
                let result=stage::install(proposal.authorize(terminal)?)?;
                Ok((0,completion_line(result,self.private)))
            }
            InitAction::Receipt=>{
                let result=receipt::foreground(self.op,self.approval,self.paths,self.explicit)?;
                Ok(original_receipt_line(self.op,result))
            }
        }
    }
}

fn receipt_fields(pool:Id,op:Id,confirmed_sha:Option<[u8;32]>)->String{
    fn hex(v:&[u8])->String{v.iter().map(|b|format!("{b:02x}")).collect()}
    let mut fields=format!("pool={} operation={}",hex(&pool),hex(&op));
    if let Some(sha)=confirmed_sha{
        write!(fields," init_request_sha256={}",hex(&sha)).expect("writing to String");
    }
    fields
}
fn completion_line(result:complete::Receipt,private:bool)->String{
    let fields=receipt_fields(result.pool,result.op,Some(result.request_sha));
    if private{
        format!("fresh-init=confirmed {fields}; original volume markers verified")
    }else{
        let outcome=if result.previous{"original INIT was already committed (historical receipt only)"}
            else{"original INIT committed after all initial volume markers were synchronized"};
        format!("{outcome} {fields}")
    }
}
fn original_receipt_line(op:Id,result:receipt::Receipt)->(u8,String){
    let (code,tag,meaning)=if result.committed_sha.is_some(){(0,"confirmed","historical receipt only")}
        else{(1,"pending","not completed; this query did not resume or repair it")};
    let fields=receipt_fields(result.pool,op,result.committed_sha);
    (code,format!("init={tag} {fields}; {meaning}; original SYSTEM only"))
}

#[cfg(test)]
mod command_cases{
    use super::*;
    fn args(s:&str)->impl Iterator<Item=OsString>+'_ {s.split(' ').map(OsString::from)}
    #[test]
    fn same_init_digest_is_reported_on_completion_and_history_but_not_pending(){
        let (pool,op,sha)=([4;16],[7;16],[0xab;32]);
        let digest=format!(" init_request_sha256={}","ab".repeat(32));
        let applied=|previous|complete::Receipt{pool,op,request_sha:sha,previous};
        assert!(completion_line(applied(false),true).contains(&digest));
        assert!(completion_line(applied(false),false).contains(&digest));
        let resumed=completion_line(applied(true),false);
        assert!(resumed.contains(&digest)&&resumed.contains("historical receipt only"));
        let (code,confirmed)=original_receipt_line(op,receipt::Receipt{pool,committed_sha:Some(sha)});
        assert_eq!(code,0);
        assert!(confirmed.contains(&digest)&&confirmed.starts_with("init=confirmed "));
        let (code,pending)=original_receipt_line(op,receipt::Receipt{pool,committed_sha:None});
        assert_eq!(code,1);
        assert!(pending.starts_with("init=pending ")&&!pending.contains("init_request_sha256"));
    }
    #[test]
    fn fresh_deployment_init_is_explicit_in_both_handoff_modes(){
        let d=|v:&str|v.repeat(16);
        let t=format!("new-deployment --init-actor {} --storage-uid 1001 --storage-gid 1002 --op {} --approval {} --system /s --recycle /r --data /f",d("31"),d("32"),d("33"));
        let retired=t.replacen("new-deployment","new-zxs-v18",1);
        assert!(Installation::parse(retired.split_ascii_whitespace().map(OsString::from)).is_err());
        if cfg!(feature="private-fresh-install"){
            let v=Installation::parse(args(&t)).unwrap();
            assert!(v.private&&!v.browser&&matches!(v.action,InitAction::Apply(Purpose::Fresh)));
        }else{assert!(Installation::parse(args(&t)).is_err());}
        let via_web=format!("web {t}");
        let guided=Installation::parse(via_web.split_ascii_whitespace().map(OsString::from));
        if cfg!(feature="private-fresh-install"){
            let selected=guided.unwrap();assert!(selected.browser&&selected.private);
        }else{assert!(guided.is_err());}
        for b in [t.replace("1001","0"),t.replace("1002","65534"),format!("{t} --resume")]{
            assert!(Installation::parse(args(&b)).is_err());
        }
    }

    #[test]
    fn layout_v18_init_resolves_only_known_defaults_and_explicit_data_labels(){
        let fields="--init-actor 03030303030303030303030303030303 --storage-uid 1001 --storage-gid 1002 --op 01010101010101010101010101010101 --approval 02020202020202020202020202020202";
        for command in ["new","resume","receipt","web new","web resume"]{
            let line=|tail:&str|format!("{command} {fields} {tail}");
            let x=Installation::parse(args(&line("--data-label Z --data /approved/foreign --data-label A"))).unwrap();
            assert_eq!(x.paths,["/zxs/disks/system","/zxs/disks/recycle","/approved/foreign","/zxs/disks/data/A","/zxs/disks/data/Z"].map(PathBuf::from));
            let x=Installation::parse(args(&line("--recycle /explicit/r --data-label A"))).unwrap();
            assert_eq!(x.paths,["/zxs/disks/system","/explicit/r","/zxs/disks/data/A"].map(PathBuf::from));
            let x=Installation::parse(args(&line("--system /explicit/s --data-label A"))).unwrap();
            assert_eq!(x.paths,["/explicit/s","/zxs/disks/recycle","/zxs/disks/data/A"].map(PathBuf::from));
            for tail in ["","--data-label ..","--data-label a/b","--data /zxs/disks/data","--system /zxs/state --data-label A",
                "--recycle /zxs/log/other --data-label A","--data /zxs/bin","--disks /zxs/disks"]{
                assert!(Installation::parse(args(line(tail).trim_end())).is_err(),"{tail}");
            }
        }
    }

    #[test]fn selecting_a_new_or_existing_operation_is_explicit_and_binds_all_roles(){
        let fields="--init-actor 03030303030303030303030303030303 --storage-uid 1001 --storage-gid 1002 --op 01010101010101010101010101010101 --approval 02020202020202020202020202020202 --system /s --recycle /r --data /z --data /a";
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

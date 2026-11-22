//! One read-only H04 request on the existing layout observer process. No
//! runtime site, account data, ACT, write grant or reusable retirement ticket.
use super::{Channel,Id,Kind,MAX_PATH_BYTES,PathKind,Pinned,PublicConfirmation,approval,place,query_with,random,require};
use crate::sqlite_install::{bad,id,originals::LockedSystem,pair::{retire_ordinary::{self,Preflight,Scope},ExplicitSubject}};
use rustix::process::{Pid,getppid};
use std::{ffi::{OsStr,OsString},fmt::Write,io,os::unix::ffi::{OsStrExt,OsStringExt},path::{Path,PathBuf}};

pub(super) const DOMAIN:&[u8]=b"helper ordinary retirement check exact v18 1\0";
const BUDGET:usize=DOMAIN.len()+32+4*16+8+3*8+2*(2+MAX_PATH_BYTES);
const PUBLICATION:u16=1<<0;
const SELECTED_FILE:u16=1<<1;
const DIRECTORY:u16=1<<2;
const FILE_BIRTH:u16=1<<3;
const RENAME:u16=1<<4;
const TRANSFER:u16=1<<5;
const PENDING:u16=1<<6;
const CLEANUP:u16=1<<7;
const CLEANUP_UNKNOWN:u16=1<<8;
const ONLY_ORDINARY:u16=1<<9;
const EXTRA:u16=1<<10;
const ALL:u16=(1<<11)-1;
const NO_CHANGE:&str="仅只读预检；没有退登记、修改、清除或卸载。";
fn invalid()->io::Error{bad("different or incomplete original ordinary-volume preflight scope")}

#[derive(Debug)]
pub(crate) struct RetirementCheck(Scope,ExplicitSubject);
impl RetirementCheck{
    pub(crate) fn parse(mut args:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        fn field(args:&mut impl Iterator<Item=OsString>,key:&str)->Result<OsString,&'static str>{
            if args.next().as_deref()!=Some(OsStr::new(key)){return Err("expected the displayed read-only fields in order");}
            args.next().ok_or("missing read-only preflight value")
        }
        fn key(args:&mut impl Iterator<Item=OsString>,name:&str)->Result<Id,&'static str>{
            id(field(args,name)?).ok_or("expected 32 nonzero lowercase hexadecimal identifier digits")
        }
        let subject=ExplicitSubject::parse(&mut args)?;
        let system=PathBuf::from(field(&mut args,"--system")?);
        let pool=key(&mut args,"--pool")?;let init=key(&mut args,"--init")?;let volume=key(&mut args,"--volume")?;
        let ordinary=PathBuf::from(field(&mut args,"--data")?);
        if args.next().is_some(){return Err("read-only preflight has no options to change or apply anything");}
        Scope::new(pool,init,volume,system,ordinary).map(|s|Self(s,subject))
            .map_err(|_|"invalid original SYSTEM or distinct ordinary path/identity for read-only preflight")
    }
    fn query(&self)->io::Result<Reply>{
        retire_ordinary::compiled_schema()?;
        approval::no_inherited_descriptors()?;
        let _host=approval::host_admin()?;let tty=PublicConfirmation::select()?;let subject=self.1.tuple();
        let pin=Pinned::already_mounted(self.0.system(),PathKind::StorageRoot)?;
        let original=pin.storage_directory()?;
        rustix::fs::flock(&original,rustix::fs::FlockOperation::NonBlockingLockExclusive)?;
        let check=||{tty.check()?;require(&pin,&original,subject)};
        check()?;
        let raw=encode(&self.0,&random::<32>()?,self.1,place(&pin)?)?;
        query_with(&raw,&original,subject,None,check,|channel,digest,until|{
            let (body,[])=channel.wait_checked::<0>(Kind::RetirementCheck,digest,until,&check)?;
            Reply::decode(&body,self.0.ids())
        })
    }
    pub(crate) fn run(self)->(u8,String){
        let report=self.query().unwrap_or(Reply::Unknown);
        report.render(self.0.ids())
    }
}

pub(super) struct Question{subject:ExplicitSubject,location:[u64;3],scope:Scope}
fn encode(scope:&Scope,nonce:&[u8;32],subject:ExplicitSubject,location:[u64;3])->io::Result<Vec<u8>>{
    let (actor,uid,gid)=subject.tuple();
    if nonce==&[0;32]||location[1..].contains(&0){return Err(invalid());}
    let mut raw=DOMAIN.to_vec();raw.extend(nonce);raw.extend(actor);raw.extend(uid.to_be_bytes());raw.extend(gid.to_be_bytes());
    for id in scope.ids(){raw.extend(id);}
    for n in location{raw.extend(n.to_be_bytes());}
    for path in [scope.system(),scope.ordinary()]{
        let v=path.as_os_str().as_bytes();PathKind::StorageRoot.check_path(v).map_err(bad)?;
        raw.extend((v.len() as u16).to_be_bytes());raw.extend(v);
    }
    if raw.len()>BUDGET{return Err(invalid());}Ok(raw)
}
impl Question{
    pub(super) fn decode(raw:&[u8])->io::Result<Self>{
        fn take<'a>(b:&mut &'a [u8],n:usize)->io::Result<&'a [u8]>{
            let (v,more)=b.split_at_checked(n).ok_or_else(invalid)?;*b=more;Ok(v)
        }
        fn fixed<const N:usize>(b:&mut &[u8])->io::Result<[u8;N]>{Ok(take(b,N)?.try_into().expect("fixed bytes"))}
        fn path(b:&mut &[u8])->io::Result<PathBuf>{
            let n=u16::from_be_bytes(fixed(b)?) as usize;
            if n==0||n>MAX_PATH_BYTES{return Err(invalid());}
            Ok(PathBuf::from(OsString::from_vec(take(b,n)?.to_vec())))
        }
        if raw.len()>BUDGET{return Err(invalid());}
        let mut bytes=raw.strip_prefix(DOMAIN).ok_or_else(invalid)?;
        let nonce=fixed(&mut bytes)?;let actor=fixed(&mut bytes)?;
        let subject=ExplicitSubject::checked(actor,u32::from_be_bytes(fixed(&mut bytes)?),u32::from_be_bytes(fixed(&mut bytes)?))?;
        let pool=fixed(&mut bytes)?;let init=fixed(&mut bytes)?;let volume=fixed(&mut bytes)?;
        let location=[u64::from_be_bytes(fixed(&mut bytes)?),u64::from_be_bytes(fixed(&mut bytes)?),u64::from_be_bytes(fixed(&mut bytes)?)];
        let system=path(&mut bytes)?;let ordinary=path(&mut bytes)?;
        let scope=Scope::new(pool,init,volume,system,ordinary)?;
        if !bytes.is_empty()||encode(&scope,&nonce,subject,location)?.as_slice()!=raw{return Err(invalid());}
        retire_ordinary::compiled_schema()?;
        Ok(Self{subject,location,scope})
    }
    pub(super) fn original_system(&self)->(ExplicitSubject,[u64;3],&Path){(self.subject,self.location,self.scope.system())}
    pub(super) fn inspect(&self,locked:&LockedSystem,channel:&Channel,digest:&[u8;32],parent:Pid)->io::Result<()>{
        let send=|reply:Reply|{
            if getppid()!=Some(parent){return Err(invalid());}
            channel.send(Kind::RetirementCheck,digest,&reply.encode(self.scope.ids()))
        };
        // The success callback runs while both originals are held, outside
        // SQLite. An uncertain send must not generate a second packet.
        match retire_ordinary::observe(locked,&self.scope,&self.subject.tuple().0,|snapshot|send(Reply::from(snapshot,self.scope.ids())?)){
            Ok(sent)=>sent,
            Err(e)=>{
                let status=if retire_ordinary::protected_volume(&e){Reply::Protected}else{Reply::Unknown};
                send(status)
            }
        }
    }
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Reply{Observed{ordinal:u8,blockers:u16},Protected,Unknown}
impl Reply{
    fn from(p:Preflight,[_,init,volume]:[Id;3])->io::Result<Self>{
        if p.original_init!=init||p.volume!=volume||!(2..=63).contains(&p.ordinal){return Err(invalid());}
        let w=&p.witnesses;
        let blockers=[(w.publication,PUBLICATION),(w.selected_file,SELECTED_FILE),(w.directory_origin,DIRECTORY),
            (w.file_birth,FILE_BIRTH),(w.file_rename_history,RENAME),(w.transfer_history,TRANSFER),
            (w.pending_transfer,PENDING),(w.armed_cleanup,CLEANUP),(w.unproved_cleanup,CLEANUP_UNKNOWN),
            (p.only_ordinary,ONLY_ORDINARY),(p.extra_entry,EXTRA)].into_iter()
            .fold(0,|bits,(present,flag)|bits|if present{flag}else{0});
        if (blockers==0)!=p.no_observed_blockers(){return Err(invalid());}
        Ok(Self::Observed{ordinal:p.ordinal,blockers})
    }
    fn encode(self,[pool,init,volume]:[Id;3])->[u8;65]{
        let mut b=[0;65];
        b[4..20].copy_from_slice(&pool);b[20..36].copy_from_slice(&init);b[36..52].copy_from_slice(&volume);
        match self{
            Self::Observed{ordinal,blockers}=>{b[0]=1;b[1]=ordinal;b[2..4].copy_from_slice(&blockers.to_be_bytes());}
            Self::Protected=>b[0]=2,Self::Unknown=>b[0]=3,
        }b
    }
    fn decode(b:&[u8;65],[pool,init,volume]:[Id;3])->io::Result<Self>{
        if b[4..20]!=pool||b[20..36]!=init||b[36..52]!=volume||b[52..]!=[0;13]{return Err(invalid());}
        let blockers=u16::from_be_bytes(b[2..4].try_into().expect("two result bytes"));
        match b[0]{
            1 if (2..=63).contains(&b[1])&&blockers&!ALL==0&&
                (blockers&(PENDING|CLEANUP|CLEANUP_UNKNOWN)==0||blockers&TRANSFER!=0)=>Ok(Self::Observed{ordinal:b[1],blockers}),
            2 if b[1]==0&&blockers==0=>Ok(Self::Protected),
            3 if b[1]==0&&blockers==0=>Ok(Self::Unknown),
            _=>Err(invalid()),
        }
    }
    fn render(self,[pool,init,volume]:[Id;3])->(u8,String){
        let hex=|v:Id|v.iter().map(|b|format!("{b:02x}")).collect::<String>();
        let mut text=format!("池 {}，请求检查卷 {}；所给历史 INIT {} 不是退盘凭据。\n",hex(pool),hex(volume),hex(init));
        let code=match self{
            Self::Unknown=>{text.push_str("未确认：没有得到完整的原池、原卷和记录证明。\n");2}
            Self::Protected=>{text.push_str("存在阻挡：SYSTEM 和唯一回收卷不在退登记范围内。\n");1}
            Self::Observed{ordinal,blockers:0}=>{
                writeln!(text,"原普通卷位置 {ordinal}：本次未见阻挡。不表示现在可以物理移除。")
                    .expect("writing String");0
            }
            Self::Observed{ordinal,blockers}=>{
                writeln!(text,"原普通卷位置 {ordinal}：存在阻挡。").expect("writing String");
                for (bit,fact) in [(PUBLICATION,"有已登记的共享发布（停用也不解除登记）"),
                    (SELECTED_FILE,"有当前 FILE 位置"),(DIRECTORY,"有目录声明或历史"),(FILE_BIRTH,"有 FILE 出生历史"),
                    (RENAME,"有 FILE 改名历史"),(TRANSFER,"有转移源或目标历史"),(PENDING,"还有待决转移"),
                    (CLEANUP,"有尚待清理的已确认转移"),(CLEANUP_UNKNOWN,"有未能证明完成的转移清理"),
                    (ONLY_ORDINARY,"这是最后一个普通卷"),(EXTRA,"在原卷标之外观察到目录条目")]{
                    if blockers&bit!=0{writeln!(text,"  - {fact}。").expect("writing String");}
                }1
            }
        };
        text.push_str(NO_CHANGE);(code,text)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    fn scope()->Scope{Scope::new([1;16],[2;16],[3;16],"/roots/S".into(),
        OsString::from_vec(b"/roots/F-\xff".to_vec()).into()).unwrap()}
    #[test]
    fn exact_private_question_preserves_raw_f_and_rejects_a_forged_domain_or_tail(){
        let s=scope();let subject=ExplicitSubject::checked([5;16],1001,1002).unwrap();
        let wire=encode(&s,&[4;32],subject,[0,6,7]).unwrap();
        let q=Question::decode(&wire).unwrap();
        assert_eq!(q.original_system(),(subject,[0,6,7],Path::new("/roots/S")));
        assert_eq!(q.scope.ordinary().as_os_str().as_bytes(),b"/roots/F-\xff");
        assert_eq!(q.scope.ids(),s.ids());
        assert!(Question::decode(&wire[..wire.len()-1]).is_err());
        let mut extra=wire.clone();extra.push(0);assert!(Question::decode(&extra).is_err());
        let mut foreign=wire;foreign[0]^=1;assert!(Question::decode(&foreign).is_err());
    }
    #[test]
    fn typed_results_never_accept_other_scope_impossible_flags_or_nonzero_reserved_bytes(){
        let ids=scope().ids();let observed=Reply::Observed{ordinal:3,blockers:TRANSFER|CLEANUP_UNKNOWN};
        let raw=observed.encode(ids);assert_eq!(Reply::decode(&raw,ids).unwrap(),observed);
        assert!(Reply::decode(&raw,[[1;16],[2;16],[9;16]]).is_err());
        assert!(Reply::decode(&Reply::Observed{ordinal:1,blockers:0}.encode(ids),ids).is_err());
        assert!(Reply::decode(&Reply::Observed{ordinal:2,blockers:PENDING}.encode(ids),ids).is_err());
        assert!(Reply::decode(&Reply::Observed{ordinal:2,blockers:1<<12}.encode(ids),ids).is_err());
        let mut extra=Reply::Protected.encode(ids);extra[64]=1;
        assert!(Reply::decode(&extra,ids).is_err());extra[64]=0;extra[1]=2;
        assert!(Reply::decode(&extra,ids).is_err());
        let (code,short)=Reply::Observed{ordinal:2,blockers:0}.render(ids);
        assert_eq!(code,0);assert!(short.contains("不表示现在可以物理移除")&&short.ends_with(NO_CHANGE));
        assert_eq!(observed.render(ids).0,1);assert_eq!(Reply::Unknown.render(ids).0,2);
    }
}

//! Fixed, read-only argument and one result for the existing lowered worker.
use super::{Channel,Id,Kind,MAX_PATH_BYTES,PathKind,Pinned,PublicConfirmation,approval,place,query_with,random,require};
use crate::{maintenance::{DiscoveryView,HostMounts},sqlite_install::{bad,id,originals::LockedSystem,
    pair::{import_source::{self,Counts,Finding,Target,TargetFacts},retire_ordinary,ExplicitSubject}}};
use rustix::{fs::{FlockOperation,flock},process::{Pid,getppid}};
use std::{ffi::{OsStr,OsString},fmt::Write,io,os::unix::ffi::{OsStrExt,OsStringExt},path::{Path,PathBuf},sync::Arc};

pub(super) const DOMAIN:&[u8]=b"helper nonempty source inventory exact v18 1\0";
const HEAD:usize=32+3*16+8+6*8;
const TARGET:&[u8]=b"target F\0";
const BUDGET:usize=DOMAIN.len()+HEAD+3*(2+MAX_PATH_BYTES)+TARGET.len()+16+3*8;
const ONLY_READ:&str="仅源目录元数据清点；未读源文件正文、未确定目标与空间，也没有导入、写入或移除。";
const PLAN_ONLY:&str="仅作计划；未读源文件正文，未复制、预留空间、写入数据或修改登记。不能据此执行导入。";
fn invalid()->io::Error{bad("not this exact original source inventory selection")}
pub(super) fn mount_scope()->io::Result<Arc<DiscoveryView>>{
    let scope=Arc::new(HostMounts::discovery_view(false)?);
    Pinned::require_local_root(&scope)?;
    Ok(scope)
}

#[derive(Debug)]
pub(crate) struct ImportCheck{
    system:PathBuf,source:PathBuf,pool:Id,init:Id,subject:ExplicitSubject,target:Option<Target>,
}
impl ImportCheck{
    pub(crate) fn parse(args:impl Iterator<Item=OsString>)->Result<Self,&'static str>{Self::selected(args,false)}
    pub(crate) fn parse_plan(args:impl Iterator<Item=OsString>)->Result<Self,&'static str>{Self::selected(args,true)}
    fn selected(mut args:impl Iterator<Item=OsString>,plan:bool)->Result<Self,&'static str>{
        fn field(args:&mut impl Iterator<Item=OsString>,label:&str)->Result<OsString,&'static str>{
            if args.next().as_deref()!=Some(OsStr::new(label)){return Err("expected the displayed read-only fields in order");}
            args.next().ok_or("missing source inventory field")
        }
        let subject=ExplicitSubject::parse(&mut args)?;
        let system=PathBuf::from(field(&mut args,"--system")?);
        let pool=id(field(&mut args,"--pool")?).ok_or("expected a 32-digit nonzero lowercase PoolId")?;
        let init=id(field(&mut args,"--init")?).ok_or("expected a 32-digit nonzero lowercase original INIT Op")?;
        let source=PathBuf::from(field(&mut args,"--source")?);
        let target=if plan{
            let volume=id(field(&mut args,"--target-volume")?).ok_or("expected an exact nonzero original ordinary VolumeId")?;
            Some(Target{volume,path:PathBuf::from(field(&mut args,"--target")?)})
        }else{None};
        if args.next().is_some(){return Err(if plan{"no additional destination, override, or action options"}
            else{"source inventory has no destination, override, or action options"});}
        let q=Self{system,source,pool,init,subject,target};
        q.validate().map_err(|_|"expected bounded original directories and selected original IDs")?;
        Ok(q)
    }
    fn validate(&self)->io::Result<()>{
        for p in [&self.system,&self.source]{PathKind::StorageRoot.check_path(p.as_os_str().as_bytes()).map_err(bad)?;}
        if self.pool==[0;16]||self.init==[0;16]||self.pool==self.init||self.system.to_str().is_none(){return Err(invalid());}
        if let Some(t)=&self.target{
            PathKind::StorageRoot.check_path(t.path.as_os_str().as_bytes()).map_err(bad)?;
            if [[0;16],self.pool].contains(&t.volume)||[&self.system,&self.source].iter()
                .any(|p|p.starts_with(&t.path)||t.path.starts_with(p)){return Err(invalid());}
        }
        Ok(())
    }
    fn query(&self)->io::Result<Reply>{
        retire_ordinary::compiled_schema()?;
        approval::no_inherited_descriptors()?;
        let scope=mount_scope()?;
        let _host=approval::host_admin()?;
        let tty=PublicConfirmation::select()?;
        let actor=self.subject.tuple();
        let system=Pinned::local_storage(&self.system,&scope)?;
        let original=system.storage_directory()?;
        flock(&original,FlockOperation::NonBlockingLockExclusive)?;
        let source=Pinned::local_storage(&self.source,&scope)?;
        let location=place(&source)?;
        let target=self.target.as_ref().map(|t|->io::Result<_>{
            let pin=Pinned::local_storage(&t.path,&scope)?;let at=place(&pin)?;Ok((pin,at))
        }).transpose()?;
        let checked=||{
            tty.check()?;require(&system,&original,actor)?;
            if place(&source)?!=location{return Err(invalid());}
            if let Some((pin,at))=&target&&place(pin)?!=*at{return Err(invalid());}
            Ok(())
        };
        checked()?;
        let raw=encode(self,&random::<32>()?,place(&system)?,location,target.as_ref().map(|(_,at)|*at))?;
        let binding=self.target.as_ref().zip(target.as_ref()).map(|(choice,(_,at))|(choice.volume,at[2]));
        let found=query_with(&raw,&original,actor,None,checked,|channel,digest,until|{
            let (body,[])=channel.wait_checked::<0>(Kind::ImportInventory,digest,until,&checked)?;
            Reply::decode(&body,self.pool,self.init,location[2],binding)
        })?;
        scope.require_unchanged()?;checked()?;Ok(found)
    }
    pub(crate) fn run(self)->(u8,String){self.query().unwrap_or(Reply::Unknown).render(self.pool,self.init,self.target.map(|t|t.volume))}
}

pub(super) struct Question{q:ImportCheck,system:[u64;3],source:[u64;3],target:Option<[u64;3]>}
fn encode(q:&ImportCheck,nonce:&[u8;32],system:[u64;3],source:[u64;3],target:Option<[u64;3]>)->io::Result<Vec<u8>>{
    fn name(b:&mut Vec<u8>,p:&Path){let raw=p.as_os_str().as_bytes();b.extend((raw.len() as u16).to_be_bytes());b.extend(raw);}
    q.validate()?;
    if nonce==&[0;32]||system[1..].contains(&0)||source[1..].contains(&0){return Err(invalid());}
    let (actor,uid,gid)=q.subject.tuple();
    let mut b=DOMAIN.to_vec();b.extend(nonce);b.extend(actor);b.extend(uid.to_be_bytes());b.extend(gid.to_be_bytes());
    b.extend(q.pool);b.extend(q.init);
    for n in system.into_iter().chain(source){b.extend(n.to_be_bytes());}
    for p in [&q.system,&q.source]{name(&mut b,p);}
    match (&q.target,target){
        (Some(t),Some(at)) if !at[1..].contains(&0)=>{
            b.extend(TARGET);b.extend(t.volume);
            for value in at{b.extend(value.to_be_bytes());}name(&mut b,&t.path);
        }
        (None,None)=>(),
        _=>return Err(invalid()),
    }
    if b.len()>BUDGET{return Err(invalid());}Ok(b)
}
impl Question{
    pub(super) fn decode(raw:&[u8])->io::Result<Self>{
        fn take<'a>(b:&mut &'a [u8],n:usize)->io::Result<&'a [u8]>{
            let (one,rest)=b.split_at_checked(n).ok_or_else(invalid)?;*b=rest;Ok(one)
        }
        fn bytes<const N:usize>(b:&mut &[u8])->io::Result<[u8;N]>{Ok(take(b,N)?.try_into().expect("fixed bytes"))}
        fn location(b:&mut &[u8])->io::Result<[u64;3]>{
            Ok([u64::from_be_bytes(bytes(b)?),u64::from_be_bytes(bytes(b)?),u64::from_be_bytes(bytes(b)?)])
        }
        fn path(b:&mut &[u8])->io::Result<PathBuf>{
            let n=u16::from_be_bytes(bytes(b)?) as usize;
            if n==0||n>MAX_PATH_BYTES{return Err(invalid());}
            Ok(PathBuf::from(OsString::from_vec(take(b,n)?.to_vec())))
        }
        if raw.len()>BUDGET{return Err(invalid());}
        let mut b=raw.strip_prefix(DOMAIN).ok_or_else(invalid)?;
        let nonce=bytes(&mut b)?;let actor=bytes(&mut b)?;
        let subject=ExplicitSubject::checked(actor,u32::from_be_bytes(bytes(&mut b)?),u32::from_be_bytes(bytes(&mut b)?))?;
        let pool=bytes(&mut b)?;let init=bytes(&mut b)?;
        let system=location(&mut b)?;let source=location(&mut b)?;
        let mut q=ImportCheck{system:path(&mut b)?,source:path(&mut b)?,pool,init,subject,target:None};
        let target=if b.is_empty(){None}else{
            b=b.strip_prefix(TARGET).ok_or_else(invalid)?;
            let volume=bytes(&mut b)?;let at=location(&mut b)?;
            q.target=Some(Target{volume,path:path(&mut b)?});Some(at)
        };
        if !b.is_empty()||encode(&q,&nonce,system,source,target)?.as_slice()!=raw{return Err(invalid());}
        retire_ordinary::compiled_schema()?;
        Ok(Self{q,system,source,target})
    }
    pub(super) fn original_system(&self)->(ExplicitSubject,[u64;3],&Path){(self.q.subject,self.system,&self.q.system)}
    pub(super) fn inspect(&self,locked:&LockedSystem,channel:&Channel,digest:&[u8;32],parent:Pid)->io::Result<()>{
        let send=|report:Reply|{
            if getppid()!=Some(parent){return Err(invalid());}
            channel.send(Kind::ImportInventory,digest,&report.encode(self.q.pool,self.q.init))
        };
        // Only one report; a failed send cannot become a second observation.
        let actor=self.q.subject.tuple().0;
        let target=self.q.target.as_ref().zip(self.target);
        let selected=import_source::Selection{pool:&self.q.pool,init:&self.q.init,actor:&actor,path:&self.q.source,location:self.source,target};
        match import_source::observe(locked,selected,|finding|
            send(match finding{
                Finding::Blocked(bits)=>Reply::Blocked(bits),
                Finding::Inventoried(counts)=>Reply::Inventoried(counts,self.source[2]),
                Finding::Planned(counts,facts)=>target.map_or(Reply::Unknown,|(t,at)|
                    Reply::Planned(counts,facts,t.volume,self.source[2],at[2])),
            })){
            Ok(sent)=>sent,
            Err(_)=>send(Reply::Unknown),
        }
    }
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Reply{Inventoried(Counts,u64),Blocked(u16),Unknown,Planned(Counts,TargetFacts,Id,u64,u64)}
impl Reply{
    fn write_counts(b:&mut[u8;65],c:Counts){
        b[3..5].copy_from_slice(&c.files.to_be_bytes());b[5..7].copy_from_slice(&c.dirs.to_be_bytes());
        b[7..9].copy_from_slice(&c.empty_dirs.to_be_bytes());b[9..25].copy_from_slice(&c.logical_bytes.to_be_bytes());
    }
    fn encode(self,pool:Id,init:Id)->[u8;65]{
        let mut b=[0;65];b[33..49].copy_from_slice(&pool);b[49..65].copy_from_slice(&init);
        match self{
            Self::Inventoried(c,mount)=>{
                b[0]=1;Self::write_counts(&mut b,c);
                b[25..33].copy_from_slice(&mount.to_be_bytes());
            }
            Self::Blocked(bits)=>{b[0]=2;b[1..3].copy_from_slice(&bits.to_be_bytes());}
            Self::Unknown=>b[0]=3,
            Self::Planned(c,f,volume,source_mount,target_mount)=>{
                b[0]=4;b[1]=f.blockers;b[2]=f.limit;Self::write_counts(&mut b,c);
                b[25..33].copy_from_slice(&f.available.to_be_bytes());b[33..49].copy_from_slice(&volume);
                b[49..57].copy_from_slice(&source_mount.to_be_bytes());b[57..65].copy_from_slice(&target_mount.to_be_bytes());
            }
        }b
    }
    fn decode(b:&[u8;65],pool:Id,init:Id,original_mount:u64,target:Option<(Id,u64)>)->io::Result<Self>{
        if original_mount==0{return Err(invalid());}
        let read=|i:usize|u16::from_be_bytes(b[i..i+2].try_into().expect("fixed counters"));
        let word=|i:usize|u64::from_be_bytes(b[i..i+8].try_into().expect("fixed observation"));
        let counts=||Counts{files:read(3),dirs:read(5),empty_dirs:read(7),
            logical_bytes:u128::from_be_bytes(b[9..25].try_into().expect("fixed logical length"))};
        // Success for a plan must bind this exact explicitly selected F and
        // both original mount IDs; the sealed manifest also binds Pool/INIT.
        if b[0]==4{
            let (volume,mount)=target.ok_or_else(invalid)?;
            let f=TargetFacts{available:word(25),limit:b[2],blockers:b[1]};let c=counts();
            if mount==0||b[33..49]!=volume||word(49)!=original_mount||word(57)!=mount||!c.valid()||!f.valid(){return Err(invalid());}
            return Ok(Self::Planned(c,f,volume,original_mount,mount));
        }
        if b[33..49]!=pool||b[49..65]!=init{return Err(invalid());}
        let bits=read(1);
        match b[0]{
            1 if bits==0&&target.is_none()=>{
                let c=counts();let mount=word(25);
                if !c.valid()||mount!=original_mount{return Err(invalid());}
                Ok(Self::Inventoried(c,mount))
            }
            2 if b[3..33]==[0;30]&&bits!=0&&bits&!import_source::KNOWN==0
                && (bits&(import_source::MANAGED|import_source::EMPTY)==0
                    ||matches!(bits,import_source::MANAGED|import_source::EMPTY))=>Ok(Self::Blocked(bits)),
            3 if b[1..33]==[0;32]=>Ok(Self::Unknown),
            _=>Err(invalid()),
        }
    }
    fn render(self,pool:Id,init:Id,target:Option<Id>)->(u8,String){
        let hex=|v:Id|v.iter().map(|b|format!("{b:02x}")).collect::<String>();
        let mut text=format!("池 {}，已指定的历史 INIT {}（不是导入凭据）。\n",hex(pool),hex(init));
        if let Some(volume)=target{writeln!(text,"所选普通目标卷：{}。",hex(volume)).expect("String");}
        let code=match self{
            Self::Inventoried(c,mount)=>{
                writeln!(text,"本次源元数据清点完成（当次挂载 ID {mount}）：普通文件 {} 个，子目录 {} 个（空目录 {} 个），逻辑长度 {} 字节。不是导入可行证明。",
                    c.files,c.dirs,c.empty_dirs,c.logical_bytes).expect("String");0
            }
            Self::Blocked(bits)=>{
                use import_source::{MANAGED,CONTROL,BOUNDARY,NON_PLAIN,HARD_LINK,NAME,PERMISSION,EMPTY};
                text.push_str("存在阻挡；不提供部分清单。\n");
                for (flag,msg) in [(MANAGED,"与原池已登记的 SYSTEM、回收根或普通根重叠"),(CONTROL,"观察到疑似旧控制名或卷归属属性"),
                    (BOUNDARY,"有超出同一文件系统/挂载范围的条目"),(NON_PLAIN,"有符号链接或非普通条目"),
                    (HARD_LINK,"有多硬链接的文件"),(NAME,"有不符合当前普通对象命名范围的文字"),
                    (PERMISSION,"原低权限进程没有某普通文件的读权限"),(EMPTY,"所选根为空，不属于非空导入") ]{
                    if bits&flag!=0{writeln!(text,"  - {msg}。").expect("String");}
                }1
            }
            Self::Unknown=>{
                text.push_str(if target.is_some(){"未确认：请检查原池与源目录、目标是否为同一 INIT 的已挂载空 F、以及访问权限。\n"}
                    else{"未确认：无法完整核实本次原池、原目录、权限或有界清点；不能据此导入。\n"});2
            }
            Self::Planned(c,f,_,src,dst)=>{
                writeln!(text,"源挂载 ID {src}：普通文件 {} 个，子目录 {} 个（空目录 {} 个），逻辑长度 {} 字节。",
                    c.files,c.dirs,c.empty_dirs,c.logical_bytes).expect("String");
                writeln!(text,"目标挂载 ID {dst}：目前只有登记标记；单名上限 {} 字节，观测可用 {} 字节。",f.limit,f.available).expect("String");
                for (flag,msg) in [(import_source::READ_ONLY,"目标当前只读"),(import_source::COMPONENT,"目标单名上限不足"),
                    (import_source::RELATIVE,"附加文件 ID 后有相对路径过长")]{
                    if f.blockers&flag!=0{writeln!(text,"  - {msg}。").expect("String");}
                }
                let too_small=u128::from(f.available)<c.logical_bytes;
                if too_small{text.push_str("  - 当前可用空间低于源文件完整逻辑长度。\n");}
                text.push_str("可用空间不是配额或保证，未计入容器、目录及事务开销。\n");
                u8::from(f.blockers!=0||too_small)
            }
        };
        text.push_str(if target.is_some(){PLAN_ONLY}else{ONLY_READ});(code,text)
    }
}

#[cfg(test)]
#[path="../../../../../tests/sqlite/nonempty_import_plan.rs"]
mod plan_tests;

#[cfg(test)]
mod tests{
    use super::*;
    fn draft()->ImportCheck{ImportCheck{system:"/original/S".into(),
        source:OsString::from_vec(b"/held/legacy-\xff".to_vec()).into(),pool:[1;16],init:[2;16],subject:ExplicitSubject::checked([8;16],1001,1002).unwrap(),target:None}}

    #[test]
    fn nonempty_v18_explicit_stateless_selection_has_no_target_or_apply_operand(){
        let parts=["--init-actor","03030303030303030303030303030303","--storage-uid","1001","--storage-gid","1002",
            "--system","/original/S","--pool","01010101010101010101010101010101",
            "--init","02020202020202020202020202020202","--source","/fixture/source"];
        let q=ImportCheck::parse(parts.into_iter().map(OsString::from)).unwrap();
        assert_eq!(q.subject.tuple(),([3;16],1001,1002));
        for flag in ["--target","--apply","--force","--profile","--resume"]{
            assert!(ImportCheck::parse(parts.into_iter().chain([flag,"/fixture/else"]).map(OsString::from)).is_err());
        }
    }

    #[test]
    fn nonempty_v18_manifest_binds_both_original_places_and_raw_source_without_authorizing_any_action(){
        let q=draft();let v=encode(&q,&[7;32],[1,2,3],[4,5,6],None).unwrap();
        let read=Question::decode(&v).unwrap();
        assert_eq!(read.original_system(),(q.subject,[1,2,3],Path::new("/original/S")));
        assert_eq!((read.source,read.q.source.as_os_str().as_bytes()),([4,5,6],b"/held/legacy-\xff".as_slice()));
        assert!(Question::decode(&v[..v.len()-1]).is_err());
        assert!(Question::decode(&[v.as_slice(),&[0]].concat()).is_err());
        let mut other=v;other[0]^=1;assert!(Question::decode(&other).is_err());
    }

    #[test]
    fn nonempty_v18_no_wrong_mount_impossible_counts_or_partial_inventory_on_the_wire(){
        let [pool,init]=[[1;16],[2;16]];
        let c=Counts{files:2,dirs:1,empty_dirs:1,logical_bytes:11};
        let found=Reply::Inventoried(c,42);
        let v=found.encode(pool,init);
        assert_eq!(Reply::decode(&v,pool,init,42,None).unwrap(),found);
        assert!(Reply::decode(&v,pool,init,43,None).is_err());
        assert!(Reply::decode(&v,pool,[3;16],42,None).is_err());
        let count=|counts|Reply::decode(&Reply::Inventoried(counts,42).encode(pool,init),pool,init,42,None);
        assert!(count(Counts{files:2048,..c}).is_err());
        assert!(count(Counts{empty_dirs:2,..c}).is_err());
        assert!(count(Counts{files:0,..c}).is_err());
        let blocked=Reply::Blocked(import_source::NON_PLAIN|import_source::HARD_LINK);
        let mut b=blocked.encode(pool,init);assert_eq!(Reply::decode(&b,pool,init,42,None).unwrap(),blocked);
        b[20]=1;assert!(Reply::decode(&b,pool,init,42,None).is_err());
        for (outcome,code) in [(found,0),(blocked,1),(Reply::Unknown,2)]{
            let (n,text)=outcome.render(pool,init,None);assert_eq!(n,code);assert!(text.ends_with(ONLY_READ));
        }
        assert!(found.render(pool,init,None).1.contains("不是导入可行证明"));
    }
}

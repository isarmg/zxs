//! Resolve explicitly selected ordinary claims plus the single original R.
//! Only the authenticated lower-privilege process opens the actual SQLite ledger.
use super::{bad,host::Worker,manifest::{read_sealed,sealed_owner},transport::{self,Channel,Kind},worker::original_parent};
use crate::{fs_tools::RuntimeFile,path_rules::MAX_PATH_BYTES,pinned::{Kind as PathKind,Pinned},
    smb_profile::{self,first::{Endpoint,Folder,Id,Mode,Origin,Plan,RecycleName,MAX_FOLDERS,MAX_WIRE}},
    sqlite_install::{approval,catalog::{self,Initial},originals::SelectedSystem,pair::{domain::{
        give_live_approval,request_live_approval,require_lent_ex,sealed,LocalDomain,NativeSite},observed,terminal::PublicConfirmation},
        pair::recycle::Recycle,stage::random}};
use helper_linux_evidence::diagnostic_process::{OfflineSql,account_sql_descriptors};
use rustix::{fs::{FlockOperation,flock},net::UCred,process::{DumpableBehavior,Gid,Signal,Uid,dumpable_behavior,
    getppid,set_dumpable_behavior,set_parent_process_death_signal}};
use sha2::{Digest,Sha256};
use std::{collections::BTreeSet,ffi::OsString,fmt::Write,fs::File,io,os::unix::{ffi::{OsStrExt,OsStringExt},fs::MetadataExt},
    path::{Path,PathBuf},time::{Duration,Instant}};

const HELPER:&[&str]=&["usr","bin","helper"];
const ONLINE:&[u8]=b"helper exact host runtime selection v10\0";
const DOMAIN:&[u8]=b"helper exact init selection v10\0";
const BUDGET:usize=DOMAIN.len()+32+3*16+3*8+1+(MAX_FOLDERS+1)*(2+MAX_PATH_BYTES);

#[derive(Debug)]
pub(in crate::sqlite_install::pair) struct Choice {
    pub(in crate::sqlite_install::pair) path:PathBuf,
    pub(in crate::sqlite_install::pair) folder:String,
    pub(in crate::sqlite_install::pair) mode:Mode,
}
struct Question { nonce:[u8;32],actor:Id,pool:Id,init:Id,location:[u64;3],system:PathBuf,paths:Vec<PathBuf> }
fn bad_question()->io::Error{bad("wrong scoped first-layout original INIT selection")}
impl Question {
    fn encode(&self)->io::Result<Vec<u8>>{
        if self.nonce==[0;32]||[self.actor,self.pool,self.init].contains(&[0;16])||
            self.system.to_str().is_none()||self.location[1]==0||self.location[2]==0
            ||!(1..=MAX_FOLDERS).contains(&self.paths.len()){return Err(bad_question());}
        let mut b=DOMAIN.to_vec();
        b.extend(self.nonce);b.extend(self.actor);b.extend(self.pool);b.extend(self.init);
        for n in self.location{b.extend(n.to_be_bytes());}
        fn path(b:&mut Vec<u8>,p:&Path)->io::Result<()>{
            let p=p.as_os_str().as_bytes();crate::path_rules::Kind::StorageRoot.check_path(p).map_err(bad)?;
            b.extend((p.len() as u16).to_be_bytes());b.extend(p);Ok(())
        }
        path(&mut b,&self.system)?;b.push(self.paths.len() as u8);
        let mut seen=BTreeSet::new();
        for p in &self.paths{
            if self.system==*p||!seen.insert(p.as_os_str().as_bytes()){return Err(bad_question());}
            path(&mut b,p)?;
        }
        Ok(b)
    }
    fn decode(bytes:&[u8])->io::Result<Self>{
        if bytes.len()>BUDGET{return Err(bad_question());}
        let mut b=bytes.strip_prefix(DOMAIN).ok_or_else(bad_question)?;
        fn take<'a>(b:&mut &'a [u8],n:usize)->io::Result<&'a [u8]>{
            let (v,rest)=b.split_at_checked(n).ok_or_else(bad_question)?;*b=rest;Ok(v)
        }
        fn field<const N:usize>(b:&mut &[u8])->io::Result<[u8;N]>{Ok(take(b,N)?.try_into().expect("fixed length"))}
        fn path(b:&mut &[u8])->io::Result<PathBuf>{
            let n=u16::from_be_bytes(field(b)?) as usize;
            if n==0||n>MAX_PATH_BYTES{return Err(bad_question());}
            let raw=take(b,n)?;crate::path_rules::Kind::StorageRoot.check_path(raw).map_err(bad)?;
            Ok(PathBuf::from(OsString::from_vec(raw.to_vec())))
        }
        let nonce=field(&mut b)?;let actor=field(&mut b)?;let pool=field(&mut b)?;let init=field(&mut b)?;
        let location=[u64::from_be_bytes(field(&mut b)?),u64::from_be_bytes(field(&mut b)?),u64::from_be_bytes(field(&mut b)?)];
        let system=path(&mut b)?;let [count]=field(&mut b)?;
        if !(1..=MAX_FOLDERS).contains(&(count as usize)){return Err(bad_question());}
        let paths=(0..count).map(|_|path(&mut b)).collect::<io::Result<Vec<_>>>()?;
        let q=Self{nonce,actor,pool,init,location,system,paths};
        if !b.is_empty()||q.encode()?.as_slice()!=bytes{return Err(bad_question());}
        Ok(q)
    }
}
fn place(pin:&Pinned)->io::Result<[u64;3]>{
    let e=pin.entity();Ok([e.dev,e.ino,pin.require_same_named_mount()?.id])
}
pub(in crate::sqlite_install::pair) struct Resolved {
    system:Pinned,ex:File,subject:(Id,u32,u32),plan:Plan,paths_preview:String,host:File,recycle:Recycle,
}
impl Resolved {
    pub(in crate::sqlite_install::pair) fn plan(&self)->&Plan{&self.plan}
    pub(in crate::sqlite_install::pair) fn preview(&self)->&str{&self.paths_preview}
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        require(&self.system,&self.ex,self.subject)?;
        self.recycle.require(&self.host,self.subject.1,[(&self.system,&self.ex)])
    }
    pub(in crate::sqlite_install::pair) fn select(system:&Path,origin:Origin,endpoints:Vec<Endpoint>,choices:Vec<Choice>,recycle:RecycleName)->io::Result<Self>{
        approval::no_inherited_descriptors()?;
        let host=approval::host_admin()?;
        let tty=PublicConfirmation::select()?;
        let subject=approval::profile()?;
        let system=Pinned::open(system,PathKind::StorageRoot)?;
        let ex=system.storage_directory()?;flock(&ex,FlockOperation::NonBlockingLockExclusive)?;
        let check=||{tty.check()?;require(&system,&ex,subject)};
        check()?;
        let mut nonce=[0;32];nonce[..16].copy_from_slice(&random()?);nonce[16..].copy_from_slice(&random()?);
        let req=Question{nonce,actor:subject.0,pool:origin.pool,init:origin.init,location:place(&system)?,system:system.path().to_path_buf(),
            paths:choices.iter().map(|c|c.path.clone()).collect()};
        let (folders,claim)=read_layout(req,&ex,subject,check,origin)?;
        let f=choices.iter().zip(&folders).map(|(c,&volume)|Folder{volume,name:c.folder.clone(),mode:c.mode}).collect();
        let plan=Plan::selected(origin,endpoints,f,recycle).map_err(bad)?;
        // Setup cannot be issued for an absent, unconfirmed or read-only R.
        // Prove its actual original before asking for the layout approval.
        let recycle=Recycle::select(&claim,&host,subject.1,[(&system,&ex)])?;
        let mut paths_preview=String::from("本次低权限只读核对的原 SYSTEM：\"");
        smb_profile::escaped(&mut paths_preview,system.path().as_os_str().as_bytes());
        paths_preview.push_str("\"；普通盘只核原登记字节，未读盘内容。\n");
        paths_preview.push_str("  已核独立回收盘原路径 \"");
        smb_profile::escaped(&mut paths_preview,recycle.source().0.as_os_str().as_bytes());
        paths_preview.push_str("\"；只核卷身份与固定标记。\n");
        for (c,id) in choices.iter().zip(&folders){
            let hex=id.iter().map(|b|format!("{b:02x}")).collect::<String>();
            paths_preview.push_str("  已选原路径 \"");
            smb_profile::escaped(&mut paths_preview,c.path.as_os_str().as_bytes());
            writeln!(paths_preview,"\" → 原普通卷 {hex}。").expect("writing a String");
        }
        let out=Self{system,ex,subject,plan,paths_preview,host,recycle};out.require()?;Ok(out)
    }
}
fn require(pin:&Pinned,file:&File,subject:(Id,u32,u32))->io::Result<()>{
    approval::host_admin()?;
    let m=pin.metadata()?;
    if approval::profile()?!=subject||!m.is_dir()||m.mode()!=0o040700||m.uid()!=subject.1||m.nlink()==0{
        return Err(bad("different protected original SYSTEM or maintenance subject"));
    }
    require_lent_ex(file,(pin.entity(),pin.require_same_named_mount()?.id))
}
fn body(i:usize,id:Id)->[u8;65]{
    let mut b=[0;65];b[0]=i as u8;b[1..17].copy_from_slice(&id);b
}
fn query<T>(raw:&[u8],original:&File,subject:(Id,u32,u32),check:impl Fn()->io::Result<()>,
    receive:impl FnOnce(&Channel,&[u8;32],Instant)->io::Result<T>)->io::Result<T>{
    let helper=RuntimeFile::open(HELPER,true,64<<20)?;helper.require_running_helper()?;
    let scope:[u8;32]=Sha256::digest(raw).into();
    let meta=sealed(raw)?;let (parent,child)=transport::pair()?;
    let mut hired=Worker::spawn(&helper.file,&child,&meta,OfflineSql::FirstLayout)?;
    drop(child);drop(meta);
    let who=UCred{pid:hired.pid,uid:Uid::from_raw(subject.1),gid:Gid::from_raw(subject.2)};
    let until=Instant::now()+Duration::from_secs(20);
    loop{
        check()?;hired.live()?;
        if Instant::now()>=until{return Err(bad("the original read-only INIT worker did not respond"));}
        if give_live_approval(&parent,who,&scope,||{check()?;helper.require_running_helper()})?{break;}
        std::thread::sleep(Duration::from_millis(20));
    }
    let channel=Channel::from_owned_child_session(parent,who,scope)?;
    channel.send_original(Kind::Lease,&scope,&[0;65],original)?;
    let v=receive(&channel,&scope,until)?;
    hired.finish(&channel,until)?;check()?;
    Ok(v)
}
fn read_layout(q:Question,ex:&File,subject:(Id,u32,u32),check:impl Fn()->io::Result<()>,origin:Origin)->io::Result<(Vec<Id>,Initial)>{
    query(&q.encode()?,ex,subject,&check,|channel,scope,until|{
        let mut ids=Vec::with_capacity(q.paths.len());
        for i in 0..q.paths.len(){
            let (v,[])=channel.wait_checked::<0>(Kind::ChosenFolder,scope,until,&check)?;
            let id:Id=v[1..17].try_into().expect("fixed reply");
            if v!=body(i,id)||id==[0;16]||ids.contains(&id){return Err(bad_question());}
            ids.push(id);
        }
        let r=receive_media(channel,scope,until,&check,q.paths.len(),None,origin,subject)?;
        if ids.iter().any(|id|id.as_slice()==&r.marker[37..53]){return Err(bad_question());}
        Ok((ids,r))
    })
}
struct LiveQuery{nonce:[u8;32],actor:Id,location:[u64;3],system:PathBuf,plan:Plan,bound:[u8;128]}
impl LiveQuery{
    fn emit(&self)->io::Result<Vec<u8>>{
        let p=self.system.to_str().ok_or_else(bad_question)?.as_bytes();
        crate::path_rules::Kind::StorageRoot.check_path(p).map_err(bad)?;
        let o=self.plan.origin;
        if self.nonce==[0;32]||self.actor==[0;16]||self.location[1..].contains(&0){return Err(bad_question());}
        let live=Sha256::digest(smb_profile::render(&self.plan).map_err(bad)?.as_bytes()).into();
        LocalDomain::expected_binding(&self.bound,&o.pool,&o.instance,&o.domain,&live)?;
        let mut b=ONLINE.to_vec();b.extend(self.nonce);b.extend(self.actor);
        for n in self.location{b.extend(n.to_be_bytes());}
        b.extend(self.bound);
        b.extend((p.len() as u16).to_be_bytes());b.extend(p);
        b.extend(self.plan.wire());
        Ok(b)
    }
    fn decode(raw:&[u8])->io::Result<Self>{
        const FIXED:usize=32+16+24+128+2;
        if raw.len()>ONLINE.len()+FIXED+MAX_PATH_BYTES+MAX_WIRE{return Err(bad_question());}
        let b=raw.strip_prefix(ONLINE).filter(|v|v.len()>FIXED).ok_or_else(bad_question)?;
        let (head,tail)=b.split_at(FIXED);
        let n=u16::from_be_bytes(head[FIXED-2..].try_into().expect("fixed length")) as usize;
        let (path,plan)=tail.split_at_checked(n).ok_or_else(bad_question)?;
        let system=PathBuf::from(std::str::from_utf8(path).map_err(|_|bad_question())?);
        let q=Self{nonce:head[..32].try_into().expect("fixed nonce"),actor:head[32..48].try_into().expect("fixed actor"),
            location:std::array::from_fn(|i|u64::from_be_bytes(head[48+i*8..56+i*8].try_into().expect("fixed location"))),
            bound:head[72..200].try_into().expect("fixed binding"),system,plan:Plan::decode(plan).map_err(bad)?};
        if q.emit()?.as_slice()!=raw{return Err(bad_question());}
        Ok(q)
    }
}
fn volume_reply(i:usize,raw:&[u8])->[u8;65]{
    let mut b=[0;65];b[0]=i as u8;b[1..33].copy_from_slice(&Sha256::digest(raw));b
}
fn media_bytes(record:&crate::sqlite_install::catalog::Initial)->Vec<u8>{
    let mut out=record.marker.to_vec();out.extend(record.path.as_os_str().as_bytes());out
}
// A missing `expected` can represent only the uniquely issued ordinal-one R.
fn material(raw:&[u8],expected:Option<&Folder>,origin:Origin)->io::Result<Initial>{
    let (prefix,path)=raw.split_at_checked(101).ok_or_else(bad_question)?;
    let marker:[u8;101]=prefix.try_into().expect("control marker width");
    crate::path_rules::Kind::StorageRoot.check_path(path).map_err(bad)?;
    let role=if expected.is_some(){2}else{1};
    if &marker[..20]!=b"ZXS-SQLITE-1/VOLUME\0"||marker[20]!=role||marker[21..37]!=origin.pool||
        marker[37..53]==[0;16]||marker[37..53]==origin.pool||expected.is_some_and(|f|marker[37..53]!=f.volume)
        ||marker[53..69]!=origin.init||marker[69..]==[0;32]{return Err(bad_question());}
    Ok(Initial{path:PathBuf::from(OsString::from_vec(path.to_vec())),marker})
}
fn receive_media(channel:&Channel,scope:&[u8;32],until:Instant,check:impl Fn()->io::Result<()>,
    i:usize,folder:Option<&Folder>,origin:Origin,subject:(Id,u32,u32))->io::Result<Initial>{
    let (reply,[fd])=channel.wait_checked::<1>(Kind::VolumeClaim,scope,until,check)?;
    let value=sealed_owner(&fd,101+MAX_PATH_BYTES,(subject.1,subject.2))?;
    if reply!=volume_reply(i,&value){return Err(bad_question());}
    material(&value,folder,origin)
}
fn send_media(channel:&Channel,scope:&[u8;32],i:usize,claim:&Initial)->io::Result<()>{
    let bytes=media_bytes(claim);let report=sealed(&bytes)?;
    channel.send_original(Kind::VolumeClaim,scope,&volume_reply(i,&bytes),&report)
}
pub(in crate::sqlite_install::pair) fn online_roots(site:&NativeSite)->io::Result<observed::StartupClaims>{
    let (pin,ex,..)=site.online_sources()?;let subject=site.subject();
    let mut nonce=[0;32];nonce[..16].copy_from_slice(&random()?);nonce[16..].copy_from_slice(&random()?);
    let q=LiveQuery{nonce,actor:subject.0,location:place(pin)?,system:pin.path().into(),plan:site.first_plan().clone(),bound:site.bound_bytes()};
    let raw=q.emit()?;
    query(&raw,ex,subject,||site.require(),|channel,scope,until|{
        let mut ordinary=Vec::with_capacity(q.plan.folders().len());
        for (i,(wanted,_)) in q.plan.folders().iter().enumerate(){
            ordinary.push(receive_media(channel,scope,until,||site.require(),i,Some(wanted),q.plan.origin,subject)?);
        }
        let recycle=receive_media(channel,scope,until,||site.require(),ordinary.len(),None,q.plan.origin,subject)?;
        if ordinary.iter().any(|c|c.marker[37..53]==recycle.marker[37..53]||c.path==recycle.path){return Err(bad_question());}
        Ok(observed::StartupClaims{ordinary,recycle})
    })
}

pub(crate) fn worker()->io::Result<()>{
    let [fd,meta]=account_sql_descriptors()?;
    approval::only_inherited_descriptors(&[3,4])?;
    let parent=getppid().ok_or_else(bad_question)?;
    original_parent(&fd,parent)?;
    let host=approval::host_admin()?;let (actor,uid,gid)=approval::profile()?;
    let helper=RuntimeFile::open(HELPER,true,64<<20)?;
    helper.require_running_helper()?;helper.require_parent_helper(parent)?;drop(helper);
    approval::drop_to_storage(uid,gid)?;
    set_parent_process_death_signal(Some(Signal::TERM))?;
    original_parent(&fd,parent)?;
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    if dumpable_behavior()?!=DumpableBehavior::NotDumpable{return Err(bad_question());}
    let raw=read_sealed(&meta,BUDGET)?;drop(meta);
    let mode=if raw.starts_with(ONLINE){(None,Some(LiveQuery::decode(&raw)?))}
        else{(Some(Question::decode(&raw)?),None)};
    let source=match &mode{
        (Some(q),None)=>(q.actor,q.location,&q.system),
        (None,Some(q))=>(q.actor,q.location,&q.system),
        _=>return Err(bad_question()),
    };
    if source.0!=actor{return Err(bad_question());}
    let scope:[u8;32]=Sha256::digest(&raw).into();
    request_live_approval(&fd,parent,&scope,||original_parent(&fd,parent))?;
    let channel=Channel::from_owned_child_session(fd,UCred{pid:parent,uid:Uid::ROOT,gid:Gid::ROOT},scope)?;
    let (empty,[lease])=channel.wait::<1>(Kind::Lease,&scope,Instant::now()+Duration::from_secs(20))?;
    if empty!=[0;65]{return Err(bad_question());}
    let pin=Pinned::open(source.2,PathKind::StorageRoot)?;
    if place(&pin)?!=source.1{return Err(bad_question());}
    let open=SelectedSystem::from_origin(pin,host,uid,gid)?;
    if let (None,Some(q))=&mode{
        let system=open.from_lent_sh(lease)?;
        let chosen=observed::startup_claims(&q.plan,&Sha256::digest(q.bound).into(),&system)?;
        if getppid()!=Some(parent){return Err(bad_question());}system.require()?;
        for (i,m) in chosen.ordinary.iter().enumerate(){send_media(&channel,&scope,i,m)?;}
        return send_media(&channel,&scope,chosen.ordinary.len(),&chosen.recycle);
    }
    let (Some(q),None)=mode else{return Err(bad_question());};
    let system=open.from_lent_ex(lease)?;
    let catalogue=observed::init_claims(&q.pool,&q.init,&system)?;
    let chosen=q.paths.iter().map(|p|{
        catalogue.iter().find(|c|c.marker[20]==2&&c.path.as_os_str().as_bytes()==p.as_os_str().as_bytes()).map(|c|c.marker[37..53].try_into().expect("VolumeId"))
            .ok_or_else(||bad("selected path is not a true ordinary F of this original INIT"))
    }).collect::<io::Result<Vec<Id>>>()?;
    if getppid()!=Some(parent){return Err(bad_question());}
    system.require()?;
    let recycle=catalog::initial_recycle(&catalogue)?;
    for (i,volume) in chosen.into_iter().enumerate(){channel.send(Kind::ChosenFolder,&scope,&body(i,volume))?;}
    send_media(&channel,&scope,q.paths.len(),recycle)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn startup_material_binds_a_marker_to_the_exact_approved_volume(){
        let o=Origin{pool:[1;16],init:[2;16],instance:[3;16],domain:[4;16],setup:[5;16]};
        let chosen=Folder{volume:[0x21;16],name:"Docs".into(),mode:Mode::ReadOnly};
        let proof=crate::sqlite_install::catalog::Initial{
            path:PathBuf::from(OsString::from_vec(b"/mnt/original-\xfe".to_vec())),
            marker:crate::sqlite_install::catalog::claimed_marker(2,&o.pool,&chosen.volume,&o.init,&[0x31;32]),
        };
        let raw=media_bytes(&proof);
        assert_eq!(material(&raw,Some(&chosen),o).unwrap().path,proof.path);
        assert!(material(&raw,Some(&Folder{volume:[0x22;16],..chosen.clone()}),o).is_err());
        assert!(material(&raw,None,o).is_err());
        let mut recycle=raw.clone();recycle[20]=1;
        assert!(material(&recycle,Some(&chosen),o).is_err());
        assert_eq!(material(&recycle,None,o).unwrap().path,proof.path);
        let mut wrong=raw.clone();wrong[53]=0;assert!(material(&wrong,Some(&chosen),o).is_err());
        let mut wrong=raw;wrong.extend(b"/../other");assert!(material(&wrong,Some(&chosen),o).is_err());
    }
    #[test] fn exact_raw_paths_are_distinct_and_not_normalized(){
        let mut q=Question{nonce:[7;32],actor:[1;16],pool:[2;16],init:[3;16],location:[0,9,15],
            system:"/system".into(),paths:vec![PathBuf::from(OsString::from_vec(b"/mnt/data-\xff".to_vec())),"/mnt/Data".into()]};
        let raw=q.encode().unwrap();assert_eq!(Question::decode(&raw).unwrap().paths,q.paths);
        let mut extra=raw.clone();extra.push(0);assert!(Question::decode(&extra).is_err());
        assert!(Question::decode(&raw[..raw.len()-1]).is_err());
        q.paths.push(q.paths[0].clone());assert!(q.encode().is_err());q.paths.pop();
        q.paths[0]=q.system.clone();assert!(q.encode().is_err());
    }
}

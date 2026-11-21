//! Private one-shot proof for a fresh standalone ZXS deployment. It is not
//! an installer policy, online route, user credential, or persisted cache.
mod wire;
mod db;
pub(super) use db::InitialDatabase;
pub(super) use wire::{CredentialScope,CryptoPins,SitePayload,DEPLOYMENT_BYTES,MAX_SITE,activation};
use super::bad;
use sha2::{Digest,Sha256};
use std::{ffi::OsString,io};

const SCHEMA_SHA:&str="a5eb88872ac976929b6ca4e5621a74de979ac9040512d1d937b50452307f42d0";
pub(in crate::sqlite_install) fn compiled_v17()->io::Result<()>{
    let wanted=crate::fs_tools::hex(SCHEMA_SHA).expect("frozen v17 DDL hash");
    let actual:[u8;32]=Sha256::digest(crate::sqlite_install::stage::SCHEMA.as_bytes()).into();
    if crate::sqlite_install::catalog::SCHEMA_VERSION!=17||actual!=wanted{
        return Err(bad("the private fresh ZXS issuer is not qualified for this exact v17 format"));
    }
    Ok(())
}
pub(in crate::sqlite_install) fn require_v17()->io::Result<()>{
    if !cfg!(feature="private-fresh-zxs-v17"){return Err(bad("private fresh ZXS issuance is disabled"));}
    compiled_v17()
}
type Id=[u8;16];
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::sqlite_install) struct ExplicitSubject{actor:Id,uid:u32,gid:u32}
impl ExplicitSubject{
    pub(in crate::sqlite_install) fn checked(actor:Id,uid:u32,gid:u32)->io::Result<Self>{
        if actor==[0;16]||[uid,gid].iter().any(|n|matches!(*n,0|65534|u32::MAX)){
            return Err(bad("expected the approved nonroot storage subject and original INIT actor"));
        }
        Ok(Self{actor,uid,gid})
    }
    pub(in crate::sqlite_install) fn parse(it:&mut impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        use super::command::{key,value};
        let actor=key(it,"--init-actor")?;
        fn who(it:&mut impl Iterator<Item=OsString>,key:&str)->Result<u32,&'static str>{
            let os=value(it,key)?;
            let s=os.to_str().ok_or("expected a canonical numeric storage credential")?;
            s.parse::<u32>().ok().filter(|v|!matches!(*v,0|65534|u32::MAX)&&v.to_string()==s)
                .ok_or("expected a canonical nonroot numeric storage UID/GID")
        }
        let uid=who(it,"--storage-uid")?;let gid=who(it,"--storage-gid")?;
        Self::checked(actor,uid,gid).map_err(|_|"wrong original explicit storage subject")
    }
    pub(in crate::sqlite_install) fn tuple(self)->(Id,u32,u32){(self.actor,self.uid,self.gid)}
}

pub(super) mod image;
use super::{command::{key,name,sha,value},distinct_roles,domain::{FreshHome,FreshParent},native_run::{Choice,ResolvedRust,transact_fresh},
    native_sql::rust_vault::{Desired,RustOrder},rust_enroll::{crypto::Cryptography,manifest::{Intent,Manifest}},terminal::PublicConfirmation};
use crate::{path_rules::Kind,smb_profile::{self,first::{Endpoint,Mode,RecycleName,MAX_ENDPOINTS,MAX_FOLDERS},
    rust_first::{self,Origin}},sqlite_install::{approval,stage::random}};
use std::{ffi::OsStr,fmt::Write as _,path::PathBuf,time::{Duration,Instant}};

/// One internal synthetic-site command with explicit immutable authority.
/// No alternate runtime/site/secret path or receipt/recovery is accepted.
#[derive(Debug)]
pub(super) struct Selection{
    subject:ExplicitSubject,system:PathBuf,origin:Origin,reference:Id,init_sha:[u8;32],
    image:[u8;32],crypto:CryptoPins,hostname:String,web:Vec<u8>,smb:Vec<u8>,
    recycle:RecycleName,endpoints:Vec<Endpoint>,folders:Vec<Choice>,
}
impl Selection{
    pub(super) fn parse(mut it:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        let subject=ExplicitSubject::parse(&mut it)?;
        let system=PathBuf::from(value(&mut it,"--system")?);
        Kind::StorageRoot.check_path(system.as_os_str().as_encoded_bytes())?;
        if !system.to_str().is_some_and(|s|s.len()<=4095){return Err("SYSTEM must be an exact protected UTF-8 path");}
        let origin=Origin{
            pool:key(&mut it,"--pool")?,init:key(&mut it,"--init")?,instance:key(&mut it,"--instance")?,
            bind:key(&mut it,"--bind")?,setup:key(&mut it,"--setup-op")?,
        };
        let ids=[origin.pool,origin.init,origin.instance,origin.bind,origin.setup];
        if ids.iter().enumerate().any(|(i,n)|ids[..i].contains(n)){return Err("fresh installation identities must be distinct");}
        let init_sha=sha(&mut it,"--init-sha256")?;
        let reference=key(&mut it,"--approval")?;
        let image=sha(&mut it,"--zxs-sha256")?;
        let crypto=CryptoPins{helper:sha(&mut it,"--helper-sha256")?,
            gcrypt:sha(&mut it,"--gcrypt-sha256")?,gpgerror:sha(&mut it,"--gpgerror-sha256")?};
        let hostname=value(&mut it,"--host")?.into_string().ok().filter(|v|rust_first::valid_host(v))
            .ok_or("expected the canonical lower ASCII installation hostname")?;
        let web=name(&mut it,"--web-login")?;
        let smb=name(&mut it,"--smb-login")?;
        if !distinct_roles(&web,&smb){return Err("expected two distinct non-guest roles");}
        let recycle=RecycleName::new(value(&mut it,"--recycle-name")?.into_string()
            .map_err(|_|"expected a valid logical recycle name")?)?;
        let mut endpoints=Vec::new();let mut folders:Vec<Choice>=Vec::new();
        while let Some(flag)=it.next(){
            if flag=="--listen"&&endpoints.len()<MAX_ENDPOINTS{
                let ep=it.next().ok_or("missing explicitly selected LAN listener")?;
                endpoints.push(Endpoint::named(ep.to_str().ok_or("expected exact IP and prefix")?)?);
            }else if flag=="--share"&&folders.len()<MAX_FOLDERS{
                let p=PathBuf::from(it.next().ok_or("missing actual original data root")?);
                Kind::StorageRoot.check_path(p.as_os_str().as_encoded_bytes())?;
                if system.starts_with(&p)||p.starts_with(&system)||folders.iter().any(|f|p.starts_with(&f.path)||f.path.starts_with(&p)){
                    return Err("SYSTEM and original data paths must not overlap");
                }
                let folder=it.next().and_then(|s|s.into_string().ok()).ok_or("missing exact UTF-8 folder name")?;
                let mode=match it.next().as_deref().and_then(OsStr::to_str){
                    Some("ro")=>Mode::ReadOnly,Some("rw")=>Mode::ReadWrite,_=>return Err("select the explicit folder mode ro or rw"),
                };
                folders.push(Choice{path:p,folder,mode});
            }else{return Err("only explicit bounded listeners and original folders; no recovery, password, or alternate executable");}
        }
        if endpoints.is_empty()||folders.is_empty(){return Err("select at least one LAN listener and original ordinary folder");}
        Ok(Self{subject,system,origin,reference,init_sha,image,crypto,hostname,web,smb,recycle,endpoints,folders})
    }
    pub(super) fn run(self)->io::Result<(u8,String)>{
        require_v17()?;
        let Self{subject,system,origin,reference,init_sha,image,crypto,hostname,web,smb,recycle,endpoints,folders}=self;
        // Check host/stdin custody before obtaining any unrelated FD. This
        // genuine low reader holds the original S EX and no root SQL.
        let parent=FreshParent::select(origin.instance)?;
        let resolved=ResolvedRust::fresh(&parent,&system,origin,hostname,endpoints,folders,recycle,subject,crypto.helper,init_sha)?;
        let tty=PublicConfirmation::select()?;
        let image_file=image::Image::select(&image)?;
        let providers=Cryptography::fresh(crypto)?;
        let payload=SitePayload::new(resolved.plan(),subject,&system,std::path::Path::new(image::PATH),image)?;
        let target=parent.bind(&resolved,payload.clone())?;
        helper_linux_evidence::diagnostic_process::watch_cancellation()?;
        let review=preview(&resolved,&payload,&target,&web,&smb,&reference,&init_sha,crypto)?;
        tty.confirm_fresh_zxs(review)?;
        let deadline=Instant::now()+Duration::from_secs(10*60);
        let live=||->io::Result<()>{
            if Instant::now()>=deadline{return Err(bad("the original fresh product approval expired"));}
            // The low Web secret temporarily changes the TTY flags. That
            // stage checks the same original tty through HeldQuiet instead.
            approval::host_admin()?;
            providers.require()?;image_file.require()?;resolved.require()
        };
        let phase=(||->io::Result<_>{
            live()?;tty.check()?;
            let site=target.prepare(&resolved,&tty,&live)?;
            let authority=site.fresh_payload().ok_or_else(||bad("missing actual fresh authority"))?;
            let(pin,_)=site.original_system()?;let entity=pin.entity();
            let m=Manifest{intent:Intent::Fresh,nonce:random()?,system:system.clone(),
                entity:[entity.dev,entity.ino,pin.require_same_named_mount()?.id],plan:site.plan().clone(),
                order:RustOrder{op:origin.setup,actor:subject.actor,approval:reference,role:Desired::Setup{web,smb}},
                fresh:Some(CredentialScope{subject,crypto,site:authority.sha()?,init_sha,database:resolved.fresh_database()?})};
            let active=transact_fresh(&site,m,&tty,&live)?;
            let record=active.at(&site)?.record;
            let site_sha=target.activate(&resolved,&site,active,&tty,&live)?;
            Ok((site_sha,record))
        })();
        let(site_sha,record)=phase.map_err(|e|io::Error::new(e.kind(),
            format!("fresh ZXS issuance outcome not confirmed; preserve the exact original INIT, SETUP and all stage/final names; do not retry, adopt or delete: {e}")))?;
        Ok((0,format!("fresh-zxs=confirmed pool={} init={} instance={} setup={} site_sha256={} generation=1 record={}; installed authority only, no service started or login checked",
            hex(&origin.pool),hex(&origin.init),hex(&origin.instance),hex(&origin.setup),hex(&site_sha),hex(&record))))
    }
}
fn hex(bytes:&[u8])->String{bytes.iter().map(|b|format!("{b:02x}")).collect()}
fn preview(resolved:&ResolvedRust,payload:&SitePayload,target:&FreshHome,web:&[u8],smb:&[u8],reference:&Id,init_sha:&[u8;32],crypto:CryptoPins)
    ->io::Result<String>{
    let p=resolved.plan();let o=p.origin;let(actor,uid,gid)=payload.subject.tuple();
    let mut out=format!("仅发行从未设置账号的 v17 新 ZXS 池。不会续单、覆盖、清理、迁移，也不会启动任何服务。若中断则保留所有原件，不能盲目重做。\n\
        原池 {} / INIT {} / 原请求 SHA-256 {} / 原主体 {} / 存储 UID:GID {uid}:{gid}。\n\
        新站点 {} / Bind {} / SETUP {} / 批准参考 {}。\n\
        实际受保护的目标 {}。此操作最后独立写入新 deployment.v1；已有文件一律不接管。\n\
        已独立确认的单一 {} SHA-256 {}；原描述 SHA-256 {}；新站点记录 SHA-256 {}。\n\
        当前指定 Helper {}、离线 libgcrypt {}、离线 libgpg-error {}；这些不随 ZXS 运行。\n\
        {}本次 ZXS 主机 {}；Web 用户 {}、SMB 用户 {}。之后在本原终端独立确认并输入两个密码。\n",
        hex(&o.pool),hex(&o.init),hex(init_sha),hex(&actor),hex(&o.instance),hex(&o.bind),hex(&o.setup),hex(reference),target.evidence()?,
        image::PATH,hex(&payload.elf_sha),hex(&payload.plan_sha),hex(&payload.sha()?),hex(&crypto.helper),hex(&crypto.gcrypt),
        hex(&crypto.gpgerror),resolved.preview(),p.host(),std::str::from_utf8(web).map_err(|_|bad("Web endpoint is not UTF-8"))?,
        std::str::from_utf8(smb).map_err(|_|bad("SMB endpoint is not UTF-8"))?);
    for e in p.endpoints(){writeln!(out,"  SMB {}:{} / 子网前缀 {}（预定而未监听）",e.address(),445,e.prefix()).expect("writing String");}
    out.push_str("  原回收路径在 ZXS 中的计划名称 \"");
    smb_profile::escaped(&mut out,p.recycle().as_str().as_bytes());out.push_str("\"\n");
    for(f,_) in p.folders(){
        out.push_str("  ZXS 目录 \"");smb_profile::escaped(&mut out,f.name.as_bytes());
        writeln!(out,"\"，原卷 {}，初始 {}",hex(&f.volume),if f.mode==Mode::ReadWrite{"rw"}else{"ro"}).expect("writing String");
    }
    Ok(out)
}

#[cfg(test)]
mod tests{
    use super::*;
    fn arg(s:&str)->impl Iterator<Item=OsString>+'_{s.split_ascii_whitespace().map(OsString::from)}
    #[test]
    fn fresh_v17_issue_parser_cannot_select_an_old_action_or_credential_path(){
        let form=format!("--init-actor {} --storage-uid 1001 --storage-gid 1002 --system /lab/S --pool {} --init {} \
            --instance {} --bind {} --setup-op {} --init-sha256 {} --approval {} --zxs-sha256 {} --helper-sha256 {} --gcrypt-sha256 {} \
            --gpgerror-sha256 {} --host fresh.test --web-login first.web --smb-login first.smb --recycle-name Recycle \
            --listen 192.0.2.17/24 --share /lab/F Work rw",hex(&[31;16]),hex(&[1;16]),hex(&[2;16]),hex(&[3;16]),
            hex(&[4;16]),hex(&[5;16]),hex(&[45;32]),hex(&[6;16]),hex(&[41;32]),hex(&[42;32]),hex(&[43;32]),hex(&[44;32]));
        assert!(Selection::parse(arg(&form)).is_ok());
        for bad in [format!("{form} --resume"),format!("{form} --password bogus"),format!("{form} --site /other"),
            form.replace("first.smb","guest"),form.replace("/lab/F","/lab/S/F"),form.replace("1001","01001"),
            form.replace(&hex(&[3;16]),&hex(&[2;16])),form.replace("192.0.2.17/24","0.0.0.0/0"),
            form.replace(&hex(&[41;32]),&hex(&[0;32]))]{assert!(Selection::parse(arg(&bad)).is_err());}
        let public=format!("issue-fresh-zxs-v17 {form}");
        assert_eq!(super::super::command::Setup::parse(arg(&public)).is_ok(),cfg!(feature="private-fresh-zxs-v17"));
    }
}

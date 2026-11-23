//! Private one-shot proof for a fresh standalone ZXS deployment. It is not
//! an installer policy, online route, user credential, or persisted cache.
mod wire;
mod db;
pub(super) use db::InitialDatabase;
pub(super) use wire::{CredentialScope,CryptoPins,SitePayload,DEPLOYMENT_BYTES,MAX_SITE,activation};
use super::bad;
use sha2::{Digest,Sha256};
use std::{ffi::OsString,io};

const SCHEMA_SHA:&str="4d54de32c0004f7b5c0ed6af44a556d8add56517518cf7d8b0a5fe8bb158a0f4";
pub(in crate::sqlite_install) fn compiled_schema()->io::Result<()>{
    let wanted=crate::fs_tools::hex(SCHEMA_SHA).expect("frozen v21 DDL hash");
    let actual:[u8;32]=Sha256::digest(crate::sqlite_install::stage::SCHEMA.as_bytes()).into();
    if crate::sqlite_install::catalog::SCHEMA_VERSION!=21||actual!=wanted{
        return Err(bad("the private fresh ZXS issuer is not qualified for this exact v21 format"));
    }
    Ok(())
}
pub(in crate::sqlite_install) fn require_private_install()->io::Result<()>{
    if !cfg!(feature="private-fresh-install"){return Err(bad("private fresh ZXS issuance is disabled"));}
    compiled_schema()
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
        fn who(it:&mut impl Iterator<Item=OsString>,key:&str)->Result<u32,&'static str>{
            let os=value(it,key)?;
            let s=os.to_str().ok_or("expected a canonical numeric storage credential")?;
            s.parse::<u32>().ok().filter(|v|!matches!(*v,0|65534|u32::MAX)&&v.to_string()==s)
                .ok_or("expected a canonical nonroot numeric storage UID/GID")
        }
        let actor=key(it,"--init-actor")?;
        let uid=who(it,"--storage-uid")?;let gid=who(it,"--storage-gid")?;
        Self::checked(actor,uid,gid).map_err(|_|"wrong original explicit storage subject")
    }
    pub(in crate::sqlite_install) fn tuple(self)->(Id,u32,u32){(self.actor,self.uid,self.gid)}
}

pub(super) mod image;
#[cfg(feature="embedded-zxs")]
pub(crate) use image::EmbeddedInstall;
use super::{command::{key,name,sha,value},domain::{FreshHome,FreshParent},native_run::{Choice,FirstShares,FreshScope,ResolvedRust,offer_account_web,transact_fresh},
    native_sql::rust_vault::{ActiveRustMeta,Desired,RustChange,RustOrder,SetupSpec,shared_login},rust_enroll::{crypto::Cryptography,manifest::{Authority,Intent,Manifest},receipt::Observed},terminal::PublicConfirmation};
use crate::{smb_profile::{self,first::{Mode,RecycleName,MAX_FOLDERS},
    rust_first::{self,Origin}},sqlite_install::{approval,product_layout,stage::random}};
use helper_linux_evidence::prompt_signals::PromptSignals;
use std::{ffi::OsStr,fmt::Write as _,path::PathBuf,time::{Duration,Instant}};

// Before and after password prompts only. Do not leave process-wide
// handlers or a blocked signal mask across the next independent TTY guard.
fn filesystem_phase<T>(live:&dyn Fn()->io::Result<()>,
    phase:impl FnOnce(&dyn Fn()->io::Result<()>)->io::Result<T>)->io::Result<T>{
    let signals=PromptSignals::begin()?;
    let guarded=||{signals.check()?;live()};
    guarded()?;
    let value=phase(&guarded);
    let last=guarded();
    signals.finish()?;
    last?;value
}

/// No alternate runtime/site/secret path or receipt/recovery is accepted.
#[derive(Debug)]
pub(super) struct Selection{
    subject:ExplicitSubject,system:PathBuf,origin:Origin,reference:Id,init_sha:[u8;32],
    image_path:PathBuf,image:[u8;32],crypto:CryptoPins,login:Vec<u8>,shares:FirstShares,
}
impl Selection{
    pub(super) fn parse(mut it:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        let subject=ExplicitSubject::parse(&mut it)?;
        let mut it=it.peekable();
        let system=if it.peek().is_some_and(|s|s=="--system"){PathBuf::from(value(&mut it,"--system")?)}
            else{PathBuf::from(product_layout::SYSTEM)};
        product_layout::media(&system)?;
        if !system.to_str().is_some_and(|s|s.len()<=4095){return Err("SYSTEM must be an exact protected UTF-8 path");}
        let origin=Origin{
            pool:key(&mut it,"--pool")?,init:key(&mut it,"--init")?,instance:key(&mut it,"--instance")?,
            bind:key(&mut it,"--bind")?,setup:key(&mut it,"--setup-op")?,
        };
        let ids=[origin.pool,origin.init,origin.instance,origin.bind,origin.setup];
        if ids.iter().enumerate().any(|(i,n)|ids[..i].contains(n)){return Err("fresh installation identities must be distinct");}
        let init_sha=sha(&mut it,"--init-sha256")?;
        let reference=key(&mut it,"--approval")?;
        let image_path=if it.peek().is_some_and(|s|s=="--zxs-image"){PathBuf::from(value(&mut it,"--zxs-image")?)}
            else{PathBuf::from(product_layout::IMAGE)};
        image::check_path(&image_path).map_err(|_|"choose the protected canonical absolute path to the fixed zxs executable")?;
        if system.starts_with(&image_path)||image_path.starts_with(&system){return Err("SYSTEM must not contain the executable");}
        let image=sha(&mut it,"--zxs-sha256")?;
        let crypto=CryptoPins{helper:sha(&mut it,"--helper-sha256")?,
            gcrypt:sha(&mut it,"--gcrypt-sha256")?,gpgerror:sha(&mut it,"--gpgerror-sha256")?};
        let hostname=value(&mut it,"--host")?.into_string().ok().filter(|v|rust_first::valid_host(v))
            .ok_or("expected the canonical lower ASCII installation hostname")?;
        let login=name(&mut it,"--login")?;
        if !shared_login(&login){return Err("expected one canonical non-guest login for both Web and SMB");}
        let recycle=RecycleName::new(value(&mut it,"--recycle-name")?.into_string()
            .map_err(|_|"expected a valid logical recycle name")?)?;
        let mut folders:Vec<Choice>=Vec::new();
        while let Some(flag)=it.next(){
            if (flag=="--share"||flag=="--share-label")&&folders.len()<MAX_FOLDERS{
                let selected=it.next().ok_or("missing actual original data root or explicit DATA label")?;
                let p=if flag=="--share-label"{product_layout::data_label(selected)?}else{PathBuf::from(selected)};
                product_layout::media(&p)?;
                if system.starts_with(&p)||p.starts_with(&system)||image_path.starts_with(&p)||p.starts_with(&image_path)||folders.iter().any(|f|p.starts_with(&f.path)||f.path.starts_with(&p)){
                    return Err("SYSTEM, ordinary media and the selected image must not overlap");
                }
                let folder=it.next().and_then(|s|s.into_string().ok()).ok_or("missing exact UTF-8 folder name")?;
                let mode=match it.next().as_deref().and_then(OsStr::to_str){
                    Some("ro")=>Mode::ReadOnly,Some("rw")=>Mode::ReadWrite,_=>return Err("select the explicit folder mode ro or rw"),
                };
                folders.push(Choice{path:p,folder,mode});
            }else{return Err("only explicit --share/--share-label entries; no recovery, password or second executable");}
        }
        if folders.is_empty(){return Err("select at least one original ordinary folder");}
        let shares=FirstShares{host:hostname,choices:folders,recycle};
        Ok(Self{subject,system,origin,reference,init_sha,image_path,image,crypto,login,shares})
    }
    pub(super) fn run(self,web:bool)->io::Result<(u8,String)>{
        require_private_install()?;
        if web {
            // The browser never holds originals or takes the SYSTEM lease.
            // It can only ask this one foreground instance to show its normal
            // review; the protected originals are checked after the child exits.
            approval::no_inherited_descriptors()?;
            let require=|| approval::host_admin().map(drop);
            require()?;
            let terminal=PublicConfirmation::select()?;
            let count=u8::try_from(self.shares.choices.len()).map_err(|_|bad("invalid fresh share count"))?;
            offer_account_web(&terminal,(self.init_sha,crate::disk_web::InstallPhase::Account,count,self.subject),require)?;
        }
        let Self{subject,system,origin,reference,init_sha,image_path,image,crypto,login,shares}=self;
        // Check host/stdin custody before obtaining any unrelated FD. This
        // genuine low reader holds the original S EX and no root SQL.
        let parent=FreshParent::select(origin.instance)?;
        let resolved=ResolvedRust::fresh(&parent,&system,origin,shares,FreshScope::new(subject,crypto.helper,init_sha))?;
        let tty=PublicConfirmation::select()?;
        super::web_secret::WebWorkspace::check_inherited_lock_budget()?;
        let image_file=image::Image::select(&image_path,&image,parent.product_root()?)?;
        resolved.require_distinct_image(&image_file)?;
        parent.require_distinct_image(&image_file)?;
        let providers=Cryptography::fresh(crypto)?;
        let payload=SitePayload::new(resolved.plan(),subject,&system,image_file.path(),image)?;
        let target=parent.bind(&resolved,payload.clone())?;
        let (pin,_,original_subject)=resolved.protected_new_system()?;
        if original_subject!=subject{return Err(bad("selected original INIT subject changed"));}
        let entity=pin.entity();
        let m=Manifest{intent:Intent::Fresh,nonce:random()?,system:system.clone(),
            entity:[entity.dev,entity.ino,pin.require_same_named_mount()?.id],plan:resolved.plan().clone(),
            order:RustOrder{op:origin.setup,actor:subject.actor,approval:reference,role:Desired::Setup(SetupSpec::new(login)?)},
            authority:Authority::Fresh(CredentialScope{subject,crypto,site:payload.sha()?,init_sha,database:resolved.fresh_database()?})};
        let change=RustChange::for_fresh(&m.plan,&m.order,init_sha)?;
        let spec=change.target();
        let review=preview(&resolved,&payload,&target,&spec,&reference,change.digest(),crypto)?;
        let consent=FreshConsent::approve(&tty,&m,&payload,change,review)?;
        super::web_secret::WebWorkspace::check_inherited_lock_budget()?;
        let deadline=Instant::now()+Duration::from_mins(10);
        let live=||->io::Result<()>{
            if Instant::now()>=deadline{return Err(bad("the original fresh product approval expired"));}
            // The single low secret capture temporarily changes the TTY flags. That
            // stage checks the same original tty through HeldQuiet instead.
            approval::host_admin()?;
            providers.require()?;resolved.require_distinct_image(&image_file)
        };
        let phase=(||->io::Result<_>{
            live()?;tty.check()?;
            let site=filesystem_phase(&live,|guarded|target.prepare(&resolved,&consent,guarded))?;
            let active=transact_fresh(&site,m,consent,&live)?;
            let record=active.at(&site)?.record;
            let site_sha=filesystem_phase(&live,|guarded|target.activate(&resolved,&site,active,&tty,guarded))?;
            Ok((site_sha,record))
        })();
        let(site_sha,record)=phase.map_err(|e|io::Error::new(e.kind(),
            format!("fresh ZXS issuance outcome not confirmed; preserve the exact original INIT, SETUP and all stage/final names; do not retry, adopt or delete: {e}")))?;
        Ok((0,format!("fresh-zxs=confirmed pool={} init={} instance={} setup={} site_sha256={} generation=1 record={}; installed authority only, no service started or login checked",
            hex(&origin.pool),hex(&origin.init),hex(&origin.instance),hex(&origin.setup),hex(&site_sha),hex(&record))))
    }
}
/// One real original-terminal challenge, before creating site or vault
/// material. The same exact manifest and independently lowered public tuple
/// must still agree to use it; no child may ask for another setup approval.
#[must_use]
pub(in crate::sqlite_install::pair) struct FreshConsent<'a>{
    tty:&'a PublicConfirmation,scope:[u8;32],site:[u8;32],action:[u8;32],target:ActiveRustMeta,
}
impl<'a> FreshConsent<'a>{
    fn approve(tty:&'a PublicConfirmation,m:&Manifest,site:&SitePayload,change:RustChange,preview:String)->io::Result<Self>{
        let scope=m.scope()?;let stamp=site.sha()?;let target=change.target();let action=change.digest();
        let Authority::Fresh(f)=m.authority else{return Err(bad("missing exact approved fresh account"));};
        if f.site!=stamp||m.plan.sha()!=site.plan_sha||m.plan.origin!=site.origin
            ||m.system!=site.system||f.subject!=site.subject||f.init_sha!=target.init_sha||change.previous().is_some(){
            return Err(bad("different exact fresh setup preview"));
        }
        tty.confirm_initial_deployment(preview)?;
        Ok(Self{tty,scope,site:stamp,action,target})
    }
    pub(in crate::sqlite_install::pair) fn terminal(&self)->&'a PublicConfirmation{self.tty}
    pub(in crate::sqlite_install::pair) fn require_site(&self,site:&SitePayload)->io::Result<()>{
        if site.sha()?!=self.site{return Err(bad("not this originally approved site"));}self.tty.check()
    }
    pub(in crate::sqlite_install::pair) fn require_manifest(&self,m:&Manifest)->io::Result<()>{
        if m.scope()?!=self.scope{return Err(bad("not this originally approved shared setup"));}self.tty.check()
    }
    pub(in crate::sqlite_install::pair) fn require_observed(&self,m:&Manifest,v:&Observed)->io::Result<()>{
        self.require_manifest(m)?;
        if v.state!=1||v.target!=self.target||v.previous.is_some()||v.action!=self.action{
            return Err(bad("lowered observation differs from the sole original setup consent"));
        }
        Ok(())
    }
}

fn hex(bytes:&[u8])->String{bytes.iter().map(|b|format!("{b:02x}")).collect()}
fn preview(resolved:&ResolvedRust,payload:&SitePayload,target:&FreshHome,shared:&ActiveRustMeta,reference:&Id,action:[u8;32],crypto:CryptoPins)
    ->io::Result<String>{
    let p=resolved.plan();let o=p.origin;let(actor,uid,gid)=payload.subject.tuple();
    let mut out=format!("仅发行从未设置账号的 v21 新 ZXS 池。不会续单、覆盖、清理、迁移，也不会启动任何服务。若中断则保留所有原件，不能盲目重做。\n\
        原池 {} / INIT {} / 原请求 SHA-256 {} / 原主体 {} / 存储 UID:GID {uid}:{gid}。\n\
        新站点 {} / Bind {} / SETUP {} / 批准参考 {}。\n\
        实际受保护的目标 {}。此操作最后独立写入新 deployment.v1；已有文件一律不接管。\n\
        已独立确认的单一 {:?} SHA-256 {}；原描述 SHA-256 {}；新站点记录 SHA-256 {}。\n\
        当前指定 Helper {}、离线 libgcrypt {}、离线 libgpg-error {}；这些不随 ZXS 运行。\n\
        {}本次 ZXS 主机 {}；Web 与 SMB 共用唯一用户名 {}、账户 {}（首代），凭据正本 {}、固定 SID {}。\n\
        共用设置请求 SHA-256 {}；仅此一次批准，之后在本原终端输入和复输同一个口令。\n",
        hex(&o.pool),hex(&o.init),hex(&shared.init_sha),hex(&actor),hex(&o.instance),hex(&o.bind),hex(&o.setup),hex(reference),target.evidence()?,
        payload.image,hex(&payload.elf_sha),hex(&payload.plan_sha),hex(&payload.sha()?),hex(&crypto.helper),hex(&crypto.gcrypt),
        hex(&crypto.gpgerror),resolved.preview(),p.host(),std::str::from_utf8(&shared.login).map_err(|_|bad("invalid shared login"))?,
        hex(&shared.account),hex(&shared.record),hex(&shared.sid),hex(&action));
    out.push_str("  SMB 0.0.0.0:445（固定 IPv4，尚未监听）\n");
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
    fn fresh_deployment_issue_parser_cannot_select_an_old_action_or_credential_path(){
        let form=format!("--init-actor {} --storage-uid 1001 --storage-gid 1002 --system /lab/S --pool {} --init {} \
            --instance {} --bind {} --setup-op {} --init-sha256 {} --approval {} --zxs-image /fixture/bin/zxs --zxs-sha256 {} --helper-sha256 {} --gcrypt-sha256 {} \
            --gpgerror-sha256 {} --host fresh.test --login first.zxs --recycle-name Recycle \
            --share /lab/F Work rw",hex(&[31;16]),hex(&[1;16]),hex(&[2;16]),hex(&[3;16]),
            hex(&[4;16]),hex(&[5;16]),hex(&[45;32]),hex(&[6;16]),hex(&[41;32]),hex(&[42;32]),hex(&[43;32]),hex(&[44;32]));
        let selected=Selection::parse(arg(&form)).unwrap();
        assert_eq!(selected.image_path,PathBuf::from("/fixture/bin/zxs"));
        assert_eq!(selected.system,PathBuf::from("/lab/S"));
        let defaults=form.replace("--system /lab/S ","").replace("--zxs-image /fixture/bin/zxs ","")
            .replace("--share /lab/F","--share-label 工具");
        let selected=Selection::parse(arg(&defaults)).unwrap();
        assert_eq!(selected.system,PathBuf::from(product_layout::SYSTEM));
        assert_eq!(selected.image_path,PathBuf::from(product_layout::IMAGE));
        assert_eq!(selected.shares.choices[0].path,PathBuf::from("/zxs/disks/data/工具"));
        for bad in [format!("{form} --resume"),format!("{form} --password bogus"),format!("{form} --site /other"),
            form.replace("first.zxs","guest"),form.replace("--login first.zxs","--web-login first.web --smb-login first.smb"),form.replace("/lab/F","/lab/S/F"),form.replace("1001","01001"),
            form.replace(&hex(&[3;16]),&hex(&[2;16])),format!("{form} --listen 192.0.2.17/24"),form.replace("--share /lab/F Work rw",""),
            defaults.replace(&format!("--zxs-sha256 {} ",hex(&[41;32])),""),defaults.replace("--share-label 工具","--share-label .."),
            form.replace("/fixture/bin/zxs","/lab/S/zxs"),form.replace("/fixture/bin/zxs","/zxs/state/zxs"),
            form.replace("/fixture/bin/zxs","/lab/F/zxs"),form.replace("/fixture/bin/zxs","/fixture/bin/other"),
            form.replace("/fixture/bin/zxs","zxs"),form.replace(&hex(&[41;32]),&hex(&[0;32]))]{assert!(Selection::parse(arg(&bad)).is_err());}
        let public=format!("issue-fresh {form}");
        let through_web=format!("web issue-fresh {form}");
        assert_eq!(super::super::command::Setup::parse(arg(&through_web)).is_ok(),cfg!(feature="private-fresh-install"));
        for bad in ["web web issue-fresh", "web reset-account", "web", "web issue-fresh --password hidden"] {
            assert!(super::super::command::Setup::parse(arg(bad)).is_err());
        }
        let retired=format!("issue-fresh-zxs-v18 {form}");
        assert!(super::super::command::Setup::parse(arg(&retired)).is_err());
        assert_eq!(super::super::command::Setup::parse(arg(&public)).is_ok(),cfg!(feature="private-fresh-install"));
    }
}

#[cfg(test)]
#[path="../../../tests/sqlite/fresh_terminal.rs"]
mod terminal_tests;

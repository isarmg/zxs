//! A single genuine local launch grant. The first-plan scope, actual published
//! account and selected volume originals predate the authority to listen.
use super::{bad,command::{key,sha,value},domain::NativeSite,native_run::online_roots,recycle::AtStartup,terminal::PublicConfirmation};
use crate::{pinned::{Entity,Kind,Mount,Pinned},smb_profile::{self,first::{Origin,Plan}},sqlite_install::markers::OrdinaryMark};
use rustix::fs::{Mode,OFlags,openat};
use std::{ffi::OsString,fs::File,io,os::unix::fs::MetadataExt,path::{Path,PathBuf}};

#[derive(Debug)]
pub(crate) struct StartSmb{system:PathBuf,origin:Origin,bound_sha:[u8;32]}
impl StartSmb{
    pub(crate) fn parse(mut args:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        let system=PathBuf::from(value(&mut args,"--system")?);
        crate::path_rules::Kind::StorageRoot.check_path(system.as_os_str().as_encoded_bytes())?;
        if system.to_str().is_none(){return Err("SYSTEM is not losslessly representable by SQLite");}
        let pool=key(&mut args,"--pool")?;let init=key(&mut args,"--init")?;
        let instance=key(&mut args,"--instance")?;let domain=key(&mut args,"--domain-op")?;
        let setup=key(&mut args,"--setup-op")?;let bound_sha=sha(&mut args,"--bound-sha256")?;
        if args.next().is_some(){return Err("start-smb takes only the enrolled original site and independent BOUND receipt");}
        Ok(Self{system,origin:Origin{pool,init,instance,domain,setup},bound_sha})
    }
    pub(crate) fn run(self)->io::Result<()>{
        let site=NativeSite::acquire_online(&self.system,self.origin,&self.bound_sha)?;
        let t=PublicConfirmation::select()?;let host=crate::sqlite_install::approval::host_admin()?;
        let (system,system_dir,base,uid)={let (s,d,b,..)=site.online_sources()?;(s,d,b,site.subject().1)};
        let site_path=Path::new("/var/lib/local-ops-helper/samba-instances").join(hex(&self.origin.instance));
        let site_pin=Pinned::open(&site_path,Kind::StorageRoot)?;
        if identity(base)?!=(site_pin.entity(),site_pin.require_same_named_mount()?.id){return Err(no());}
        let claims=online_roots(&site)?;
        let mut folders=Vec::<Volume>::new();
        for c in claims.ordinary{
            let pin=Pinned::open(&c.path,Kind::StorageRoot)?;
            if !separate(&pin,system)||!separate(&pin,&site_pin)||folders.iter().any(|f|!separate(&pin,&f.pin)){return Err(no());}
            let dir=pin.storage_directory()?;
            require_source(&pin,&dir,uid)?;
            let marker=OrdinaryMark::select(&dir,uid,&c.marker)?;
            folders.push(Volume{pin,dir,marker,wanted:c.marker});
        }
        let recycle=AtStartup::assess(&claims.recycle,&host,uid,&site_pin,
            std::iter::once((system,system_dir)).chain(folders.iter().map(|f|(&f.pin,&f.dir))));
        let mut site_env=Online{site,site_pin,folders,recycle,host,terminal:t,bound_sha:self.bound_sha};
        site_env.check_before_attach()?;
        crate::smb_host::foreground(&mut site_env)
    }
}
fn no()->io::Error{bad("the original authorized SMB startup scope could not be established")}
fn hex(bytes:&[u8])->String{bytes.iter().map(|v|format!("{v:02x}")).collect()}
fn separate(a:&Pinned,b:&Pinned)->bool{
    a.entity()!=b.entity()&&!a.entity_is_visible_ancestor_of(b)&&!b.entity_is_visible_ancestor_of(a)
}
fn identity(f:&File)->io::Result<(Entity,u64)>{
    Ok((Entity::of(&f.metadata()?)?,Mount::on(f)?.ok_or_else(no)?.id))
}
fn require_source(pin:&Pinned,dir:&File,uid:u32)->io::Result<()>{
    let m=dir.metadata()?;
    if identity(dir)?!=(pin.entity(),pin.require_same_named_mount()?.id)||m.uid()!=uid||
        m.mode()!=0o040700||m.nlink()==0{return Err(no());}
    Ok(())
}
struct Volume{pin:Pinned,dir:File,marker:OrdinaryMark,wanted:[u8;101]}
/// Private typed authority. No construction from externally supplied FD or
/// release map. The original S shared and site EX OFDs stay owned throughout.
pub(crate) struct Online{site:NativeSite,site_pin:Pinned,folders:Vec<Volume>,recycle:AtStartup,host:File,terminal:PublicConfirmation,bound_sha:[u8;32]}
impl Online{
    pub(crate) fn plan(&self)->&Plan{self.site.first_plan()}
    pub(crate) fn bound_sha(&self)->[u8;32]{self.bound_sha}
    pub(crate) fn live_sha(&self)->[u8;32]{self.site.live_digest()}
    pub(crate) fn subject(&self)->(u32,u32){let (_,u,g)=self.site.subject();(u,g)}
    pub(crate) fn host(&self)->io::Result<(&Pinned,&File,&File,&File,&File)>{self.site.online_sources()}
    pub(crate) fn public_originals(&self)->[&File;2]{self.site.public_pins()}
    pub(crate) fn roots(&self)->impl Iterator<Item=(&Path,&File)>{
        self.folders.iter().map(|v|(v.pin.path(),&v.dir)).chain(self.recycle.original().map(|r|r.source()))
    }
    pub(crate) fn account_parent(&self)->io::Result<File>{
        let (_,_,site,..)=self.host()?;
        let f=File::from(openat(site,c"accountdb",OFlags::DIRECTORY|OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?);
        let m=f.metadata()?;
        if m.mode()!=0o040700||m.uid()!=0||m.gid()!=0||m.nlink()==0||identity(&f)?.1!=identity(site)?.1{return Err(no());}
        Ok(f)
    }
    fn require_ordinary(&self)->io::Result<()>{
        let (s,original,host,..)=self.host()?;
        require_source(s,original,self.subject().0)?;
        if identity(host)?!=(self.site_pin.entity(),self.site_pin.require_same_named_mount()?.id){return Err(no());}
        for v in &self.folders{
            require_source(&v.pin,&v.dir,self.subject().0)?;
            v.marker.verify(&v.dir,&v.wanted)?;
        }
        Ok(())
    }
    fn require_recycle(&self)->io::Result<()>{
        if let Some(r)=self.recycle.original(){
            let (s,original,..)=self.host()?;
            if !r.not_site(&self.site_pin){return Err(no());}
            r.require(&self.host,self.subject().0,
                std::iter::once((s,original)).chain(self.folders.iter().map(|v|(&v.pin,&v.dir))))?;
        }
        Ok(())
    }
    // Before any private mount or child exists we can drop a lost R proof,
    // never adopt a newly appeared volume. After assembly the same originals
    // are required across launch: loss then fails that attempt closed.
    pub(crate) fn check_before_attach(&mut self)->io::Result<()>{
        self.require_ordinary()?;
        if self.require_recycle().is_err(){self.recycle=AtStartup::Unconfirmed;}
        Ok(())
    }
    pub(crate) fn require(&self)->io::Result<()>{self.require_ordinary()?;self.require_recycle()}
    pub(crate) fn approve(&mut self,program_manifest:[u8;32])->io::Result<()>{
        self.check_before_attach()?;
        let text=format!("启动唯一受管 SMB，前台进程停止时收回本次两角色。仅开放以下选定的 LAN，保持来宾关闭；不操作防火墙、Samba 全局服务或其它业务。\n原 BOUND SHA-256 {}；独立核准的程序清单 SHA-256 {}。\n{}{}\n",
            hex(&self.bound_sha),hex(&program_manifest),smb_profile::approval_text(self.plan()),self.recycle.private_status());
        self.terminal.confirm_start(text)?;self.check_before_attach()?;self.terminal.check()
    }
    pub(crate) fn ready(&self)->io::Result<()>{
        self.terminal.report(&format!("smb=listen-confirmed；已核这次原监听与批准清单相符。未测试客户登录或文件读写。\n{}\n",self.recycle.private_status()))
    }
    pub(crate) fn stopped(&self){
        let _=self.terminal.report("smb=original-processes-exited；本次两个原程序均正常退出。非逐会话写入保证。\n");
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn independently_approved_first_site_is_the_only_start_scope(){
        let id=|n:u8|format!("{n:02x}").repeat(16);
        let input=format!("--system /original/S --pool {} --init {} --instance {} --domain-op {} --setup-op {} --bound-sha256 {}",
            id(1),id(2),id(3),id(4),id(5),"06".repeat(32));
        let args=|s:String|s.split_ascii_whitespace().map(OsString::from).collect::<Vec<_>>();
        assert!(StartSmb::parse(args(input.clone()).into_iter()).is_ok());
        for suffix in [" --port 446"," --config /other"," --password nope"," --service /usr/sbin/smbd"]{
            assert!(StartSmb::parse(args(input.clone()+suffix).into_iter()).is_err());
        }
    }
}

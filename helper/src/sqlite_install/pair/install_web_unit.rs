//! Direct local installation of one inactive native Web service fragment.
//! No filesystem takeover, automatic activation, network or browser input.
use super::{Id,bad,command::{key,sha,value},domain::require_lent_ex,inactive_web_unit::{Fragment,Target,UNIT,require_amd64_elf},
    native_run::verify_web_unit,terminal::PublicConfirmation,web_systemd};
use crate::{fs_tools::{RuntimeFile,hash_upto},pinned::{Kind,Pinned},smb_profile,
    sqlite_install::approval};
use rustix::fs::{FlockOperation,flock};
use std::{ffi::OsString,io,os::unix::{ffi::OsStrExt,fs::MetadataExt},path::PathBuf};

const PROGRAM:&[&str]=&["usr","bin","zxs"];
const LIMIT:u64=128<<20;
fn no()->io::Error{bad("the current Web installer, pool or independent executable approval changed")}
fn hex(x:&[u8])->String{x.iter().map(|b|format!("{b:02x}")).collect()}

#[derive(Debug)]
pub(crate) struct InstallWebUnit{system:PathBuf,ids:[Id;3],reference:Id,elf:[u8;32]}
impl InstallWebUnit{
    pub(crate) fn parse(mut a:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        let system=PathBuf::from(value(&mut a,"--system")?);
        Kind::StorageRoot.check_path(system.as_os_str().as_bytes())?;
        let ids=[key(&mut a,"--pool")?,key(&mut a,"--init")?,key(&mut a,"--setup-op")?];
        let reference=key(&mut a,"--approval")?;let elf=sha(&mut a,"--zxs-sha256")?;
        if a.next().is_some(){return Err("only one exact original native Web unit may be installed");}
        Ok(Self{system,ids,reference,elf})
    }
    pub(crate) fn run(self)->io::Result<()> {
        approval::no_inherited_descriptors()?;
        let host=approval::host_admin()?;let who=approval::profile()?;let terminal=PublicConfirmation::select()?;
        let pin=Pinned::open(&self.system,Kind::StorageRoot)?;let original=pin.storage_directory()?;
        flock(&original,FlockOperation::NonBlockingLockExclusive)?;
        let check=||{
            approval::host_admin()?;terminal.check()?;
            let m=original.metadata()?;
            if approval::profile()?!=who||m.uid()!=who.1||m.mode()!=0o040700||m.nlink()==0{return Err(no());}
            require_lent_ex(&original,(pin.entity(),pin.require_same_named_mount()?.id))
        };
        check()?;
        // The only database read is in the original authenticated storage-UID
        // process. This root writer holds the real exclusively locked root,
        // but never opens SQLite, credential files, or business media.
        verify_web_unit(&pin,&original,who,self.ids,&check)?;
        let mut program=RuntimeFile::open(PROGRAM,true,LIMIT)?;let n=program.file.metadata()?.len();
        require_amd64_elf(&program.file)?;
        if hash_upto(&mut program.file,n,LIMIT)?!=self.elf{return Err(no());}
        program.recheck()?;
        let fragment=Fragment::render(pin.path(),&self.ids[0],who.1,who.2)?;
        let target=Target::select(&host)?;target.vacancy()?;
        web_systemd::require_unresolved(&check)?;
        let mut system=String::new();smb_profile::escaped(&mut system,pin.path().as_os_str().as_bytes());
        let review=format!("仅新安装 /etc/systemd/system/{UNIT}，不加载、启停、开机启用或配置网络。未知同名不覆盖。只核目前可见的同名服务，不证明将来无配置覆盖。\n原 SYSTEM：\"{system}\"（dev {} / inode {} / mount {}），存储 UID/GID {}/{}。\n池 {}、原 INIT {}、原首设 {}；本机维护主体 {}、审批参考 {}。\n单元 SHA-256 {}，独立核准的 /usr/bin/zxs SHA-256 {}。\n",
            pin.entity().dev,pin.entity().ino,pin.require_same_named_mount()?.id,who.1,who.2,
            hex(&self.ids[0]),hex(&self.ids[1]),hex(&self.ids[2]),hex(&who.0),hex(&self.reference),hex(&fragment.sha()),hex(&self.elf));
        terminal.confirm_web_unit(review)?;
        verify_web_unit(&pin,&original,who,self.ids,&check)?;
        target.publish(&fragment,||{program.recheck()?;web_systemd::require_unresolved(&check)})
            .map_err(|e|io::Error::new(e.kind(),format!("unit installation not confirmed; preserve any original stage or final name: {e}")))?;
        // A lost display acknowledgement cannot undo an exact durable file.
        let _=terminal.report("web-unit=installed；文件已同步，未请求加载、启动或启用，不代表登录可用。\n");
        Ok(())
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn the_artifact_request_does_not_accept_service_path_or_runtime_options(){
        let args=format!("--system /pool/system --pool {} --init {} --setup-op {} --approval {} --zxs-sha256 {}",
            "01".repeat(16),"02".repeat(16),"03".repeat(16),"04".repeat(16),"15".repeat(32));
        let parse=|s:&str|InstallWebUnit::parse(s.split_whitespace().map(OsString::from));
        assert!(parse(&args).is_ok());
        for tail in [" --unit other.service"," --start"," --exec /other"," --port 8080"]{
            assert!(parse(&format!("{args}{tail}")).is_err());
        }
    }
}

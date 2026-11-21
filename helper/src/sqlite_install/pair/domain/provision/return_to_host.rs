//! Actual root-owned installation hierarchy and exact original directory locks.
mod rust_site;
mod rust_vault_io;
mod site_lease;
mod layout;
mod start;
mod public_record;
mod deployed;
pub(in crate::sqlite_install::pair) use deployed::{DeployedSite,verify_deployed_worker};
pub(in crate::sqlite_install::pair) use rust_vault_io::{Allocated,finish_seen_pending,verify_fresh_vault};
pub(in crate::sqlite_install::pair) use rust_site::{RustSite,VaultStamp,checked_vault_stamp,verify_fresh_worker};
pub(super) use layout::bare_acl;
pub(in crate::sqlite_install::pair) use layout::{FreshHome,FreshParent};
pub(in crate::sqlite_install::pair) use start::{child as request_live_approval,respond as give_live_approval};
use super::{bad,child,contents,mark,live};
use crate::{pinned::Entity,sqlite_install::{approval,request::Id}};
use rustix::{fs::{FlockOperation,Mode,OFlags,fcntl_getfl,flock,openat},io::Errno};
use std::{ffi::CString,fs::File,io::{self,Read},os::{fd::AsRawFd,unix::fs::MetadataExt}};

const PRODUCT:[&std::ffi::CStr;4]=[c"var",c"lib",c"zxs",c"rust-smb"];
fn safe_dir(f:&File)->io::Result<()>{
    let m=f.metadata()?;
    if !m.is_dir()||m.uid()!=0||m.nlink()==0||m.mode()&0o022!=0{return Err(bad("unprotected host instance ancestor"));}Ok(())
}
struct Site{host:File,chain:Vec<File>,leaf:CString}
fn product_pin(parent:&File,name:&std::ffi::CStr,may_cross:bool)->io::Result<File>{
    // The final bare O_PATH does not induce an automount. In particular
    // no dedicated product component may be a separate mounted object.
    use rustix::fs::{openat2,ResolveFlags};
    let mut resolve=ResolveFlags::BENEATH|ResolveFlags::NO_SYMLINKS|ResolveFlags::NO_MAGICLINKS;
    if !may_cross{resolve|=ResolveFlags::NO_XDEV;}
    let f=File::from(openat2(parent,name,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty(),resolve)?);
    safe_dir(&f)?;crate::pinned::local_data_filesystem(&f)?;Ok(f)
}
impl Site{
    fn product(host:File,instance:&Id,missing_final:bool)->io::Result<Self>{
        crate::sqlite_install::pair::compiled_v17()?;
        let leaf=CString::new(instance.iter().map(|b|format!("{b:02x}")).collect::<String>()).expect("lower hex");
        let names=&PRODUCT;
        let mut chain=Vec::with_capacity(names.len()+1);safe_dir(&host)?;
        for (i,name) in names.iter().copied().chain(std::iter::once(leaf.as_c_str())).enumerate(){
            let p=chain.last().unwrap_or(&host);
            let opened=product_pin(p,name,i<2);
            let original=match opened{
                Err(e) if missing_final&&i==names.len()&&e.kind()==io::ErrorKind::NotFound=>break,
                r=>r?,
            };
            safe_dir(&original)?;chain.push(original);
        }
        let this=Self{host,chain,leaf};this.check()?;Ok(this)
    }
    fn prefix(&self)->&'static[&'static std::ffi::CStr;4]{&PRODUCT}
    fn component(&self,parent:&File,name:&std::ffi::CStr,level:usize)->io::Result<File>{product_pin(parent,name,level<2)}
    fn base(&self)->&File{self.chain.last().expect("base retained")}
    fn check(&self)->io::Result<()>{
        self.require_from(approval::host_admin()?)
    }
    // Keep the original root selected before permanently lowering identity.
    // Rewalk from the actual current "/" without administrative privileges.
    fn require_from(&self,host:File)->io::Result<()>{
        safe_dir(&self.host)?;safe_dir(&host)?;
        if mark(&self.host)?!=mark(&host)?{return Err(bad("different current host root"));}
        let mut p=host;
        for (level,(original,n)) in self.chain.iter().zip(self.prefix().iter().copied().chain(std::iter::once(self.leaf.as_c_str()))).enumerate(){
            let next=self.component(&p,n,level)?;safe_dir(original)?;safe_dir(&next)?;
            if mark(original)?!=mark(&next)?{return Err(bad("protected installation path changed"));}
            p=next;
        }Ok(())
    }
}
fn owns_flock(data:&[u8],inode:u64,mount:u64,mode:&str)->bool{
    let Ok(text)=std::str::from_utf8(data)else{return false;};
    if text.len()>4096||!text.ends_with('\n'){return false;}
    let mut got=[false;3];
    for row in text.lines(){
        let Some((key,value))=row.split_once(':')else{return false;};
        let value=value.trim_ascii();let index=match key{
            "mnt_id" if value.parse::<u64>()==Ok(mount)=>0,
            "ino" if value.parse::<u64>()==Ok(inode)=>1,
            "lock"=>{
                let fields:Vec<_>=value.split_ascii_whitespace().collect();
                let [ordinal,"FLOCK","ADVISORY",got_mode,pid,device,"0","EOF"]=fields.as_slice() else{return false;};
                if *ordinal!="1:"||*got_mode!=mode||pid.parse::<i32>().is_err(){return false;}
                let mut dev=device.split(':');
                let hex=|s:Option<&str>|s.is_some_and(|v|!v.is_empty()&&v.len()<=8&&v.bytes().all(|b|b.is_ascii_hexdigit()));
                if !hex(dev.next())||!hex(dev.next())||dev.next().and_then(|n|n.parse::<u64>().ok())!=Some(inode)||dev.next().is_some(){return false;}2
            }
            "mnt_id"|"ino"=>return false,_=>continue,
        };
        if std::mem::replace(&mut got[index],true){return false;}
    }
    got==[true;3]
}
fn require_owned(fd:&File,id:(Entity,u64),mode:&str)->io::Result<()>{
    let path=CString::new(format!("thread-self/fdinfo/{}",fd.as_raw_fd())).expect("decimal descriptor");
    let mut bytes=Vec::new();crate::maintenance::thread_file(&path)?.take(4097).read_to_end(&mut bytes)?;
    if !owns_flock(&bytes,id.0.ino,id.1,mode){return Err(bad("no exact requested whole-file flock on this open description"));}
    Ok(())
}
pub(in crate::sqlite_install) fn lease(fd:&File,id:(Entity,u64))->io::Result<()>{prove_lease(fd,id,true)}
pub(in crate::sqlite_install) fn shared_lease(fd:&File,id:(Entity,u64))->io::Result<()>{prove_lease(fd,id,false)}
fn prove_lease(fd:&File,id:(Entity,u64),exclusive:bool)->io::Result<()>{
    let mode=if exclusive{"WRITE"}else{"READ"};
    let flags=fcntl_getfl(fd)?;
    if flags.contains(OFlags::PATH)||!flags.contains(OFlags::DIRECTORY)||flags&OFlags::ACCMODE!=OFlags::RDONLY
        ||mark(fd)?!=id||!fd.metadata()?.is_dir(){return Err(bad("not the expected original worker lease"));}
    require_owned(fd,id,mode)?;
    // Probe a competing independent OFD; never convert or reacquire the
    // original. A READ loan can never be used as a WRITE loan.
    let other=child(fd,c".")?;
    if mark(&other)?!=id{return Err(bad("lease original changed"));}
    let competing=if exclusive{FlockOperation::NonBlockingLockShared}else{FlockOperation::NonBlockingLockExclusive};
    match flock(&other,competing){
        Err(Errno::WOULDBLOCK)=>require_owned(fd,id,mode),Err(e)=>Err(e.into()),Ok(())=>Err(bad("original lease no longer conflicts")),
    }
}

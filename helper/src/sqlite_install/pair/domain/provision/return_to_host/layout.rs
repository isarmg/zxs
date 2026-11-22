//! Preserve exact protected product ancestors plus nonreplacement publication.
mod initial_deployment;
pub(in crate::sqlite_install::pair) use initial_deployment::{FreshHome,FreshParent};
use super::{PRODUCT,Site,bad,child,contents,mark,product_leaf,product_pin,product_top,safe_dir};
use super::super::Times;
use crate::{maintenance::HostMounts,pinned::Mount,sqlite_install::product_layout};
use rustix::{fs::{Mode,RenameFlags,StatVfsMountFlags,fchmod,fchown,fgetxattr,fstatvfs,mkdirat,renameat_with},
    io::Errno,process::{Gid,Uid}};
use std::{ffi::{CStr,OsStr},fs::File,io::{self,Read},os::unix::{ffi::OsStrExt,fs::MetadataExt}};
const OWNER:Mode=Mode::RWXU;
const INSTANCE:Mode=Mode::from_bits_retain(0o750);
const PUBLIC:Mode=Mode::from_bits_retain(0o444);
fn conflict()->io::Error{io::Error::new(io::ErrorKind::AlreadyExists,"layout or stage already exists; no takeover or automatic retry")}
fn noentry(parent:&File,n:&CStr)->io::Result<()>{
    match product_leaf(parent,n){
        Err(e) if e.kind()==io::ErrorKind::NotFound=>Ok(()),Ok(_)=>Err(conflict()),Err(e)=>Err(e)
    }
}
fn same_name(parent:&File,n:&CStr,original:&File)->io::Result<()>{
    let now=product_leaf(parent,n)?;
    let (a,m)=mark(original)?;let (p,pm)=mark(parent)?;
    if mark(&now)?!=(a,m)||pm!=m||p.dev!=a.dev||p==a{return Err(bad("different original layout name, entity or mount"));}
    Ok(())
}
// Do not infer write/search rights from group bits while a named Linux POSIX
// ACL or inherited default ACL may alter them. Unknown on this host is not
// permission to chown/chmod an existing object.
pub(in crate::sqlite_install::pair::domain::provision) fn bare_acl(f:&File,key:&CStr)->io::Result<()>{
    match fgetxattr(f,key,&mut [0u8;0][..]){
        Err(Errno::NODATA)=>Ok(()),
        Ok(_)|Err(Errno::RANGE)=>Err(bad("extended POSIX ACL is outside the first-install layout profile")),
        Err(e)=>Err(e.into()),
    }
}
// Opening a fixed spelling can reach another spelling on a casefolded FS.
// Read only directory entries on the already-selected parent's own stream;
// never open or return an unrelated entry. The independent lookup must still
// resolve to the held directory (a mount point need not have its dirent ino).
pub(super) fn exact_with(parent:&File,name:&CStr,selected:&File,reopen:impl FnOnce()->io::Result<File>)->io::Result<()>{
    product_layout::raw_member(parent,OsStr::from_bytes(name.to_bytes()),||{
        if mark(&reopen()?)?!=mark(selected)?{return Err(bad("changed child while checking exact layout spelling"));}
        Ok(())
    })
}
fn check_directory(pin:&File,check:impl FnOnce(&File)->io::Result<()>)->io::Result<()>{
    let dir=child(pin,c".")?;
    if mark(&dir)?!=mark(pin)?{return Err(bad("changed original protected directory"));}
    check(&dir)
}
pub(super) fn top_spelling(host:&File,top:&File)->io::Result<()>{
    // The actual host root allows no access ACL; the fixed product root
    // must match the exact same profile as the cold consumer.
    check_directory(host,|f|bare_acl(f,c"system.posix_acl_access"))?;
    exact_with(host,PRODUCT[0],top,||product_top(host))?;
    check_directory(top,protected_control)
}
pub(super) fn state_spelling(host:&File,top:&File,state:&File)->io::Result<()>{
    top_spelling(host,top)?;
    exact_with(top,PRODUCT[1],state,||product_pin(top,PRODUCT[1]))?;
    check_directory(state,protected_control)
}
pub(super) fn protected_control(dir:&File)->io::Result<()>{
    attrs(dir,0,0,Mode::from_bits_retain(product_layout::CONTROL_MODE),true)?;
    bare_acl(dir,c"system.posix_acl_access")?;
    bare_acl(dir,c"system.posix_acl_default")
}
// A whole already mounted /zxs is the sole possible OS mount boundary.
// Keep every original descriptor; no transition at state or an instance.
fn one_mount(host:(crate::pinned::Entity,u64),d:(crate::pinned::Entity,u64))->bool{
    host.0.dev==d.0.dev && host.1==d.1
}
fn host_mount(site:&Site)->io::Result<()>{
    let mut parent=mark(&site.host)?;
    let mut seen=vec![parent.0];
    for (level,d) in site.chain.iter().enumerate(){
        let current=mark(d)?;
        if seen.contains(&current.0){return Err(bad("aliased original instance ancestors"));}
        if !one_mount(parent,current){
            if level!=0||parent.1==current.1{
                return Err(io::Error::new(io::ErrorKind::Unsupported,
                    "a separate mount or device change below /zxs is not supported for protected state"));
            }
            let mount=Mount::on(d)?.filter(|m|m.id==current.1).ok_or_else(||bad("layout mount identity changed"))?;
            HostMounts::current()?.require_product_mount(mount,parent.1,current.0,crate::pinned::local_data_filesystem(d)?)?;
        }
        seen.push(current.0);parent=current;
    }
    Ok(())
}
fn creation_rights(status:&[u8])->bool{
    fn field<'a>(s:&'a[u8],k:&[u8])->Option<&'a str>{
        let mut rows=s.split(|&c|c==b'\n').filter_map(|row|row.strip_prefix(k));
        let x=std::str::from_utf8(rows.next()?).ok()?;(rows.next().is_none()).then_some(x)
    }
    if status.len()>65536||!status.ends_with(b"\n"){return false;}
    let Some(mask)=field(status,b"Umask:\t").filter(|v|v.len()==4&&v.as_bytes()[0]==b'0'&&v.bytes().all(|c|matches!(c,b'0'..=b'7')))
        .and_then(|v|u32::from_str_radix(v,8).ok())else{return false;};
    let Some(cap)=field(status,b"CapEff:\t").filter(|v|v.len()==16&&v.bytes().all(|c|c.is_ascii_hexdigit()))
        .and_then(|v|u64::from_str_radix(v,16).ok())else{return false;};
    // CHOWN, FOWNER and either DAC_OVERRIDE or DAC_READ_SEARCH. Do not
    // change the process mask or capabilities to make a site acceptable.
    mask&0o700==0 && cap&0x9==0x9 && cap&0x6!=0
}
pub(super) fn creation_preflight(parent:&File)->io::Result<()>{
    if parent.metadata()?.mode()&0o2000!=0{return Err(bad("setgid on the immediate installation parent is not supported; not changing it"));}
    let mut status=Vec::new();crate::maintenance::thread_file(c"thread-self/status")?.take(65537).read_to_end(&mut status)?;
    if !creation_rights(&status){return Err(bad("current umask or effective privileges cannot create the exact new layout"));}
    Ok(())
}
fn access_chain(site:&Site,gid:u32)->io::Result<()>{
    site.check()?;host_mount(site)?;
    for path in std::iter::once(&site.host).chain(site.chain.iter()){
        crate::pinned::local_data_filesystem(path)?;
        let dir=child(path,c".")?;if mark(&dir)?!=mark(path)?{return Err(bad("unattributed installation ancestor"));}
        safe_dir(&dir)?;bare_acl(&dir,c"system.posix_acl_access")?;
        let m=dir.metadata()?;let search=if m.gid()==gid{0o010}else{0o001};
        if m.mode()&search==0{return Err(bad("storage subject cannot traverse the protected installation ancestors"));}
    }
    state_spelling(&site.host,&site.chain[0],&site.chain[1])?;
    site.check()
}
pub(super) fn verify_registered_instance(site:&Site, gid:u32)->io::Result<()>{
    if site.chain.len()!=site.prefix().len()+1{return Err(bad("the full original instance path is required"));}
    access_chain(site,gid)?;
    let base=site.base();let m=base.metadata()?;
    if m.gid()!=gid||m.mode()&0o7777!=0o750{return Err(bad("not the protected original instance parent"));}
    let parent=&site.chain[site.prefix().len()-1];
    exact_with(parent,&site.leaf,base,||site.component(parent,&site.leaf,site.prefix().len()))?;
    site.check()
}
fn new_dir(parent:&File,n:&CStr)->io::Result<File>{
    // Only the actual successful creator goes on to open this name. Even an
    // already-empty directory is ineligible, never a candidate for chmod/chown.
    mkdirat(parent,n,OWNER)?;
    let pin=product_pin(parent,n)?;
    let f=child(&pin,c".")?;
    if mark(&pin)?!=mark(&f)?{return Err(bad("changed newly created directory"));}
    same_name(parent,n,&f)?;let m=f.metadata()?;
    if !m.is_dir()||m.uid()!=rustix::process::geteuid().as_raw()||m.nlink()==0||m.mode()&0o7777!=0o700{
        return Err(bad("new temporary directory was not private"));}
    bare_acl(&f,c"system.posix_acl_access")?;bare_acl(&f,c"system.posix_acl_default")?;
    contents(&f,&[])?;Ok(f)
}
fn attrs(f:&File,u:u32,g:u32,mode:Mode,directory:bool)->io::Result<()>{
    let md=f.metadata()?;
    if (if directory{!md.is_dir()||md.nlink()==0}else{!md.is_file()||md.nlink()!=1})
        ||md.uid()!=u||md.gid()!=g||md.mode()&0o7777!=mode.bits(){
        return Err(bad("original layout object does not have its exact owner, group or mode"));}
    Ok(())
}
fn owned(f:&File,u:u32,g:u32,m:Mode)->io::Result<()>{
    fchown(f,Some(Uid::from_raw(u)),Some(Gid::from_raw(g)))?;fchmod(f,m)?;Ok(())
}

fn publish(parent:&File,stage:&CStr,dest:&CStr)->io::Result<()>{
    renameat_with(parent,stage,parent,dest,RenameFlags::NOREPLACE)?;Ok(())
}

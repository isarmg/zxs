//! Protected original image or destination parent. No supplied path may
//! select a program to run here; only our known local mount cut is walked.
use crate::{maintenance::HostMounts,pinned::{DiscoveryAdmission,Entity,Kind,Mount,discover_component,local_data_filesystem},
    sqlite_install::approval};
use super::denied;
use rustix::{fs::{Mode,OFlags,ResolveFlags,fgetxattr,openat2},io::Errno};
use std::{ffi::OsStr,fs::File,io,os::unix::{ffi::OsStrExt,fs::MetadataExt},path::{Path,PathBuf}};

pub(super) const LEAF:&str="zxs";
pub(super) const SITE:&str="/var/lib/zxs/rust-smb";
pub(super) const WITHIN:ResolveFlags=ResolveFlags::BENEATH.union(ResolveFlags::NO_XDEV)
    .union(ResolveFlags::NO_SYMLINKS).union(ResolveFlags::NO_MAGICLINKS);
pub(super) const PIN:OFlags=OFlags::PATH.union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);

fn canonical(path:&Path,folder:bool)->io::Result<()>{
    let raw=path.as_os_str().as_bytes();
    let text=std::str::from_utf8(raw).map_err(|_|denied())?;
    let kind=if folder{Kind::StorageRoot}else{Kind::Regular};
    kind.check_path(raw).map_err(crate::pinned::invalid)?;
    if !(2..=4095).contains(&raw.len())||text.chars().any(char::is_control)
        ||raw[1..].split(|&b|b==b'/').any(|name|name.len()>255)
        ||path.starts_with(SITE)||Path::new(SITE).starts_with(path)
        ||!folder&&path.file_name()!=Some(OsStr::new(LEAF)){
        return Err(denied());
    }
    Ok(())
}
pub(in crate::sqlite_install::pair) fn check_image(path:&Path)->io::Result<()>{canonical(path,false)}
#[cfg(any(test,feature="embedded-zxs"))]
pub(super) fn check_parent(path:&Path)->io::Result<()>{canonical(path,true)}
pub(super) fn no_attribute(f:&File,n:&std::ffi::CStr)->io::Result<()>{
    if matches!(fgetxattr(f,n,&mut [] as &mut [u8]),Err(Errno::NODATA)){Ok(())}else{Err(denied())}
}
#[derive(Clone,Copy,Eq,PartialEq)]
struct Place{entity:Entity,mount:u64}
impl Place{
    fn at(f:&File)->io::Result<Self>{
        Ok(Self{entity:Entity::of(&f.metadata()?)?,mount:Mount::on(f)?.ok_or_else(denied)?.id})
    }
}
struct Prior{original:File,at:Place}
fn qualified_dir(f:&File)->io::Result<File>{
    let m=f.metadata()?;
    if !m.is_dir()||m.uid()!=0||m.nlink()==0||m.mode()&0o022!=0{return Err(denied());}
    let d=File::from(openat2(f,c".",OFlags::RDONLY|OFlags::DIRECTORY|OFlags::CLOEXEC|OFlags::NOFOLLOW|
        OFlags::NOATIME|OFlags::NONBLOCK,Mode::empty(),WITHIN)?);
    if Place::at(&d)?!=Place::at(f)?{return Err(denied());}
    no_attribute(&d,c"system.posix_acl_access")?;no_attribute(&d,c"system.posix_acl_default")?;
    Ok(d)
}
fn walk(path:&Path,is_dir:bool)->io::Result<Vec<Prior>>{
    canonical(path,is_dir)?;
    let home=approval::host_admin()?;
    let table=HostMounts::discovery_view(false)?;
    let initial=table.root_mount()?;
    if table.admit(&home)?!=initial{return Err(denied());}
    local_data_filesystem(&home)?;
    let root=Place::at(&home)?;
    let mut chain=vec![Prior{original:home,at:root}];
    let mut prefix=PathBuf::from("/");
    let mut names=path.as_os_str().as_bytes()[1..].split(|&b|b==b'/').peekable();
    let mut mounts=None;
    while let Some(part)=names.next(){
        let name=OsStr::from_bytes(part);
        let from=chain.last().expect("held host root");
        let opener=qualified_dir(&from.original)?;
        prefix.push(name);
        let crossing=table.crossing(from.at.mount,&prefix)?;
        let last_file=!is_dir&&names.peek().is_none();
        if last_file&&crossing.is_some(){return Err(denied());}
        let opened=discover_component(&opener,name,crossing.is_some())?;
        let expected=crossing.unwrap_or(from.at.mount);
        if table.admit(&opened)?!=expected{return Err(denied());}
        let at=Place::at(&opened)?;
        if at.mount!=expected||chain.iter().any(|f|f.at.entity==at.entity){return Err(denied());}
        if crossing.is_some(){
            if mounts.is_none(){mounts=Some(HostMounts::current()?);}
            let (parent,_)=mounts.as_ref().expect("bounded named mount").full_mount(at.mount)?;
            if parent!=from.at.mount||Mount::on(&opened)?.is_none_or(|m|m.at_root!=Some(true)){return Err(denied());}
        }
        local_data_filesystem(&opened)?;
        if last_file {
            if !opened.metadata()?.is_file(){return Err(denied());}
        }else if !opened.metadata()?.is_dir(){return Err(denied());}
        chain.push(Prior{original:opened,at});
    }
    if is_dir{qualified_dir(&chain.last().expect("selected parent").original)?;}
    Ok(chain)
}

pub(super) struct OriginalPath{path:PathBuf,dir:bool,chain:Vec<Prior>}
impl OriginalPath{
    pub(super) fn selected(path:&Path,dir:bool)->io::Result<Self>{
        Ok(Self{path:path.to_owned(),dir,chain:walk(path,dir)?})
    }
    pub(super) fn require(&self)->io::Result<()>{
        let now=walk(&self.path,self.dir)?;
        if now.len()!=self.chain.len(){return Err(denied());}
        for (owned,named) in self.chain.iter().zip(now){
            if owned.at!=named.at||Place::at(&owned.original)?!=owned.at{return Err(denied());}
        }
        Ok(())
    }
    pub(super) fn pin(&self)->&File{&self.chain.last().expect("held path").original}
    pub(super) fn directory(&self)->io::Result<File>{if !self.dir{return Err(denied());}self.require()?;qualified_dir(self.pin())}
    pub(super) fn path(&self)->&Path{&self.path}
    pub(super) fn includes(&self,entity:Entity)->bool{self.chain.iter().any(|v|v.at.entity==entity)}
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn selected_zxs_path_is_canonical_fixed_leaf_and_outside_durable_site(){
        for good in ["/opt/zxs/bin/zxs","/fresh tools/版本/zxs","/usr/bin/zxs"]{
            assert!(check_image(Path::new(good)).is_ok());
        }
        for bad in ["zxs","/","/opt/ZXS","/opt/other","/opt//zxs","/opt/../zxs","/opt/./zxs","/opt/zxs/",
            "/opt/zxs\n/zxs","/var/lib/zxs/rust-smb/zxs","/var/lib/zxs/rust-smb"]{
            assert!(check_image(Path::new(bad)).is_err(),"{bad:?}");
        }
    }
}

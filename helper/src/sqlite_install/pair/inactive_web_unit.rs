//! One fixed native-HTTP systemd fragment. This module can publish only a
//! freshly rendered unit beneath the real protected /etc/systemd/system.
//! It never loads the unit, starts a service, enables it, or adopts a name.
use super::{Id,bad};
use crate::{path_rules::Kind,pinned::{Entity,Mount,installed_regular_reader}};
use rustix::{fs::{AtFlags,Mode,OFlags,RenameFlags,StatVfsMountFlags,fgetxattr,fstatvfs,openat,renameat_with,statat},io::Errno};
use sha2::{Digest,Sha256};
use std::{ffi::CStr,fs::File,io::{self,Read,Write},os::unix::fs::{FileExt,MetadataExt},path::Path};

pub(super) const UNIT:&str="zxs-management-http.service";
const NAME:&CStr=c"zxs-management-http.service";
const STAGING:&CStr=c".zxs-management-http.service.new";
const ANCESTORS:[&CStr;3]=[c"etc",c"systemd",c"system"];
const DIR:OFlags=OFlags::PATH.union(OFlags::DIRECTORY).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
const INPUT:&[u8]=include_bytes!("../../../resources/web-management/zxs-management-http.service.in");
const MAX_RENDERED:usize=INPUT.len()+4*4095+64;
fn no()->io::Error{bad("cannot establish the exact protected native Web unit")}
fn hex(v:&[u8])->String{v.iter().map(|b|format!("{b:02x}")).collect()}

pub(super) fn require_amd64_elf(file:&File)->io::Result<()> {
    let mut prefix=[0u8;20];file.read_exact_at(&mut prefix,0)?;
    if &prefix[..7]!=b"\x7fELF\x02\x01\x01"||!matches!(prefix[16..20],[2,0,62,0]|[3,0,62,0]){
        return Err(no());
    }
    Ok(())
}

pub(super) struct Fragment(Vec<u8>);
impl Fragment{
    /// Untrusted text cannot introduce a directive, shell or substitution.
    /// The only four values are selected as nonsecret site inputs; actual
    /// system/identity provenance belongs to the calling grant.
    pub(super) fn render(system:&Path,pool:&Id,uid:u32,gid:u32)->io::Result<Self>{
        if *pool==[0;16]||[uid,gid].iter().any(|n|matches!(*n,0|65534|u32::MAX)){return Err(no());}
        let text=system.to_str().ok_or_else(no)?;
        Kind::StorageRoot.check_path(text.as_bytes()).map_err(bad)?;
        if text.len()>=4096||text.as_bytes()[1..].split(|&b|b==b'/').any(|part|part.len()>255)
            ||text.chars().any(|c|{let n=c as u32;c.is_control()||(0xfdd0..=0xfdef).contains(&n)||(n&0xffff)>=0xfffe}){
            return Err(no());
        }
        let quoted=text.replace('\\',"\\\\").replace('"',"\\\"").replace('%',"%%");
        let pool=hex(pool);let uid=uid.to_string();let gid=gid.to_string();
        let template=std::str::from_utf8(INPUT).map_err(|_|no())?;
        let mut bytes=Vec::with_capacity(template.len()+2*quoted.len());
        // Split the signed template itself so a literal @VARIABLE@ in the
        // user's valid SYSTEM pathname is not recursively reinterpreted.
        let mut fields=template.split('@');
        bytes.extend_from_slice(fields.next().ok_or_else(no)?.as_bytes());
        let mut counts=[0;4];
        while let Some(name)=fields.next(){
            let (value,i)=match name {"SYSTEM"=>(quoted.as_str(),0),"POOL"=>(pool.as_str(),1),
                "STORAGE_UID"=>(uid.as_str(),2),"STORAGE_GID"=>(gid.as_str(),3),_=>return Err(no())};
            counts[i]+=1;bytes.extend_from_slice(value.as_bytes());
            bytes.extend_from_slice(fields.next().ok_or_else(no)?.as_bytes());
        }
        if counts!=[2,1,1,1]||bytes.len()>MAX_RENDERED||!bytes.ends_with(b"\n"){return Err(no());}
        Ok(Self(bytes))
    }
    pub(super) fn sha(&self)->[u8;32]{Sha256::digest(&self.0).into()}
}

#[derive(Clone,Copy,Eq,PartialEq,Debug)]
struct Identity{entity:Entity,mount:Mount}
fn identity(f:&File)->io::Result<Identity>{Ok(Identity{entity:Entity::of(&f.metadata()?)?,mount:Mount::on(f)?.ok_or_else(no)?})}
fn protected(f:&File)->io::Result<Identity>{
    let m=f.metadata()?;
    if !m.is_dir()||m.uid()!=0||m.gid()!=0||m.mode()&0o7022!=0||m.nlink()==0{return Err(no());}
    identity(f)
}
fn walk(host:&File)->io::Result<Vec<File>>{
    protected(host)?;let mut parents=vec![host.try_clone()?];
    for name in ANCESTORS{
        let f=File::from(openat(parents.last().ok_or_else(no)?,name,DIR,Mode::empty())?);protected(&f)?;parents.push(f);
    }
    Ok(parents)
}
fn no_entry(parent:&File,name:&CStr)->io::Result<()>{
    match statat(parent,name,AtFlags::NO_AUTOMOUNT|AtFlags::SYMLINK_NOFOLLOW){
        Err(Errno::NOENT)=>Ok(()),Ok(_)=>Err(io::Error::new(io::ErrorKind::AlreadyExists,"fixed service or staging name already exists; not adopted")),Err(e)=>Err(e.into()),
    }
}
fn no_acl(parent:&File)->io::Result<()> {
    let mut unused=[0u8;0];
    for name in [c"system.posix_acl_access",c"system.posix_acl_default"]{
        if !matches!(fgetxattr(parent,name,&mut unused[..]),Err(Errno::NODATA)){return Err(no());}
    }
    Ok(())
}
fn validate_leaf(f:&File,p:&File,mode:u32,n:usize)->io::Result<Identity>{
    let m=f.metadata()?;let id=identity(f)?;
    if m.uid()!=0||m.gid()!=0||m.mode()!=(0o100000|mode)||m.nlink()!=1||m.len()!=n as u64||identity(p)?.mount!=id.mount {
        return Err(no());
    }
    Ok(id)
}
fn matching(parent:&File,name:&CStr,wanted:&Fragment)->io::Result<File>{
    let pin=File::from(openat(parent,name,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?);
    let id=validate_leaf(&pin,parent,0o644,wanted.0.len())?;
    let f=installed_regular_reader(&pin)?;
    if validate_leaf(&f,parent,0o644,wanted.0.len())?!=id{return Err(no());}
    let mut raw=Vec::with_capacity(wanted.0.len()+1);
    (&f).take(wanted.0.len() as u64+1).read_to_end(&mut raw)?;
    let last=File::from(openat(parent,name,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?);
    if raw!=wanted.0||validate_leaf(&f,parent,0o644,wanted.0.len())?!=id||validate_leaf(&last,parent,0o644,wanted.0.len())?!=id{return Err(no());}
    Ok(f)
}

pub(super) struct Target{chain:Vec<File>,parent:File,ids:Vec<Identity>}
impl Target{
    pub(super) fn select(host:&File)->io::Result<Self>{
        let chain=walk(host)?;let ids=chain.iter().map(protected).collect::<io::Result<Vec<_>>>()?;
        let parent=File::from(openat(chain.last().ok_or_else(no)?,c".",OFlags::RDONLY|OFlags::DIRECTORY|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?);
        let this=Self{chain,parent,ids};this.enforce()?;Ok(this)
    }
    fn enforce(&self)->io::Result<()>{
        let current=super::super::approval::host_admin()?;
        let fresh=walk(&current)?;
        if protected(&self.parent)?!=*self.ids.last().ok_or_else(no)?
            ||self.chain.iter().map(protected).collect::<io::Result<Vec<_>>>()?!=self.ids
            ||fresh.iter().map(protected).collect::<io::Result<Vec<_>>>()?!=self.ids
            ||fstatvfs(&self.parent)?.f_flag.contains(StatVfsMountFlags::RDONLY){return Err(no());}
        no_acl(&self.parent)
    }
    pub(super) fn vacancy(&self)->io::Result<()>{self.enforce()?;no_entry(&self.parent,NAME)?;no_entry(&self.parent,STAGING)}
    /// A separate caller grant checks real PID1 unit scope, the original pool
    /// and foreground confirmation. Before rename only it may authorize the
    /// final byte publication. Error leaves any touched names intact.
    pub(super) fn publish(&self,wanted:&Fragment,revalidate:impl Fn()->io::Result<()>)->io::Result<()> {
        revalidate()?;self.vacancy()?;
        let p=&self.parent;
        let opened=File::from(openat(p,STAGING,OFlags::WRONLY|OFlags::CREATE|OFlags::EXCL|OFlags::NOFOLLOW|OFlags::CLOEXEC,
            Mode::RUSR|Mode::WUSR)?);
        let id=validate_leaf(&opened,p,0o600,0)?;
        let mut writer=&opened;writer.write_all(&wanted.0)?;
        rustix::fs::fchmod(&opened,Mode::RUSR|Mode::WUSR|Mode::RGRP|Mode::ROTH)?;
        opened.sync_all()?;
        if identity(&matching(p,STAGING,wanted)?)?!=id{return Err(no());}
        revalidate()?;self.enforce()?;no_entry(p,NAME)?;
        if identity(&matching(p,STAGING,wanted)?)?!=id{return Err(no());}
        renameat_with(p,STAGING,p,NAME,RenameFlags::NOREPLACE)?;
        p.sync_all()?;self.enforce()?;no_entry(p,STAGING)?;
        if identity(&matching(p,NAME,wanted)?)?!=id{return Err(no());}
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic_fixed_native_unit_never_interprets_input_as_a_directive() {
        let p=Path::new("/实际路径/A % & \\\" @POOL@");
        let file=Fragment::render(p,&[0x21;16],2000,2001).unwrap();
        let s=std::str::from_utf8(&file.0).unwrap();
        assert!(s.contains("User=2000\nGroup=2001\n"));
        assert!(s.contains("Environment=ZXS_POOL_ID=21212121212121212121212121212121\n"));
        assert!(s.contains("ExecStart=/usr/bin/zxs\n"));
        assert!(s.contains(r#"%% & \\\" @POOL@"#));
        for input in ["/", "/x/../another", "/bad\nExecStart=other", "/a\u{fdd0}"]{
            assert!(Fragment::render(Path::new(input),&[1;16],2000,2001).is_err());
        }
        assert!(Fragment::render(Path::new("/S"),&[1;16],0,2001).is_err());
    }
}

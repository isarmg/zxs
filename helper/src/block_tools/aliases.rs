//! Optional, explicitly labelled udev aliases. Nothing here reads a block
//! device: the only descriptor which can designate a device node is O_PATH.
use super::*;
use crate::pinned::Mount;
use rustix::fs::{Mode,OFlags,RawDir,ResolveFlags,open,openat2};
use std::{collections::BTreeMap,ffi::OsStr,fs::File,mem::MaybeUninit,
    os::unix::{ffi::OsStrExt,fs::{MetadataExt,FileTypeExt}}};

const LIMIT:usize=512;
fn confined()->ResolveFlags{ResolveFlags::BENEATH|ResolveFlags::NO_XDEV|ResolveFlags::NO_MAGICLINKS}
fn path()->OFlags{OFlags::PATH|OFlags::CLOEXEC}
fn protected(file:&File)->io::Result<()> {
    let m=file.metadata()?;
    if !m.is_dir() || m.uid()!=0 || m.mode()&0o022!=0 {return Err(unsupported("untrusted device-alias directory"));}
    Ok(())
}
fn legal(s:&[u8])->bool{
    !s.is_empty() && s.len()<=64 && s.iter().all(|b|b.is_ascii_hexdigit()||*b==b'-') && s.iter().any(u8::is_ascii_hexdigit)
}
type AliasMap=BTreeMap<u64,Field<String>>;
pub(super) struct Index(Field<AliasMap>);
impl Index {
    pub(super) fn get(&self,d:Dev)->Field<String>{
        self.0.as_ref().map_err(|e|*e)?.get(&d.rdev()).cloned().unwrap_or(Err(Reason::NotReported))
    }
}
pub(super) struct Aliases {pub(super) fs:Index,pub(super) partition:Index}

fn read_one(dev:&File,set:&str)->Field<AliasMap>{
    let root=openat2(dev,c"disk",path()|OFlags::DIRECTORY,Mode::empty(),confined()|ResolveFlags::NO_SYMLINKS)
        .map(File::from).map_err(|e|Reason::io(e.into()))?;
    protected(&root).map_err(Reason::io)?;
    let dir=openat2(&root,set,OFlags::RDONLY|OFlags::DIRECTORY|OFlags::CLOEXEC,Mode::empty(),
        confined()|ResolveFlags::NO_SYMLINKS).map(File::from).map_err(|e|Reason::io(e.into()))?;
    protected(&dir).map_err(Reason::io)?;
    let stamp=|f:&File|->io::Result<_>{let m=f.metadata()?;Ok((m.dev(),m.ino(),m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec()))};
    let original_dir=stamp(&dir).map_err(Reason::io)?;
    let original_disk=stamp(&root).map_err(Reason::io)?;
    let mut buffer=[MaybeUninit::<u8>::uninit();4096];
    let mut entries=RawDir::new(&dir,&mut buffer);
    let(mut count,mut out)=(0,BTreeMap::new());
    while let Some(next)=entries.next(){
        let item=next.map_err(|e|Reason::io(e.into()))?;let name=item.file_name().to_bytes();
        if matches!(name,b"."|b".."){continue;}
        count+=1;if count>LIMIT{return Err(Reason::Limit);}
        if !legal(name){continue;} // unusual identifier: no guess, no output from that name
        let alias=OsStr::from_bytes(name);
        let before=openat2(&dir,alias,path()|OFlags::NOFOLLOW,Mode::empty(),confined())
            .map(File::from).map_err(|e|Reason::io(e.into()))?;
        let m=before.metadata().map_err(Reason::io)?;
        if !m.file_type().is_symlink() || m.uid()!=0 {return Err(Reason::Invalid);}
        let name=std::str::from_utf8(name).map_err(|_|Reason::Invalid)?;
        let link=format!("disk/{set}/{name}");
        let resolve=||openat2(dev,link.as_str(),path(),Mode::empty(),confined()).map(File::from)
            .map_err(|e|Reason::io(e.into()));
        let node=resolve()?;
        let n=node.metadata().map_err(Reason::io)?;
        if !n.file_type().is_block_device() {return Err(Reason::Invalid);}
        let again=openat2(&dir,alias,path()|OFlags::NOFOLLOW,Mode::empty(),confined())
            .map(File::from).map_err(|e|Reason::io(e.into()))?;
        let still=resolve()?.metadata().map_err(Reason::io)?;
        if stamp(&before).map_err(Reason::io)?!=stamp(&again).map_err(Reason::io)?
            || !still.file_type().is_block_device() || (n.dev(),n.ino(),n.rdev())!=(still.dev(),still.ino(),still.rdev()) {
            return Err(Reason::Changed);
        }
        out.entry(n.rdev()).and_modify(|value|*value=Err(Reason::Invalid)).or_insert_with(||Ok(name.to_owned()));
    }
    let again=openat2(dev,format!("disk/{set}").as_str(),path()|OFlags::DIRECTORY,Mode::empty(),
        confined()|ResolveFlags::NO_SYMLINKS).map(File::from).map_err(|e|Reason::io(e.into()))?;
    if stamp(&again).map_err(Reason::io)?!=original_dir || stamp(&dir).map_err(Reason::io)?!=original_dir
        || stamp(&root).map_err(Reason::io)?!=original_disk {return Err(Reason::Changed);}
    Ok(out)
}

pub(super) struct DevRoot {fd:File,entity:Entity,mount:Mount}
impl DevRoot {
    pub(super) fn pin()->Field<Self>{
        let fd=File::from(open(c"/dev",path()|OFlags::DIRECTORY|OFlags::NOFOLLOW,Mode::empty()).map_err(|e|Reason::io(e.into()))?);
        protected(&fd).map_err(Reason::io)?;
        let entity=Entity::of(&fd.metadata().map_err(Reason::io)?).map_err(Reason::io)?;
        let mount=Mount::on(&fd).map_err(Reason::io)?.ok_or(Reason::Unsupported)?;
        if mount.at_root!=Some(true){return Err(Reason::Unsupported);}
        Ok(Self{fd,entity,mount})
    }
    fn still_named(&self)->Field<()>{
        let now=Self::pin()?;
        if (now.entity,now.mount)!=(self.entity,self.mount){return Err(Reason::Changed);}
        Ok(())
    }
}

pub(super) fn read(root:Field<DevRoot>,mounts:Field<&HostMounts>)->Aliases{
    let res=(|| {
        let dev=root?;
        if !mounts?.is_full_devtmpfs(dev.mount.id,dev.entity.dev) {return Err(Reason::Unsupported);}
        let fs=Index(read_one(&dev.fd,"by-uuid"));
        let partition=Index(read_one(&dev.fd,"by-partuuid"));
        dev.still_named()?;
        Ok(Aliases{fs,partition})
    })();
    res.unwrap_or_else(|e|Aliases{fs:Index(Err(e)),partition:Index(Err(e))})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vendor_text_control_characters_and_paths_are_not_uuid_aliases(){
        for s in [&b"02b605ba-13b0-417e-9016-81b518f6e0c1"[..],b"ABCD-1234",b"8a3101a1-02"]{
            assert!(legal(s));
        }
        for s in [&b"../x"[..],b"",b"-",b"x\x1b[0m",b"ab/ab",b"xyz"]{assert!(!legal(s));}
    }
}

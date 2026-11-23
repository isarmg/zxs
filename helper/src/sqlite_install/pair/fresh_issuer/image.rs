//! Approve only the exact already positioned one-ELF ZXS product in the
//! private disposable environment. An ELF header check is not cold acceptance.
mod path;
#[cfg(any(test,feature="embedded-zxs"))]
mod asset;
#[cfg(feature="embedded-zxs")]
pub(crate) use asset::EmbeddedInstall;
pub(in crate::sqlite_install::pair) use path::check_image as check_path;
use crate::{embedded_image::{MAX_IMAGE, denied, standalone}, pinned::{Entity,Mount,installed_regular_reader}};
use path::{OriginalPath,no_attribute};
use rustix::fs::{StatVfsMountFlags,fstatvfs};
use std::{fs::File,io,os::unix::fs::{FileExt,MetadataExt},path::Path};

#[derive(Clone,Copy,Eq,PartialEq)]
struct ImageStamp{entity:Entity,mount:u64,len:u64,ctime:(i64,i64),mtime:(i64,i64)}
impl ImageStamp{
    fn at(f:&File)->io::Result<Self>{
        let m=f.metadata()?;
        if m.mode()!=0o100_755||m.uid()!=0||m.gid()!=0||m.nlink()!=1||
            !(1..=MAX_IMAGE).contains(&m.len()){return Err(denied());}
        Ok(Self{entity:Entity::of(&m)?,mount:Mount::on(f)?.ok_or_else(denied)?.id,len:m.len(),
            ctime:(m.ctime(),m.ctime_nsec()),mtime:(m.mtime(),m.mtime_nsec())})
    }
}
pub(in crate::sqlite_install::pair) struct Image{original:OriginalPath,reader:File,stamp:ImageStamp}
impl Image{
    pub(in crate::sqlite_install::pair) fn select(path:&Path,expected:&[u8;32],product:Entity)->io::Result<Self>{
        if expected==&[0;32]{return Err(denied());}
        let original=OriginalPath::selected(path,false,product)?;
        let stamp=ImageStamp::at(original.pin())?;
        let reader=installed_regular_reader(original.pin())?;
        let owned=Self{original,reader,stamp};
        owned.require()?;
        if crate::fs_tools::hash_upto(&mut &owned.reader,stamp.len,MAX_IMAGE)?!=*expected{return Err(denied());}
        standalone(stamp.len,|offset,body|owned.reader.read_exact_at(body,offset))?;
        owned.require()?;Ok(owned)
    }
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        self.original.require()?;
        for x in [c"security.capability",c"system.posix_acl_access"]{no_attribute(&self.reader,x)?;}
        if fstatvfs(&self.reader)?.f_flag.contains(StatVfsMountFlags::NOEXEC)||
            ImageStamp::at(self.original.pin())?!=self.stamp||ImageStamp::at(&self.reader)?!=self.stamp{
            return Err(denied());
        }
        Ok(())
    }
    pub(in crate::sqlite_install::pair) fn path(&self)->&Path{self.original.path()}
    pub(in crate::sqlite_install::pair) fn includes(&self,entity:Entity)->bool{self.original.includes(entity)}
    pub(in crate::sqlite_install::pair) fn entity(&self)->Entity{self.stamp.entity}
    #[cfg(any(test,feature="embedded-zxs"))]
    fn mount(&self)->u64{self.stamp.mount}
    #[cfg(any(test,feature="embedded-zxs"))]
    fn require_same_published(&self,original:&File)->io::Result<()>{
        self.require()?;
        if ImageStamp::at(original)?!=self.stamp{return Err(denied());}
        Ok(())
    }
}

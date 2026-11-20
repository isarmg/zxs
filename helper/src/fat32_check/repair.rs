//! An opt-in repair of a newly created working image. The selected source is
//! always readonly and never supplied to the writable sandbox.
use super::{ext4,leased::{CopySource,Selected},runner,runtime::Runtime};
use crate::{fs_tools::hex,pinned::{Entity,Kind,Mount,Pinned,invalid,local_data_filesystem,private_inode_reader}};
use helper_linux_evidence::diagnostic_process::cancelled;
use rustix::{
    fs::{Mode,OFlags,RawDir,RenameFlags,StatVfsMountFlags,fcntl_getfl,fstatvfs,mkdirat,openat,renameat_with},
    io::Errno,
    process::{getgid,getuid},
    rand::{GetRandomFlags,getrandom},
};
use std::{ffi::{OsStr,OsString},fs::File,io,mem::MaybeUninit,
    os::unix::{ffi::OsStrExt,fs::{FileExt,MetadataExt}},path::{Path,PathBuf}};

const DIRECTORY:OFlags=OFlags::DIRECTORY.union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
const PIN:OFlags=OFlags::PATH.union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
const IMAGE:&std::ffi::CStr=c"image.ext";
const USAGE:&str="expected repair-ext4-copy ABSOLUTE_IMAGE --sha256 HEX --output ABSOLUTE_NEW_DIR";
fn no()->io::Error{invalid("the selected output name, parent or new private working inode is not exact")}
fn live()->io::Result<()>{if cancelled(){Err(io::ErrorKind::Interrupted.into())}else{Ok(())}}
fn print_sha(v:&[u8;32])->String{v.iter().map(|b|format!("{b:02x}")).collect()}

#[derive(Debug)]
pub(crate) struct Request{source:PathBuf,digest:[u8;32],output:PathBuf}
impl Request{
    pub(crate) fn parse(mut a:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        let source=PathBuf::from(a.next().ok_or(USAGE)?);
        if a.next().as_deref()!=Some(OsStr::new("--sha256")){return Err(USAGE);}
        let digest=a.next().and_then(|v|hex(v.to_str()?)).ok_or(USAGE)?;
        if a.next().as_deref()!=Some(OsStr::new("--output")){return Err(USAGE);}
        let output=PathBuf::from(a.next().ok_or(USAGE)?);
        Kind::Regular.check_path(source.as_os_str().as_bytes())?;
        Kind::StorageRoot.check_path(output.as_os_str().as_bytes())?;
        // The published file must remain usable by the same ordinary-copy
        // checker. Count the fixed leaf as well as the requested directory.
        Kind::Regular.check_path(output.join("image.ext").as_os_str().as_bytes())?;
        if a.next().is_some(){return Err(USAGE);}
        Ok(Self{source,digest,output})
    }

    pub(crate) fn run(self)->io::Result<String>{
        rustix::thread::set_no_new_privs(true)?;
        let uid=getuid().as_raw();
        crate::maintenance::diagnostic_worker(uid,getgid().as_raw())?;
        let original=Selected::select(&self.source)?;
        let dest=Destination::select(self.output,uid)?;
        let length=original.len();
        // No header read, new file or lease while the operator is deciding.
        let runtime=Runtime::ext4()?;
        let preview=format!("只在新工作副本上让 e2fsck 自动修复可安全处理的 ext 问题，再对新副本做一次独立只读检查。不会挂载或修改输入镜像、原磁盘或现有目标。\n输入镜像 {:?}，{length} 字节，外部提供的 SHA-256 {}。\n只有成功复查后才无覆盖地发布 {:?}。预留工作目录 {:?}；空间不足、中断或结果不明时可能留在这两个名字之一，请保留并不要用作已复查镜像。\n",
            self.source,print_sha(&self.digest),dest.output_image(),dest.work_path());
        crate::sqlite_install::confirm_ext_repair(preview)?;
        dest.require_unoccupied()?;
        original.require()?;
        runner::prepare_ext_repair(length)?;
        let original=CopySource::select(original,ext4::read_frame)?;
        runtime.recheck()?;
        // A partial copy is not adopted or cleaned up on paper after a
        // failure. The just-confirmed preview already gave its exact name.
        let result=(||{
            let work=dest.create(length)?;
            let source=original.stream(&self.digest,|at,bytes|work.file.write_all_at(bytes,at))?;
            work.require()?;
            let preen_exit=runner::repair_ext4(&runtime,&work,&source)?;
            let verified_space=work.synced()?;
            source.require()?;
            let (reopened,sha)=ext4::Image::measure_work(verified_space.reader()?,ext4::read_frame)?;
            if runner::check_ext4(&runtime,&reopened)?.exit()!=0{
                return Err(invalid("the work copy did not pass the independent forced readonly recheck"));
            }
            verified_space.publish(&source,&reopened)?;
            Ok(format!("format=ext-family result=working-copy-rechecked source=unmodified-by-helper preen_status={preen_exit} recheck=no-findings-reported image_bytes={length} sha256={} output={:?}",
                print_sha(&sha),dest.output_image()))
        })();
        result.map_err(|e:io::Error|io::Error::new(e.kind(),format!("{e}; no confirmed output; preserve any new work at {:?} or {:?}",dest.work_path(),dest.output)))
    }
}

struct Destination{
    parent:File,pin:Pinned,mount:u64,uid:u32,output:PathBuf,name:OsString,work:OsString,
}
impl Destination{
    fn select(output:PathBuf,uid:u32)->io::Result<Self>{
        let name=output.file_name().ok_or_else(no)?.to_owned();
        let pin=Pinned::open(output.parent().ok_or_else(no)?,Kind::StorageRoot)?;
        let parent=pin.relative_directory()?;
        let mount=pin.require_same_named_mount()?.id;
        let mut slug=[0u8;16];
        if getrandom(&mut slug,GetRandomFlags::NONBLOCK)?!=slug.len()||slug==[0;16]{return Err(no());}
        let work=OsString::from(format!(".zxs-ext-work-{}",slug.iter().map(|b|format!("{b:02x}")).collect::<String>()));
        if name==work{return Err(no());}
        let dest=Self{parent,pin,mount,uid,output,name,work};
        dest.require_unoccupied()?;Ok(dest)
    }
    fn work_path(&self)->PathBuf{self.pin.path().join(&self.work)}
    fn output_image(&self)->PathBuf{self.output.join("image.ext")}
    fn require(&self)->io::Result<()>{
        let m=self.parent.metadata()?;
        if self.uid==0||!m.is_dir()||m.uid()!=self.uid||m.nlink()==0||m.mode()&0o022!=0||
            Entity::of(&m)?!=self.pin.entity()||self.pin.require_same_named_mount()?.id!=self.mount
            ||Mount::on(&self.parent)?.is_none_or(|m|m.id!=self.mount)
            ||fstatvfs(&self.parent)?.f_flag.contains(StatVfsMountFlags::RDONLY){
            return Err(no());
        }
        local_data_filesystem(&self.parent)?;Ok(())
    }
    fn absent(&self,name:&OsStr)->io::Result<()>{
        match openat(&self.parent,name,PIN,Mode::empty()){
            Err(Errno::NOENT)=>Ok(()),Err(e)=>Err(e.into()),Ok(_)=>Err(no()),
        }
    }
    fn require_unoccupied(&self)->io::Result<()>{
        self.require()?;self.absent(&self.name)?;self.absent(&self.work)
    }
    fn dir_entity(&self,f:&File)->io::Result<Entity>{
        let m=f.metadata()?;
        // A parent with setgid may pass it to this new directory; it grants
        // no group access. Neither data nor default ACL may give others rwx.
        if !m.is_dir()||m.uid()!=self.uid||m.nlink()==0||!matches!(m.mode()&0o7777,0o700|0o2700)
            ||Mount::on(f)?.is_none_or(|s|s.id!=self.mount){return Err(no());}
        Entity::of(&m)
    }
    fn create(&self,len:u64)->io::Result<Writable<'_>>{
        live()?;self.require_unoccupied()?;
        mkdirat(&self.parent,&self.work,Mode::RUSR|Mode::WUSR|Mode::XUSR)?;
        let dir=File::from(openat(&self.parent,&self.work,DIRECTORY|OFlags::RDONLY|OFlags::NONBLOCK,Mode::empty())?);
        let id=self.dir_entity(&dir)?;
        self.require()?;
        let f=File::from(openat(&dir,IMAGE,OFlags::CREATE|OFlags::EXCL|OFlags::RDWR|OFlags::NONBLOCK
            |OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::RUSR|Mode::WUSR)?);
        let space=Workspace{dest:self,dir,id,inode:Entity::of(&f.metadata()?)?,len};
        space.require_file(&self.work,&f,0)?;
        Ok(Writable{space,file:f})
    }
}

struct Workspace<'a>{dest:&'a Destination,dir:File,id:Entity,inode:Entity,len:u64}
impl Workspace<'_>{
    fn require_dir(&self,name:&OsStr)->io::Result<()>{
        self.dest.require()?;
        let named=File::from(openat(&self.dest.parent,name,PIN|OFlags::DIRECTORY,Mode::empty())?);
        if self.dest.dir_entity(&named)?!=self.id||self.dest.dir_entity(&self.dir)?!=self.id{return Err(no());}
        Ok(())
    }
    fn validate(&self,f:&File,len:u64)->io::Result<()>{
        let m=f.metadata()?;
        if !m.is_file()||m.uid()!=self.dest.uid||m.nlink()!=1||m.mode()&0o7777!=0o600||m.len()!=len
            ||Mount::on(f)?.is_none_or(|m|m.id!=self.dest.mount)||Entity::of(&m)?!=self.inode{
            return Err(no());
        }Ok(())
    }
    fn selected(&self,name:&OsStr,len:u64)->io::Result<File>{
        self.require_dir(name)?;
        let dir=File::from(openat(&self.dir,c".",DIRECTORY|OFlags::RDONLY|OFlags::NONBLOCK,Mode::empty())?);
        if self.dest.dir_entity(&dir)?!=self.id{return Err(no());}
        let mut buffer=[MaybeUninit::uninit();4096];let mut cursor=RawDir::new(&dir,&mut buffer);
        let mut seen=false;
        while let Some(entry)=cursor.next(){
            let entry=entry?;
            if matches!(entry.file_name().to_bytes(),b"."|b".."){continue;}
            if entry.file_name()!=IMAGE||std::mem::replace(&mut seen,true){return Err(no());}
        }
        if !seen{return Err(no());}
        let f=File::from(openat(&self.dir,IMAGE,PIN,Mode::empty())?);
        self.validate(&f,len)?;self.require_dir(name)?;Ok(f)
    }
    fn require_file(&self,name:&OsStr,f:&File,len:u64)->io::Result<()>{
        let named=self.selected(name,len)?;self.validate(&named,len)?;self.validate(f,len)
    }
    fn reader(&self)->io::Result<File>{
        let pin=self.selected(&self.dest.work,self.len)?;
        let file=private_inode_reader(&pin,self.dest.uid,self.dest.mount)?;
        self.require_file(&self.dest.work,&file,self.len)?;Ok(file)
    }
    fn publish(&self,source:&ext4::Image,work:&ext4::Image)->io::Result<()>{
        let d=self.dest;
        let ready=||{
            live()?;source.require()?;work.require()?;
            self.require_file(&d.work,work.file(),self.len)?;
            d.absent(&d.name)
        };
        ready()?;self.dir.sync_all()?;d.parent.sync_all()?;
        ready()?;
        renameat_with(&d.parent,&d.work,&d.parent,&d.name,RenameFlags::NOREPLACE)?;
        d.parent.sync_all()?;
        live()?;d.absent(&d.work)?;
        self.require_file(&d.name,work.file(),self.len)?;
        work.require()?;source.require()
    }
}

pub(super) struct Writable<'a>{space:Workspace<'a>,file:File}
impl<'a> Writable<'a>{
    pub(super) fn file(&self)->&File{&self.file}
    pub(super) fn length(&self)->u64{self.space.len}
    pub(super) fn require(&self)->io::Result<()>{
        let flags=fcntl_getfl(&self.file)?;
        if flags.contains(OFlags::PATH)||flags&OFlags::ACCMODE!=OFlags::RDWR{return Err(no());}
        self.space.require_file(&self.space.dest.work,&self.file,self.space.len)
    }
    fn synced(self)->io::Result<Workspace<'a>>{
        self.require()?;self.file.sync_all()?;self.require()?;
        let Self{space,file}=self;drop(file);Ok(space)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn the_output_leaf_is_still_a_supported_input_at_the_path_boundaries(){
        let sha="b0".repeat(32);
        let parse=|output:String|Request::parse([OsString::from("/saved/ext.img"),"--sha256".into(),
            OsString::from(sha.as_str()),"--output".into(),output.into()].into_iter());
        let max=crate::path_rules::MAX_PATH_BYTES;
        assert!(parse("/x".repeat(127)).is_ok());
        assert!(parse("/x".repeat(128)).is_err());
        // Nonempty, ordinary-sized components; the mask concerns total
        // path bytes, and counts the fixed /image.ext on both routes.
        let parent=format!("{}/abcdef",format!("/{}","a".repeat(254)).repeat(15));
        let more=max-"/image.ext".len()-parent.len();
        assert!(parse(format!("{parent}/{}","a".repeat(more-1))).is_ok());
        assert!(parse(format!("{parent}/{}","a".repeat(more))).is_err());
    }
    #[test]
    fn one_new_explicit_work_directory_not_any_upstream_option(){
        let sha="b0".repeat(32);
        let parse=|s:&str|Request::parse(s.split(' ').map(OsString::from));
        let approved=format!("/saved/ext.img --sha256 {sha} --output /work/new");
        assert!(parse(&approved).is_ok());
        for bad in [format!("{approved} -y"),format!("{approved} --output /other"),
            format!("/saved/ext.img --sha256 {sha}"),approved.replace("/work/new","/"),
            approved.replace("/work/new","/work/../new"),approved.replace("/saved/ext.img","ext.img")]{
            assert!(parse(&bad).is_err());
        }
    }
}

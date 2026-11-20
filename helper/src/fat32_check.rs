//! Locally selected FAT32/exFAT/ext4 mirror checks. No repairs.
mod exfat;
mod ext4;
mod group;
mod image;
pub(crate) mod panel;
mod runner;
mod runtime;
pub(crate) use helper_linux_evidence::diagnostic_process::CopyFormat as Format;
use crate::fs_tools::hex;
use std::{ffi::OsString,fmt,io,path::{Path,PathBuf}};

/// The same held eight-file host installation as the offline checker; this
/// wrapper cannot start it or expose a checker, library or arbitrary action.
pub(crate) struct SamLauncher(runtime::Runtime,[u8;32]);
impl SamLauncher{
    pub(crate) fn for_release(expected:&[u8;32])->io::Result<Self>{
        let (r,b)=runtime::Runtime::open_for(Some(expected))?;Ok(Self(r,b))
    }
    pub(crate) fn sha(&self)->[u8;32]{self.1}
    pub(crate) fn require(&self)->io::Result<()>{self.0.recheck()}
}

#[derive(Debug)]
pub(crate) struct Request { image:PathBuf, digest:[u8;32] }
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub(crate) enum Outcome {NoFindingsReported,ReportedMessages,FindingsOrToolError}
pub(crate) struct Report {pub(crate) state:Outcome,pub(crate) files:u32,pub(crate) used:u32,pub(crate) total:u32}
impl Report {
    pub(crate) fn exit(&self)->u8 {if self.state==Outcome::NoFindingsReported {0}else{1}}
}
impl fmt::Display for Report {
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {
        let result=match self.state{
            Outcome::NoFindingsReported=>"no-findings-reported",
            Outcome::ReportedMessages=>"completed-with-additional-messages",
            Outcome::FindingsOrToolError=>"findings-or-checker-inconsistency",
        };
        write!(f,"format=FAT32 checker=dosfstools-4.2 check=-n operand=independent-sealed-image repair=none result={result} files={} used_clusters={} total_clusters={}",self.files,self.used,self.total)
    }
}
impl Request {
    pub(crate) fn parse(mut args:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        Self::parse_image(&mut args,"expected one absolute saved FAT32 volume image and --sha256 HEX; no devices or checker options")
    }
    pub(crate) fn parse_exfat(mut args:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        Self::parse_image(&mut args,"expected one absolute saved exFAT volume image and --sha256 HEX; no devices or checker options")
    }
    pub(crate) fn parse_ext4(mut args:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        Self::parse_image(&mut args,"expected one absolute saved ext4 volume image and --sha256 HEX; no devices or checker options")
    }
    fn parse_image(args:&mut impl Iterator<Item=OsString>,usage:&'static str)->Result<Self,&'static str>{
        let image=args.next().ok_or(usage)?;
        if !Path::new(&image).is_absolute()||args.next().as_deref()!=Some(std::ffi::OsStr::new("--sha256")){return Err(usage);}
        let digest=args.next().and_then(|s|hex(s.to_str()?)).ok_or(usage)?;
        if args.next().is_some(){return Err(usage);}
        Ok(Self{image:image.into(),digest})
    }
    pub(crate) fn run_ext4(self)->io::Result<ext4::Report>{
        runner::prepare_ext4_worker()?;
        let runtime=runtime::Runtime::ext4()?;
        let image=ext4::Image::capture(&self.image,&self.digest)?;
        runner::check_ext4(&runtime,&image)
    }
    pub(crate) fn run_exfat(self)->io::Result<exfat::Report>{
        runner::prepare_worker()?;
        let runtime=runtime::Runtime::exfat()?;
        let image=image::ExfatSealed::capture(&self.image,&self.digest)?;
        runner::check_exfat(&runtime,&image)
    }
    pub(crate) fn run(self)->io::Result<Report>{
        runner::prepare_worker()?;
        let runtime=runtime::Runtime::open()?;
        let image=image::Sealed::capture(&self.image,&self.digest)?;
        runner::check(&runtime,&image)
    }
}

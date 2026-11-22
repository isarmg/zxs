//! Isolated saved-volume diagnostics and separately approved work-copy repairs.
mod btrfs;
mod exfat;
mod ext4;
mod group;
mod image;
mod leased;
mod ntfs;
pub(crate) mod panel;
mod runner;
mod runtime;
pub(crate) mod repair;
mod xfs;
pub(crate) use helper_linux_evidence::diagnostic_process::{CheckFormat,CopyFormat as Format,LeasedFormat};
use crate::fs_tools::hex;
use std::{ffi::{OsStr,OsString},fmt,io,path::{Path,PathBuf}};

#[derive(Debug)]
pub(crate) struct Request { format:CheckFormat, image:PathBuf, digest:[u8;32] }
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
// The terminal command and the terminal's panel option share one exact set
// of readonly choices. No format can be selected by an HTTP request.
fn select_format(token: &OsStr, prefix: &str) -> Option<CheckFormat> {
    let stem = token.to_str()?.strip_prefix(prefix)?.strip_suffix("-copy")?;
    Some(match stem {
        "fat32" => CheckFormat::Sealed(Format::Fat32),
        "exfat" => CheckFormat::Sealed(Format::Exfat),
        "ext4" => CheckFormat::Leased(LeasedFormat::Ext4),
        "xfs" => CheckFormat::Leased(LeasedFormat::Xfs),
        "ntfs" => CheckFormat::Leased(LeasedFormat::Ntfs),
        "btrfs" => CheckFormat::Leased(LeasedFormat::Btrfs),
        _ => return None,
    })
}

pub(crate) fn command_format(command: &OsStr) -> Option<CheckFormat> {
    select_format(command, "check-")
}

pub(crate) fn panel_format(option: &OsStr) -> Option<CheckFormat> {
    select_format(option, "--")
}

impl Request {
    pub(crate) fn parse(format: CheckFormat, mut args: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        let usage = match format {
            CheckFormat::Sealed(Format::Fat32) => "expected one absolute saved FAT32 volume image and --sha256 HEX; no devices or checker options",
            CheckFormat::Sealed(Format::Exfat) => "expected one absolute saved exFAT volume image and --sha256 HEX; no devices or checker options",
            CheckFormat::Leased(LeasedFormat::Ext4) => "expected one absolute saved ext4 volume image and --sha256 HEX; no devices or checker options",
            CheckFormat::Leased(LeasedFormat::Xfs) => "expected one absolute saved XFS volume image and --sha256 HEX; no devices or checker options",
            CheckFormat::Leased(LeasedFormat::Ntfs) => "expected one absolute saved NTFS volume image and --sha256 HEX; no devices or checker options",
            CheckFormat::Leased(LeasedFormat::Btrfs) => "expected one absolute saved single-device Btrfs image and --sha256 HEX; no devices or checker options",
        };
        let image = args.next().ok_or(usage)?;
        if !Path::new(&image).is_absolute() || args.next().as_deref() != Some(OsStr::new("--sha256")) {
            return Err(usage);
        }
        let digest = args.next().and_then(|s| hex(s.to_str()?)).ok_or(usage)?;
        if args.next().is_some() { return Err(usage); }
        Ok(Self { format, image: image.into(), digest })
    }

    pub(crate) fn format(&self) -> CheckFormat { self.format }

    pub(crate) fn run(self) -> io::Result<(u8, String)> {
        runner::prepare_check(self.format)?;
        // Release the original runtime and image (including any lease)
        // before preparing console output. The panel has its own receipt path.
        Ok(match self.format {
            CheckFormat::Sealed(Format::Fat32) => {
                let report = {
                    let runtime = runtime::Runtime::open()?;
                    let image = image::Sealed::capture(&self.image, &self.digest)?;
                    runner::check(&runtime, &image)?
                };
                (report.exit(), report.to_string())
            }
            CheckFormat::Sealed(Format::Exfat) => {
                let report = {
                    let runtime = runtime::Runtime::exfat()?;
                    let image = image::ExfatSealed::capture(&self.image, &self.digest)?;
                    runner::check_exfat(&runtime, &image)?
                };
                (report.exit(), report.to_string())
            }
            CheckFormat::Leased(LeasedFormat::Ext4) => {
                let report = {
                    let runtime = runtime::Runtime::ext4()?;
                    let image = ext4::Image::capture(&self.image, &self.digest, ext4::read_frame)?;
                    runner::check_ext4(&runtime, &image)?
                };
                (report.exit(), report.to_string())
            }
            CheckFormat::Leased(LeasedFormat::Xfs) => {
                let report = {
                    let runtime = runtime::Runtime::xfs()?;
                    let image = xfs::Image::capture(&self.image, &self.digest, xfs::read_frame)?;
                    runner::check_xfs(&runtime, &image)?
                };
                (report.exit(), report.to_string())
            }
            CheckFormat::Leased(LeasedFormat::Ntfs) => {
                let report = {
                    let runtime = runtime::Runtime::ntfs()?;
                    let image = ntfs::Image::capture(&self.image, &self.digest, ntfs::read_frame)?;
                    runner::observe_ntfs(&runtime, &image)?
                };
                (report.exit(), report.to_string())
            }
            CheckFormat::Leased(LeasedFormat::Btrfs) => {
                let report = {
                    let runtime = runtime::Runtime::btrfs()?;
                    let image = btrfs::Image::capture(&self.image, &self.digest, btrfs::read_frame)?;
                    runner::check_btrfs(&runtime, &image)?
                };
                (report.exit(), report.to_string())
            }
        })
    }
}

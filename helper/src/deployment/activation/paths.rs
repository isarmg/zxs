use crate::Result;
use data_serialization::Serialize;
use platform_io::fs_safety::{
    RelativePath, linux::{FileIdentity, MountPolicy, OpenAt2Root},
};
use std::{os::unix::fs::MetadataExt, path::Path};

#[derive(Clone, PartialEq, Eq, Serialize)]
pub(super) struct Directory {
    path: &'static str,
    device: String,
    inode: String,
    uid: u32,
    gid: u32,
    mode: u32,
}
pub(super) struct Paths {
    host: OpenAt2Root,
    owner: (u32, u32),
    smb: bool,
    directories: Vec<Directory>,
}
impl Paths {
    pub fn open(owner: (u32, u32), smb: bool) -> Result<Self> {
        let host = OpenAt2Root::open(Path::new("/zxs"), MountPolicy::AllowNestedMounts)?;
        let mut result = Self { host, owner, smb, directories: Vec::new() };
        result.directories = result.observe()?;
        Ok(result)
    }
    pub fn identity(&self) -> FileIdentity { self.host.identity() }
    pub fn snapshot(&self) -> &[Directory] { &self.directories }
    pub fn recheck(&self) -> Result<()> {
        if self.observe()? != self.directories {
            return Err("activation directories changed identity or ownership; no replacement was accepted".into());
        }
        Ok(())
    }
    fn observe(&self) -> Result<Vec<Directory>> {
        let current = OpenAt2Root::open(Path::new("/zxs"), MountPolicy::AllowNestedMounts)?;
        let meta = current.directory().metadata()?;
        if current.identity() != self.host.identity()
            || meta.uid() != 0 || meta.gid() != 0 || meta.mode() & 0o7777 != 0o755
        {
            return Err("activation requires the original root-owned /zxs mode 0755; no paths or permissions were repaired".into());
        }
        let mut result = Vec::new();
        for relative in ["cache", "config", "home", "log", "run", "tmp",
            "data", "data/state", "data/management", "data/disks", "smb-export"] {
            if relative == "smb-export" && !self.smb { continue; }
            let directory = OpenAt2Root::from_directory(
                current.open_directory(&RelativePath::new(relative)?)?, MountPolicy::AllowNestedMounts)?;
            let meta = directory.directory().metadata()?;
            let export = relative == "smb-export";
            let owner = if export { (0, 0) } else { self.owner };
            let mode = if export { 0o755 } else { 0o700 };
            if (meta.uid(), meta.gid()) != owner || meta.mode() & 0o7777 != mode {
                return Err(format!("activation requires the already prepared owner/mode of /zxs/{relative}; it was not changed").into());
            }
            if export && directory.entries()?.next().transpose()?.is_some() {
                return Err("the protected SMB export must remain empty; no user data was cleared".into());
            }
            result.push(Directory { path: relative, device: meta.dev().to_string(),
                inode: meta.ino().to_string(), uid: meta.uid(), gid: meta.gid(), mode });
        }
        Ok(result)
    }
}

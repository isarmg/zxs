use super::write_atomic_at_mode;
use crate::Result;
use data_serialization::Serialize;
use platform_io::fs_safety::{
    EntryName, Error, RelativePath,
    linux::{AdvisoryLock, FileIdentity, MountPolicy, OpenAt2Root, SingleLinkRequirement},
};
use std::{fs::File, io::Read, os::unix::fs::MetadataExt, path::Path};

pub(crate) const SYSTEMD_PATH: &str = "/etc/systemd/system/zxs.service";
pub(crate) const SAMBA_PATH: &str = "/etc/samba/zxs.conf";
const MAX_CONFIG_BYTES: usize = 64 * 1024;

#[derive(Serialize)]
pub(crate) struct ParentSnapshot {
    path: &'static str,
    device: String,
    inode: String,
}
struct Parent {
    path: &'static str,
    root: OpenAt2Root,
    _lock: AdvisoryLock,
}
impl Parent {
    fn open(path: &'static str, root: OpenAt2Root) -> Result<Self> {
        trusted(root.directory())?;
        let lock = AdvisoryLock::directory(root.directory())?;
        Ok(Self {
            path,
            root,
            _lock: lock,
        })
    }
    fn child(&self, name: &str, path: &'static str) -> Result<Self> {
        let file = self.root.open_directory(&RelativePath::new(name)?)?;
        Self::open(
            path,
            OpenAt2Root::from_directory(file, MountPolicy::AllowNestedMounts)?,
        )
    }
    fn snapshot(&self) -> ParentSnapshot {
        ParentSnapshot {
            path: self.path,
            device: self.root.identity().device.to_string(),
            inode: self.root.identity().inode.to_string(),
        }
    }
    fn recheck(&self) -> Result<()> {
        let current = OpenAt2Root::open(Path::new(self.path), MountPolicy::AllowNestedMounts)?;
        trusted(current.directory())?;
        if current.identity() != self.root.identity() {
            return Err(format!("configuration parent {} changed identity; retain the original publication evidence", self.path).into());
        }
        Ok(())
    }
}
pub(crate) struct ConfigurationFiles {
    etc: Parent,
    systemd: Parent,
    units: Parent,
    samba: Option<Parent>,
}
impl ConfigurationFiles {
    pub(crate) fn open(include_samba: bool) -> Result<Self> {
        let etc = Parent::open(
            "/etc",
            OpenAt2Root::open(Path::new("/etc"), MountPolicy::AllowNestedMounts)?,
        )?;
        let systemd = etc.child("systemd", "/etc/systemd")?;
        let units = systemd.child("system", "/etc/systemd/system")?;
        let samba = if include_samba {
            Some(etc.child("samba", "/etc/samba")?)
        } else {
            None
        };
        let result = Self {
            etc,
            systemd,
            units,
            samba,
        };
        result.recheck()?;
        Ok(result)
    }
    pub(crate) fn parents(&self) -> Vec<ParentSnapshot> {
        let mut result = vec![
            self.etc.snapshot(),
            self.systemd.snapshot(),
            self.units.snapshot(),
        ];
        if let Some(samba) = &self.samba {
            result.push(samba.snapshot());
        }
        result
    }
    pub(crate) fn recheck(&self) -> Result<()> {
        self.etc.recheck()?;
        self.systemd.recheck()?;
        self.units.recheck()?;
        if let Some(samba) = &self.samba {
            samba.recheck()?;
        }
        Ok(())
    }
    pub(crate) fn inspect(&self, unit: &[u8], samba: Option<&[u8]>) -> Result<Vec<bool>> {
        self.recheck()?;
        let mut present = vec![matching(&self.units.root, "zxs.service", unit)?.is_some()];
        match (&self.samba, samba) {
            (Some(parent), Some(bytes)) => {
                present.push(matching(&parent.root, "zxs.conf", bytes)?.is_some())
            }
            (None, None) => {}
            _ => return Err("configuration set differs from its admitted target parents".into()),
        }
        self.recheck()?;
        Ok(present)
    }
    pub(crate) fn inspect_smb_unit(&self, bytes: &[u8]) -> Result<bool> {
        self.recheck()?;
        let present = matching(&self.units.root, "zxs-smb.service", bytes)?.is_some();
        self.recheck()?;
        Ok(present)
    }
    pub(crate) fn install_smb_unit(&self, bytes: &[u8]) -> Result<bool> {
        self.recheck()?;
        let created = publish(&self.units.root, "zxs-smb.service", bytes)?;
        self.recheck()?;
        Ok(created)
    }
    pub(crate) fn install_unit(&self, bytes: &[u8]) -> Result<bool> {
        self.recheck()?;
        let created = publish(&self.units.root, "zxs.service", bytes)?;
        self.recheck()?;
        Ok(created)
    }
    pub(crate) fn install_samba(&self, bytes: &[u8]) -> Result<bool> {
        self.recheck()?;
        let parent = self
            .samba
            .as_ref()
            .ok_or("no Samba destination was admitted")?;
        let created = publish(&parent.root, "zxs.conf", bytes)?;
        self.recheck()?;
        Ok(created)
    }
}
fn trusted(file: &File) -> Result<()> {
    let metadata = file.metadata()?;
    if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err("configuration parent must be root-owned and not writable by other users/groups; no permissions were changed".into());
    }
    Ok(())
}
fn matching(root: &OpenAt2Root, name: &str, bytes: &[u8]) -> Result<Option<File>> {
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err("configuration exceeds its byte budget".into());
    }
    let name = EntryName::new(name)?;
    let file = match root.open_file(&name.as_relative(), false) {
        Ok(file) => file,
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if metadata.uid() != 0
        || metadata.mode() & 0o7777 != 0o644
        || metadata.len() != bytes.len() as u64
    {
        return Err(format!("existing configuration {:?} has different ownership, mode or size; it was not overwritten", name.as_os_str()).into());
    }
    let identity = SingleLinkRequirement::check(&file)?;
    let mut actual = Vec::new();
    (&file)
        .take(bytes.len() as u64 + 1)
        .read_to_end(&mut actual)?;
    if actual != bytes {
        return Err(format!("existing configuration {:?} differs; replacement is not implemented and all bytes are preserved", name.as_os_str()).into());
    }
    check_named(root, &name, identity)?;
    Ok(Some(file))
}
fn check_named(root: &OpenAt2Root, name: &EntryName, identity: FileIdentity) -> Result<()> {
    if root.identity_of(&name.as_relative())? != identity {
        return Err("configuration identity changed during inspection".into());
    }
    Ok(())
}
fn publish(root: &OpenAt2Root, name: &str, bytes: &[u8]) -> Result<bool> {
    let created = match matching(root, name, bytes)? {
        Some(file) => {
            file.sync_all()?;
            false
        }
        None => {
            write_atomic_at_mode(root, &EntryName::new(name)?, bytes, false, 0o644)?;
            true
        }
    };
    root.directory().sync_all()?;
    let file = matching(root, name, bytes)?.ok_or("published configuration disappeared")?;
    file.sync_all()?;
    root.directory().sync_all()?;
    Ok(created)
}

#[cfg(test)]
#[path = "../../../test/helper/unit/private_io/configuration_files_tests.rs"]
mod tests;

//! New fixed configuration files only. The parent descriptors and directory
//! locks outlive every read, no-clobber rename and durability barrier.
use super::write_atomic_at_mode_receipt;
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
/// Lock order: host lease, program parent, /etc, systemd, system, samba.
/// External administrators must remain stopped while these advisory locks are held.
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
    pub(crate) fn capture(
        &self,
        unit: &[u8],
        samba: Option<&[u8]>,
    ) -> Result<ConfigurationCapture<'_>> {
        self.recheck()?;
        let unit = matching(&self.units.root, "zxs.service", unit)?;
        let samba = match (&self.samba, samba) {
            (Some(parent), Some(bytes)) => matching(&parent.root, "zxs.conf", bytes)?,
            (None, None) => None,
            _ => return Err("configuration set differs from its admitted target parents".into()),
        };
        let captured = ConfigurationCapture { unit, samba };
        self.recheck()?;
        captured.require_current()?;
        Ok(captured)
    }
    pub(crate) fn inspect(&self, unit: &[u8], samba: Option<&[u8]>) -> Result<Vec<bool>> {
        let captured = self.capture(unit, samba)?;
        let mut present = vec![captured.unit.is_some()];
        if self.samba.is_some() {
            present.push(captured.samba.is_some());
        }
        Ok(present)
    }
    /// The dedicated Samba service is installed only by explicit activation.
    /// Configuration-file installation covers the unit and optional Samba config, not activation.
    pub(crate) fn inspect_smb_unit(&self, bytes: &[u8]) -> Result<bool> {
        self.recheck()?;
        let present = matching(&self.units.root, "zxs-smb.service", bytes)?.is_some();
        self.recheck()?;
        Ok(present)
    }
    pub(crate) fn install_smb_unit(&self, bytes: &[u8]) -> Result<bool> {
        self.recheck()?;
        let held = publish(&self.units.root, "zxs-smb.service", bytes, false)?;
        self.recheck()?;
        held.require_current()?;
        Ok(held.created)
    }
    pub(crate) fn install_unit<'a>(
        &'a self,
        bytes: &[u8],
        original: Option<ConfigPublication<'a>>,
        must_exist: bool,
    ) -> Result<ConfigPublication<'a>> {
        self.recheck()?;
        let held = publish_captured(&self.units.root, "zxs.service", bytes, original, must_exist)?;
        self.recheck()?;
        Ok(held)
    }
    pub(crate) fn install_samba<'a>(
        &'a self,
        bytes: &[u8],
        original: Option<ConfigPublication<'a>>,
        must_exist: bool,
    ) -> Result<ConfigPublication<'a>> {
        self.recheck()?;
        let parent = self
            .samba
            .as_ref()
            .ok_or("no Samba destination was admitted")?;
        let held = publish_captured(&parent.root, "zxs.conf", bytes, original, must_exist)?;
        self.recheck()?;
        Ok(held)
    }
}
fn trusted(file: &File) -> Result<()> {
    let metadata = file.metadata()?;
    if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err("configuration parent must be root-owned and not writable by other users/groups; no permissions were changed".into());
    }
    Ok(())
}
pub(crate) struct ConfigurationCapture<'a> {
    pub(crate) unit: Option<ConfigPublication<'a>>,
    pub(crate) samba: Option<ConfigPublication<'a>>,
}
impl ConfigurationCapture<'_> {
    pub(crate) fn require_current(&self) -> Result<()> {
        if let Some(held) = &self.unit {
            held.require_current()?;
        }
        if let Some(held) = &self.samba {
            held.require_current()?;
        }
        Ok(())
    }
}
/// The exact compared leaf remains open across subsequent file/receipt steps.
pub(crate) struct ConfigPublication<'a> {
    root: &'a OpenAt2Root,
    name: EntryName,
    held: File,
    identity: FileIdentity,
    stamp: crate::system_program::FileStamp,
    pub(crate) created: bool,
}
impl ConfigPublication<'_> {
    pub(crate) fn require_current(&self) -> Result<()> {
        self.root.verify_entry_identity(&self.name, self.identity)?;
        if crate::system_program::FileStamp::of(&self.held.metadata()?) != self.stamp {
            return Err(
                "configuration changed after exact comparison; preserve the original receipt"
                    .into(),
            );
        }
        Ok(())
    }
}
fn matching<'a>(
    root: &'a OpenAt2Root,
    name: &str,
    bytes: &[u8],
) -> Result<Option<ConfigPublication<'a>>> {
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err("configuration file exceeds the fixed size budget".into());
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
    let stamp = crate::system_program::FileStamp::of(&metadata);
    let mut actual = Vec::new();
    (&file)
        .take(bytes.len() as u64 + 1)
        .read_to_end(&mut actual)?;
    if actual != bytes {
        return Err(format!("existing configuration {:?} differs; replacement is not implemented and all bytes are preserved", name.as_os_str()).into());
    }
    let held = ConfigPublication {
        root,
        name,
        held: file,
        identity,
        stamp,
        created: false,
    };
    held.require_current()?;
    Ok(Some(held))
}
fn publish<'a>(
    root: &'a OpenAt2Root,
    name: &str,
    bytes: &[u8],
    must_exist: bool,
) -> Result<ConfigPublication<'a>> {
    publish_captured(root, name, bytes, matching(root, name, bytes)?, must_exist)
}
fn publish_captured<'a>(
    root: &'a OpenAt2Root,
    name: &str,
    bytes: &[u8],
    original: Option<ConfigPublication<'a>>,
    must_exist: bool,
) -> Result<ConfigPublication<'a>> {
    let held = match original {
        Some(held) => {
            if held.root.identity() != root.identity()
                || held.name.as_os_str() != std::ffi::OsStr::new(name)
            {
                return Err("captured configuration names another original parent or file".into());
            }
            held.require_current()?;
            held
        }
        None if must_exist => {
            return Err(
                "a completed configuration file is missing; do not refill it on retry".into(),
            );
        }
        None => {
            let published =
                write_atomic_at_mode_receipt(root, &EntryName::new(name)?, bytes, false, 0o644)?;
            let mut held =
                matching(root, name, bytes)?.ok_or("published configuration disappeared")?;
            if held.identity != published.identity() {
                return Err(
                    "configuration was replaced before its original inode was read back".into(),
                );
            }
            held.created = true;
            held
        }
    };
    held.held.sync_all()?;
    root.directory().sync_all()?;
    held.require_current()?;
    Ok(held)
}

#[cfg(test)]
#[path = "../../../test/helper/unit/private_io/configuration_files_tests.rs"]
mod tests;

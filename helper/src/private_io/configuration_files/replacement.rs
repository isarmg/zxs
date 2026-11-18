//! Only existing fixed files. No buffer containing original configuration is
//! returned, diagnosed, persisted or reused as a generated configuration.
use super::{ConfigurationFiles, MAX_CONFIG_BYTES, Parent, SAMBA_PATH, SYSTEMD_PATH};
use crate::{Result, system_program::FileStamp};
use data_serialization::Serialize;
use platform_io::fs_safety::{EntryName, linux::{MountPolicy, OpenAt2Root}};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, Metadata},
    os::unix::fs::{FileExt, MetadataExt},
};

#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub(crate) enum Target { SystemdService, SambaConfiguration }
impl Target {
    pub(crate) fn path(self) -> &'static str {
        match self { Self::SystemdService => SYSTEMD_PATH, Self::SambaConfiguration => SAMBA_PATH }
    }
    fn name(self) -> &'static str {
        match self { Self::SystemdService => "zxs.service", Self::SambaConfiguration => "zxs.conf" }
    }
}
/// Exact original including its parent; identical bytes held by another inode
/// never match. ctime is an intra-read fence, not an across-exchange identifier.
#[derive(Eq, PartialEq, Serialize)]
pub(crate) struct Original {
    target: Target,
    destination: &'static str,
    parent_device: String,
    parent_inode: String,
    device: String,
    inode: String,
    bytes: u32,
    mode: &'static str,
    uid: u32,
    gid: u32,
    sha256: String,
}

struct HeldOriginal {
    file: File,
    directory: OpenAt2Root,
    stable: FileStamp,
    original: Original,
}
pub(crate) struct HeldOriginals {
    unit: HeldOriginal,
    samba: Option<HeldOriginal>,
}
impl HeldOriginals {
    pub(crate) fn snapshots(&self) -> (&Original, Option<&Original>) {
        (&self.unit.original, self.samba.as_ref().map(|h| &h.original))
    }
    pub(crate) fn recheck(&self, parents: &ConfigurationFiles) -> Result<()> {
        parents.recheck()?;
        self.unit.require(&parents.units)?;
        match (&self.samba, &parents.samba) {
            (None, None) => {}
            (Some(file), Some(parent)) => file.require(parent)?,
            _ => return Err("fixed configuration replacement target set changed".into()),
        }
        parents.recheck()
    }
}

fn allowed(m: &Metadata) -> Result<&'static str> {
    if !m.is_file() || m.uid() != 0 || m.nlink() != 1 || m.ino() == 0
        || m.len() > MAX_CONFIG_BYTES as u64
    { return Err("existing configuration is not one bounded root-owned regular inode".into()); }
    match m.mode() & 0o7777 {
        0o600 => Ok("0600"), 0o644 => Ok("0644"),
        _ => Err("existing configuration must have exactly mode 0600 or 0644".into()),
    }
}

impl HeldOriginal {
    fn capture(parent: &Parent, target: Target) -> Result<Self> {
        let name = EntryName::new(target.name())?;
        let directory = OpenAt2Root::from_directory(parent.root.directory().try_clone()?, MountPolicy::SameMount)?;
        let file = directory.open_file(&name.as_relative(), false)?;
        let before = file.metadata()?;
        let mode = allowed(&before)?;
        // Replacement is a read-only preview even on strict-atime filesystems.
        let flags = fcntl_getfl(&file)?;
        fcntl_setfl(&file, flags | OFlags::NOATIME)?;
        if !fcntl_getfl(&file)?.contains(OFlags::NOATIME) {
            return Err("read-only original could not be held without atime changes".into());
        }
        let stable = FileStamp::of(&before);
        let expected = before.len();
        // No pathname read, no whole-file buffer, no stored or printed old byte.
        let mut offset = 0;
        let mut bytes = [0u8; 4096];
        let mut hash = Sha256::new();
        while offset < expected {
            let part = (expected - offset).min(bytes.len() as u64) as usize;
            file.read_exact_at(&mut bytes[..part], offset)?;
            hash.update(&bytes[..part]);
            bytes[..part].fill(0);
            offset += part as u64;
        }
        let original = Original {
            target, destination: target.path(), parent_device: parent.root.identity().device.to_string(),
            parent_inode: parent.root.identity().inode.to_string(), device: before.dev().to_string(),
            inode: before.ino().to_string(), bytes: u32::try_from(expected)?, mode,
            uid: before.uid(), gid: before.gid(),
            sha256: platform_io::lower_hex(&hash.finalize()),
        };
        let held = Self { file, directory, stable, original };
        held.require(parent)?;
        Ok(held)
    }
    fn require(&self, parent: &Parent) -> Result<()> {
        let name = EntryName::new(self.original.target.name())?;
        let held = self.file.metadata()?;
        allowed(&held)?;
        let named = self.directory.open_file(&name.as_relative(), false)?;
        if self.directory.identity() != parent.root.identity() || self.stable != FileStamp::of(&held) || self.stable != FileStamp::of(&named.metadata()?)
            || self.original.parent_device != parent.root.identity().device.to_string()
            || self.original.parent_inode != parent.root.identity().inode.to_string()
        { return Err("original configuration or its held parent changed; no replacement was made".into()); }
        Ok(())
    }
}

impl ConfigurationFiles {
    pub(crate) fn hold_existing_for_replacement(&self) -> Result<HeldOriginals> {
        self.recheck()?;
        let unit = HeldOriginal::capture(&self.units, Target::SystemdService)?;
        let samba = self.samba.as_ref().map(|parent| HeldOriginal::capture(
            parent, Target::SambaConfiguration,
        )).transpose()?;
        let held = HeldOriginals { unit, samba };
        held.recheck(self)?;
        Ok(held)
    }
}

#[cfg(test)]
#[path = "../../../../test/helper/unit/replacement_original.rs"]
mod tests;

//! Helper-local service identity input. No NSS calls, account creation or secrets.
use crate::Result;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{
    EntryName,
    linux::{MountPolicy, OpenAt2Root, SingleLinkRequirement},
};
use sha2::{Digest, Sha256};
use std::{io::Read, os::unix::fs::MetadataExt, path::Path};

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(try_from = "String")]
pub(crate) struct Name(String);
impl Name {
    pub(crate) fn new(value: String) -> Result<Self> {
        if value.is_empty()
            || value.len() > 32
            || value == "root"
            || !matches!(value.as_bytes().first().copied(), Some(b'a'..=b'z' | b'_'))
            || !value
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'))
        {
            return Err("account/group must be a non-root 1–32 byte lowercase Unix name, beginning with a letter or underscore".into());
        }
        Ok(Self(value))
    }
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for Name {
    type Error = Box<dyn std::error::Error + Send + Sync>;
    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub name: Name,
    pub uid: u32,
    pub gid: u32,
    source_device: String,
    source_inode: String,
    source_sha256: String,
}
pub(crate) struct LocalAccount {
    snapshot: Snapshot,
}
const MAX_PASSWD_BYTES: usize = 4 * 1024 * 1024;
impl LocalAccount {
    pub(crate) fn resolve(name: Name) -> Result<Self> {
        let source = LocalSource::read("passwd")?;
        let (uid, gid) = local_record(std::str::from_utf8(&source.bytes)?, &name)?;
        Ok(Self {
            snapshot: Snapshot {
                name,
                uid,
                gid,
                source_device: source.identity.device.to_string(),
                source_inode: source.identity.inode.to_string(),
                source_sha256: format!("{:x}", Sha256::digest(&source.bytes)),
            },
        })
    }
    pub(crate) fn snapshot(&self) -> Snapshot {
        self.snapshot.clone()
    }
    pub(crate) fn owner(&self) -> (u32, u32) {
        (self.snapshot.uid, self.snapshot.gid)
    }
    pub(crate) fn recheck(&self) -> Result<()> {
        let current = Self::resolve(self.snapshot.name.clone())?;
        if current.snapshot != self.snapshot {
            return Err(
                "selected local account or passwd source changed; prepare a new exact preview"
                    .into(),
            );
        }
        Ok(())
    }
}
/// The group name is resolved independently: systemd's Group= must not silently
/// select a different GID from the account used to create private data.
#[derive(Clone, Eq, PartialEq, Serialize)]
pub(crate) struct GroupSnapshot {
    pub name: Name,
    pub gid: u32,
    source_device: String,
    source_inode: String,
    source_sha256: String,
}
pub(crate) struct LocalGroup {
    snapshot: GroupSnapshot,
}
impl LocalGroup {
    pub(crate) fn resolve(name: Name) -> Result<Self> {
        let source = LocalSource::read("group")?;
        let gid = local_group_record(std::str::from_utf8(&source.bytes)?, &name)?;
        Ok(Self {
            snapshot: GroupSnapshot {
                name,
                gid,
                source_device: source.identity.device.to_string(),
                source_inode: source.identity.inode.to_string(),
                source_sha256: format!("{:x}", Sha256::digest(&source.bytes)),
            },
        })
    }
    pub(crate) fn snapshot(&self) -> GroupSnapshot {
        self.snapshot.clone()
    }
    pub(crate) fn gid(&self) -> u32 {
        self.snapshot.gid
    }
    pub(crate) fn recheck(&self) -> Result<()> {
        let current = Self::resolve(self.snapshot.name.clone())?;
        if current.snapshot != self.snapshot {
            return Err(
                "selected group or local group source changed; prepare a new exact preview".into(),
            );
        }
        Ok(())
    }
}
struct LocalSource {
    identity: platform_io::fs_safety::linux::FileIdentity,
    bytes: Vec<u8>,
}
impl LocalSource {
    fn read(name: &str) -> Result<Self> {
        let root = OpenAt2Root::open(Path::new("/etc"), MountPolicy::AllowNestedMounts)?;
        let parent = root.directory().metadata()?;
        if parent.uid() != 0 || parent.mode() & 0o022 != 0 {
            return Err("/etc is not an administrative identity source".into());
        }
        let entry = EntryName::new(name)?;
        let file = root.open_file(&entry.as_relative(), false)?;
        let metadata = file.metadata()?;
        if metadata.uid() != 0
            || metadata.mode() & 0o022 != 0
            || metadata.len() > MAX_PASSWD_BYTES as u64
        {
            return Err("local identity source has unsafe ownership, permissions or size".into());
        }
        let identity = SingleLinkRequirement::check(&file)?;
        let mut bytes = Vec::new();
        (&file)
            .take(MAX_PASSWD_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_PASSWD_BYTES || bytes.len() as u64 != metadata.len() {
            return Err("local identity source changed length or exceeded its budget".into());
        }
        if root.identity_of(&entry.as_relative())? != identity {
            return Err("local identity source changed during selection".into());
        }
        let current_root = OpenAt2Root::open(Path::new("/etc"), MountPolicy::AllowNestedMounts)?;
        if current_root.identity() != root.identity() {
            return Err("local identity source directory changed identity".into());
        }
        Ok(Self { identity, bytes })
    }
}
fn local_group_record(text: &str, name: &Name) -> Result<u32> {
    let mut selected = None;
    let mut gids = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split(':').collect();
        if fields.len() != 4 {
            return Err("local group source contains an unsupported record".into());
        }
        let gid: u32 = fields[2]
            .parse()
            .map_err(|_| "local group source contains an invalid GID")?;
        gids.push(gid);
        if fields[0] == name.as_str() && selected.replace(gid).is_some() {
            return Err("selected local group is duplicated".into());
        }
    }
    let gid = selected.ok_or(
        "selected group is absent from local /etc/group; group creation is not implemented",
    )?;
    if gid == 0 || gids.iter().filter(|candidate| **candidate == gid).count() != 1 {
        return Err("selected group must have a unique non-root local GID".into());
    }
    Ok(gid)
}
fn local_record(text: &str, name: &Name) -> Result<(u32, u32)> {
    let mut selected = None;
    let mut ids = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split(':').collect();
        if fields.len() != 7 {
            return Err(
                "local passwd source contains an unsupported record; no account was selected"
                    .into(),
            );
        }
        let uid: u32 = fields[2]
            .parse()
            .map_err(|_| "local passwd source contains an invalid UID")?;
        let gid: u32 = fields[3]
            .parse()
            .map_err(|_| "local passwd source contains an invalid GID")?;
        ids.push((fields[0], uid));
        if fields[0] == name.as_str() {
            if selected.replace((uid, gid)).is_some() {
                return Err("selected local account is duplicated".into());
            }
        }
    }
    let (uid,gid) = selected.ok_or("selected service account does not exist in local /etc/passwd; account creation is not implemented")?;
    if uid == 0 || gid == 0 {
        return Err("private service data requires a non-root UID and primary GID".into());
    }
    if ids
        .iter()
        .filter(|(_, candidate)| *candidate == uid)
        .count()
        != 1
    {
        return Err("selected service UID is shared by another local account".into());
    }
    Ok((uid, gid))
}
#[cfg(test)]
#[path = "../../test/helper/unit/initialization/account_tests.rs"]
mod tests;

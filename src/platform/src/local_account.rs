//! Local service identity input. No NSS calls, account creation or secrets.
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
use crate::fs_safety::{
    EntryName,
    linux::{MountPolicy, OpenAt2Root, SingleLinkRequirement},
};
use data_serialization::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(try_from = "String")]
pub struct Name(String);
impl Name {
    pub fn new(value: String) -> Result<Self> {
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
    pub fn as_str(&self) -> &str {
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
pub struct Snapshot {
    pub name: Name,
    pub uid: u32,
    pub gid: u32,
    source_device: String,
    source_inode: String,
    source_sha256: String,
}
pub struct LocalAccount {
    snapshot: Snapshot,
    home: PathBuf,
    shell: PathBuf,
}
const MAX_PASSWD_BYTES: usize = 4 * 1024 * 1024;
impl LocalAccount {
    pub fn resolve(name: Name) -> Result<Self> {
        Self::lookup(name)?
            .ok_or_else(|| "selected service account is absent from local /etc/passwd".into())
    }
    /// Read a local identity; absence is distinct from malformed or unsafe sources.
    pub fn lookup(name: Name) -> Result<Option<Self>> {
        let source = LocalSource::read("passwd")?;
        let Some(record) = local_record(std::str::from_utf8(&source.bytes)?, &name)? else {
            return Ok(None);
        };
        let AccountRecord {
            uid,
            gid,
            home,
            shell,
        } = record;
        Ok(Some(Self {
            snapshot: Snapshot {
                name,
                uid,
                gid,
                source_device: source.identity.device.to_string(),
                source_inode: source.identity.inode.to_string(),
                source_sha256: crate::lower_hex(&Sha256::digest(&source.bytes)),
            },
            home: home.into(),
            shell: shell.into(),
        }))
    }
    pub fn home(&self) -> &Path {
        &self.home
    }
    pub fn shell(&self) -> &Path {
        &self.shell
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.clone()
    }
    pub fn owner(&self) -> (u32, u32) {
        (self.snapshot.uid, self.snapshot.gid)
    }
    pub fn recheck(&self) -> Result<()> {
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
pub struct GroupSnapshot {
    pub name: Name,
    pub gid: u32,
    source_device: String,
    source_inode: String,
    source_sha256: String,
}
pub struct LocalGroup {
    snapshot: GroupSnapshot,
}
impl LocalGroup {
    pub fn resolve(name: Name) -> Result<Self> {
        Self::lookup(name)?.ok_or_else(|| "selected group is absent from local /etc/group".into())
    }
    /// Read a local group without treating malformed sources as absence.
    pub fn lookup(name: Name) -> Result<Option<Self>> {
        let source = LocalSource::read("group")?;
        let Some(gid) = local_group_record(std::str::from_utf8(&source.bytes)?, &name)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            snapshot: GroupSnapshot {
                name,
                gid,
                source_device: source.identity.device.to_string(),
                source_inode: source.identity.inode.to_string(),
                source_sha256: crate::lower_hex(&Sha256::digest(&source.bytes)),
            },
        }))
    }
    pub fn snapshot(&self) -> GroupSnapshot {
        self.snapshot.clone()
    }
    pub fn gid(&self) -> u32 {
        self.snapshot.gid
    }
    pub fn recheck(&self) -> Result<()> {
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
    identity: crate::fs_safety::linux::FileIdentity,
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
fn local_group_record(text: &str, name: &Name) -> Result<Option<u32>> {
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
    let Some(gid) = selected else {
        return Ok(None);
    };
    if gid == 0 || gids.iter().filter(|candidate| **candidate == gid).count() != 1 {
        return Err("selected group must have a unique non-root local GID".into());
    }
    Ok(Some(gid))
}
struct AccountRecord<'a> {
    uid: u32,
    gid: u32,
    home: &'a str,
    shell: &'a str,
}
fn local_record<'a>(text: &'a str, name: &Name) -> Result<Option<AccountRecord<'a>>> {
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
        if fields[0] == name.as_str()
            && selected
                .replace(AccountRecord {
                    uid,
                    gid,
                    home: fields[5],
                    shell: fields[6],
                })
                .is_some()
        {
            return Err("selected local account is duplicated".into());
        }
    }
    let Some(record) = selected else {
        return Ok(None);
    };
    let AccountRecord { uid, gid, .. } = record;
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
    Ok(Some(record))
}
#[cfg(test)]
#[path = "../../../test/platform/unit/local_account_tests.rs"]
mod tests;

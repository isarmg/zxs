use super::*;
use data_serialization::Serialize;
use platform_io::publication::PublicationReceipt;
use sha2::{Digest, Sha256};
use std::{
    io::{Seek, SeekFrom},
    path::PathBuf,
};

#[derive(Debug, Serialize)]
pub(crate) struct RescueReadback {
    parent_device: String,
    parent_inode: String,
    device: String,
    inode: String,
    bytes: String,
    sha256: String,
}

#[derive(PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    length: u64,
    uid: u32,
    gid: u32,
    mode: u32,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl Stamp {
    fn read(file: &File, maximum: usize) -> Result<Self> {
        let metadata = file.metadata()?;
        SingleLinkRequirement::check(file)?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7022 != 0
            || metadata.len() > maximum as u64
        {
            return Err("rescue image must be an owned bounded regular file without special or group/other-write permissions".into());
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            mode: metadata.mode(),
            links: metadata.nlink(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

pub(crate) fn rescue_digest(bytes: &[u8]) -> String {
    platform_io::lower_hex(&Sha256::digest(bytes))
}

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn recheck_parent(target: &AnchoredPath, path: &Path) -> Result<()> {
    let current = OpenAt2Root::open(
        &parent(path).canonicalize()?,
        MountPolicy::AllowNestedMounts,
    )?;
    if current.identity() != target.directory.identity() {
        return Err(
            "rescue image parent changed during verification; no complete readback is claimed"
                .into(),
        );
    }
    Ok(())
}

fn no_sidecars(target: &AnchoredPath) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = target.name.as_os_str().to_os_string();
        name.push(suffix);
        match statat(target.directory.directory(), name.as_os_str(), AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => return Err("rescue SQLite has a live or unknown sidecar; preserve all names and stop its writer".into()),
            Err(rustix::io::Errno::NOENT) => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn read(file: &File, maximum: usize) -> Result<Vec<u8>> {
    let limit = u64::try_from(maximum)?
        .checked_add(1)
        .ok_or("rescue image limit overflow")?;
    let mut descriptor = file;
    descriptor.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    descriptor.take(limit).read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err("rescue image exceeds its byte budget".into());
    }
    Ok(bytes)
}

fn verify_at(
    target: AnchoredPath,
    path: &Path,
    published: Option<&PublicationReceipt>,
    maximum: usize,
    verify: impl FnOnce(&[u8]) -> Result<()>,
) -> Result<RescueReadback> {
    recheck_parent(&target, path)?;
    no_sidecars(&target)?;
    let file = target
        .directory
        .open_file(&target.name.as_relative(), false)?;
    let before = Stamp::read(&file, maximum)?;
    if published.is_some_and(|receipt| {
        let identity = receipt.identity();
        identity.device != before.device || identity.inode != before.inode
    }) {
        return Err("published rescue image was replaced before its complete readback".into());
    }
    let bytes = read(&file, maximum)?;
    if bytes.len() as u64 != before.length || Stamp::read(&file, maximum)? != before {
        return Err("rescue image changed while its first read was captured".into());
    }
    let sha256 = rescue_digest(&bytes);
    verify(&bytes)?;
    drop(bytes);
    let after_bytes = read(&file, maximum)?;
    if rescue_digest(&after_bytes) != sha256 || Stamp::read(&file, maximum)? != before {
        return Err("rescue image changed during SQLite verification".into());
    }
    let named = target
        .directory
        .open_file(&target.name.as_relative(), false)?;
    if Stamp::read(&named, maximum)? != before {
        return Err("rescue image name no longer identifies the checked file".into());
    }
    no_sidecars(&target)?;
    recheck_parent(&target, path)?;
    Ok(RescueReadback {
        parent_device: target.directory.identity().device.to_string(),
        parent_inode: target.directory.identity().inode.to_string(),
        device: before.device.to_string(),
        inode: before.inode.to_string(),
        bytes: before.length.to_string(),
        sha256,
    })
}

pub(crate) fn verify_rescue_sqlite(
    path: &Path,
    maximum: usize,
    verify: impl FnOnce(&[u8]) -> Result<()>,
) -> Result<RescueReadback> {
    verify_at(AnchoredPath::open(path)?, path, None, maximum, verify)
}

#[derive(Debug)]
struct PublishedUnverified {
    path: PathBuf,
    identity: FileIdentity,
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for PublishedUnverified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "rescue SQLite was published at {:?} as {:?}, but readback is incomplete; preserve it and use the explicit verify command instead of rebuilding or overwriting",
            self.path, self.identity
        )
    }
}
impl std::error::Error for PublishedUnverified {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub(crate) fn publish_rescue_sqlite(
    path: &Path,
    bytes: &[u8],
    maximum: usize,
    verify: impl FnOnce(&[u8]) -> Result<()>,
) -> Result<RescueReadback> {
    let target = AnchoredPath::open(path)?;
    recheck_parent(&target, path)?;
    no_sidecars(&target)?;
    if bytes.len() > maximum {
        return Err("rescue image exceeds its byte budget".into());
    }
    let receipt =
        write_atomic_at_mode_receipt(&target.directory, &target.name, bytes, false, 0o600)?;
    let identity = receipt.identity();
    verify_at(target, path, Some(&receipt), maximum, verify).map_err(|source| {
        Box::new(PublishedUnverified {
            path: path.to_owned(),
            identity,
            source,
        }) as Box<dyn std::error::Error + Send + Sync>
    })
}

#[cfg(test)]
#[path = "../../../test/helper/unit/private_io/rescue_image_tests.rs"]
mod tests;

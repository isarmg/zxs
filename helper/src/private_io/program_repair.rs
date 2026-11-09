//! Same-payload repair retains both an independent backup and the exchanged
//! original inode. No data/configuration directory is opened here.
use super::*;
use data_serialization::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const MAX_PROGRAM_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ProgramSnapshot {
    device: String,
    inode: String,
    uid: u32,
    gid: u32,
    mode: u32,
    bytes: u64,
    sha256: String,
}

struct Captured {
    file: File,
    snapshot: ProgramSnapshot,
    bytes: Vec<u8>,
}
impl Captured {
    fn matches_payload(&self, payload: &[u8]) -> bool {
        self.snapshot.mode == 0o555 && self.bytes == payload
    }
}

enum Position {
    Before,
    After,
}

impl ProgramDestination {
    pub(crate) fn repair_recheck(&self) -> Result<()> {
        self.recheck_parent()
    }

    fn capture_at(&self, name: &EntryName) -> Result<Option<Captured>> {
        self.recheck_parent()?;
        let file = match self.target.directory.open_file(&name.as_relative(), false) {
            Ok(file) => file,
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7022 != 0
            || metadata.len() > MAX_PROGRAM_BYTES as u64
        {
            return Err("repair source must be an administrative-owned, bounded regular file without special or group/other-write modes; unsafe files are not replaced".into());
        }
        let identity = SingleLinkRequirement::check(&file)?;
        let mut bytes = Vec::new();
        (&file)
            .take(MAX_PROGRAM_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_PROGRAM_BYTES || bytes.len() as u64 != metadata.len() {
            return Err("repair program changed length or exceeded its byte budget".into());
        }
        let snapshot = ProgramSnapshot {
            device: identity.device.to_string(),
            inode: identity.inode.to_string(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            mode: metadata.mode() & 0o7777,
            bytes: metadata.len(),
            sha256: platform_io::lower_hex(&Sha256::digest(&bytes)),
        };
        self.check_repair_name(name, identity)?;
        self.recheck_parent()?;
        Ok(Some(Captured {
            file,
            snapshot,
            bytes,
        }))
    }

    pub(crate) fn repair_snapshot(&self, payload: &[u8]) -> Result<Option<ProgramSnapshot>> {
        match self.capture_at(&self.target.name)? {
            Some(current) if current.matches_payload(payload) => Err("the exact embedded program is already installed; use ordinary installation verification instead of creating a repair".into()),
            Some(current) => Ok(Some(current.snapshot)),
            None => Ok(None),
        }
    }

    fn repair_position(
        &self,
        digest: &str,
        original: &Option<ProgramSnapshot>,
        payload: &[u8],
    ) -> Result<(Position, Option<Captured>)> {
        let target = self.capture_at(&self.target.name)?;
        let stage = self.capture_at(&repair_stage(digest)?)?;
        match (original, target, stage) {
            (Some(expected), Some(current), stage) if current.snapshot == *expected => {
                if stage.as_ref().is_some_and(|file| !file.matches_payload(payload)) {
                    return Err("repair staging is not the exact replacement; nothing was overwritten".into());
                }
                Ok((Position::Before, Some(current)))
            }
            (Some(expected), Some(current), Some(old))
                if current.matches_payload(payload) && old.snapshot == *expected =>
            {
                Ok((Position::After, Some(old)))
            }
            (None, None, stage) => {
                if stage.as_ref().is_some_and(|file| !file.matches_payload(payload)) {
                    return Err("missing-program repair has conflicting staging bytes".into());
                }
                Ok((Position::Before, None))
            }
            (None, Some(current), None) if current.matches_payload(payload) => Ok((Position::After, None)),
            _ => Err("program and repair staging do not match one legal state of the exact original request; preserve both names and the backup".into()),
        }
    }

    pub(crate) fn repair_original_bytes(
        &self,
        digest: &str,
        original: &Option<ProgramSnapshot>,
        payload: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        Ok(self
            .repair_position(digest, original, payload)?
            .1
            .map(|original| original.bytes))
    }

    pub(crate) fn repair_is_published(
        &self,
        digest: &str,
        original: &Option<ProgramSnapshot>,
        payload: &[u8],
    ) -> Result<bool> {
        Ok(matches!(
            self.repair_position(digest, original, payload)?.0,
            Position::After
        ))
    }

    pub(crate) fn completed_repair_snapshot(
        &self,
        digest: &str,
        original: &Option<ProgramSnapshot>,
        payload: &[u8],
    ) -> Result<ProgramSnapshot> {
        if !matches!(
            self.repair_position(digest, original, payload)?.0,
            Position::After
        ) {
            return Err("completed repair lost its exact target; it was not recreated".into());
        }
        Ok(self
            .capture_at(&self.target.name)?
            .ok_or("completed repair target disappeared")?
            .snapshot)
    }

    pub(crate) fn repair_install(
        &self,
        digest: &str,
        original: &Option<ProgramSnapshot>,
        payload: &[u8],
        revalidate: impl Fn() -> Result<()>,
    ) -> Result<ProgramSnapshot> {
        revalidate()?;
        let stage = repair_stage(digest)?;
        let (position, _) = self.repair_position(digest, original, payload)?;
        if matches!(position, Position::Before) {
            match self.capture_at(&stage)? {
                Some(file) => file.file.sync_all()?,
                None => {
                    write_atomic_at_mode(&self.target.directory, &stage, payload, false, 0o555)?
                }
            }
            revalidate()?;
            let (position, _) = self.repair_position(digest, original, payload)?;
            if !matches!(position, Position::Before) {
                return Err(
                    "repair position changed before exchange; no exchange was attempted".into(),
                );
            }
            let flags = if original.is_some() {
                RenameFlags::EXCHANGE
            } else {
                RenameFlags::NOREPLACE
            };
            renameat_with(
                self.target.directory.directory(),
                stage.as_os_str(),
                self.target.directory.directory(),
                self.target.name.as_os_str(),
                flags,
            )
            .map_err(|error| RepairPublication {
                parent: self.parent_identity(),
                stage: stage.as_os_str().to_string_lossy().into_owned(),
                source: AtomicCommitError::Unknown(platform_io::fs_safety::Error::Io(error.into())),
            })?;
        }
        self.finish_repair(digest, original, payload, &revalidate)
    }

    fn finish_repair(
        &self,
        digest: &str,
        original: &Option<ProgramSnapshot>,
        payload: &[u8],
        revalidate: &impl Fn() -> Result<()>,
    ) -> Result<ProgramSnapshot> {
        let (position, retained) = self.repair_position(digest, original, payload)?;
        if !matches!(position, Position::After) {
            return Err(
                "repair has no completed program publication; all evidence was retained".into(),
            );
        }
        if let Some(old) = retained {
            old.file.sync_all()?;
        }
        let current = self
            .capture_at(&self.target.name)?
            .ok_or("repaired executable disappeared before synchronization")?;
        current.file.sync_all()?;
        self.target
            .directory
            .directory()
            .sync_all()
            .map_err(|error| RepairPublication {
                parent: self.parent_identity(),
                stage: format!(".zxs-helper-repair-{digest}.program"),
                source: AtomicCommitError::Published(
                    platform_io::fs_safety::Error::PublishedDurabilityUnknown(error),
                ),
            })?;
        revalidate()?;
        let final_file = self
            .capture_at(&self.target.name)?
            .ok_or("repaired executable disappeared after synchronization")?;
        if final_file.snapshot != current.snapshot || !final_file.matches_payload(payload) {
            return Err(
                "repaired executable changed before completion; no success is claimed".into(),
            );
        }
        Ok(final_file.snapshot)
    }

    fn check_repair_name(&self, name: &EntryName, identity: FileIdentity) -> Result<()> {
        let named = statat(
            self.target.directory.directory(),
            name.as_os_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        )?;
        if (named.st_dev, named.st_ino) != (identity.device, identity.inode) {
            return Err("program identity changed during repair inspection".into());
        }
        Ok(())
    }
}

fn repair_stage(digest: &str) -> Result<EntryName> {
    Ok(EntryName::new(format!(
        ".zxs-helper-repair-{digest}.program"
    ))?)
}

#[derive(Debug)]
struct RepairPublication {
    parent: FileIdentity,
    stage: String,
    source: AtomicCommitError,
}
impl std::fmt::Display for RepairPublication {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "program repair exchange/publication under {:?}, stage {}; preserve the target, original inode and backup",
            self.parent, self.stage
        )
    }
}
impl std::error::Error for RepairPublication {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[cfg(test)]
#[path = "../../../test/helper/unit/private_io/program_repair_tests.rs"]
mod tests;

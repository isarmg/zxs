//! Installation of reconstructed, scoped databases, not database-based State
//! election. Only decided publication / current-State repair invokes `apply`.
//! All staging is outside the managed entity; old and unknown objects are kept.
use crate::{
    Error, ErrorCode, Result,
    disk::{DiskRoot, FileStamp, HeldRegular, InstallExchange},
};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::linux::FileIdentity;
use std::path::{Path, PathBuf};

const FORMAT: &str = "scoped-database-install";
const MAX_ENVELOPE: usize = 8192;
const MAX_RETAINED_ENTRIES: usize = 8192;
const MAX_RETAINED_BYTES: u64 = 256 * 1024 * 1024;

/// Admission quota cannot strand an already decided operation because its own
/// interrupted preparation consumed reserved space. Only verified recovery
/// callers select this mode; it is not derived from an on-disk envelope.
#[derive(Clone, Copy)]
pub(crate) enum InstallAdmission {
    NewOperation,
    DecidedRecovery,
}
impl InstallAdmission {
    pub(crate) fn check(
        self,
        root: &DiskRoot,
        history: &Path,
        bytes: u64,
        entries: usize,
    ) -> Result<()> {
        match self {
            Self::NewOperation => check_budget(root, history, bytes, entries),
            Self::DecidedRecovery => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serialization(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Scope {
    Directory {
        history: String,
        repository: String,
        directory: String,
    },
}
impl Scope {
    pub(crate) fn directory(history: &str, repository: &str, directory: &str) -> Result<Self> {
        storage_contract::validate_id(history, "history")?;
        storage_contract::validate_id(repository, "repo")?;
        crate::container_storage::validate_database_directory_id(directory)?;
        Ok(Self::Directory {
            history: history.into(),
            repository: repository.into(),
            directory: directory.into(),
        })
    }
    fn leaf(&self) -> &'static str {
        crate::container_storage::directory_database_name()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serialization(deny_unknown_fields)]
struct Source {
    scope: Scope,
    device: u64,
    inode: u64,
    bytes: u64,
    digest: String,
}
#[derive(Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
struct Envelope {
    format: String,
    source: Source,
    stage_device: u64,
    stage_inode: u64,
    new: FileStamp,
    old: Option<FileStamp>,
}

pub(crate) struct PreparedInstall {
    entity: DiskRoot,
    source: Source,
    bytes: Vec<u8>,
    old: Option<HeldRegular>,
    history: PathBuf,
    maximum: usize,
    resume: Option<(DiskRoot, Envelope)>,
    already_current: bool,
    admission: InstallAdmission,
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, message)
}

impl PreparedInstall {
    /// The caller obtained `bytes` from this frozen admitted State. The verifier
    /// checks only stored scope/format, never grants authority to a stored DB.
    pub(crate) fn capture(
        root: &DiskRoot,
        history: &Path,
        entity: (&Path, FileIdentity),
        scope: (Scope, InstallAdmission),
        bytes: Vec<u8>,
        maximum: usize,
        verify: impl FnOnce(&[u8]) -> Result<()>,
    ) -> Result<Self> {
        if bytes.len() > maximum {
            return Err(invalid("prepared database exceeds its limit"));
        }
        let held = root.private_child_matching(entity.0, entity.1)?;
        let (scope, admission) = scope;
        let mut old = HeldRegular::capture(&held, scope.leaf(), maximum)?;
        if let Some(old) = &old {
            verify(old.bytes())?;
        }
        let already_current = old.as_ref().is_some_and(|old| old.bytes() == bytes);
        if let Some(old) = &mut old {
            old.release_bytes();
        }
        let source = Source {
            scope,
            device: entity.1.device,
            inode: entity.1.inode,
            bytes: bytes.len() as u64,
            digest: blake3::hash(&bytes).to_hex().to_string(),
        };
        let path = history.join(".zxs-layout/database-installs");
        root.private_dir(&path)?;
        let mut prepared = Self {
            entity: held,
            source,
            bytes,
            old,
            history: history.to_owned(),
            maximum,
            resume: None,
            already_current,
            admission,
        };
        let generation = path.join(prepared.binding()?);
        if root.optional_metadata(&generation)?.is_some() {
            prepared.resume = prepared.recover_envelope(&root.private_child(&generation)?)?;
        }
        Ok(prepared)
    }

    fn binding(&self) -> Result<String> {
        Ok(storage_contract::object_hash(
            "scoped-database-install-source",
            &self.source,
        )?)
    }

    pub(crate) fn reservation(&self) -> (u64, usize) {
        if self.resume.is_some() || self.already_current {
            (0, 0)
        } else {
            (
                self.source
                    .bytes
                    .max(self.old.as_ref().map_or(0, |old| old.stamp().bytes))
                    + MAX_ENVELOPE as u64,
                5,
            )
        }
    }

    pub(crate) fn apply(
        mut self,
        root: &DiskRoot,
        observe: &mut impl FnMut(&'static str) -> Result<()>,
    ) -> Result<()> {
        if let Some(old) = &self.old {
            old.check_running()?;
        }
        if let Some((stage, envelope)) = self.resume.take() {
            return self.finish(stage, envelope, observe);
        }
        let current = HeldRegular::capture(&self.entity, self.source.scope.leaf(), self.maximum)?;
        if current
            .as_ref()
            .is_some_and(|file| file.bytes() == self.bytes)
        {
            self.entity.sync_dir(self.entity.path())?;
            return Ok(());
        }
        if current.as_ref().map(HeldRegular::stamp) != self.old.as_ref().map(HeldRegular::stamp) {
            return Err(invalid(
                "database target changed before installation intent",
            ));
        }
        let (reserved, entries) = self.reservation();
        self.admission
            .check(root, &self.history, reserved, entries)?;
        let generation = self
            .history
            .join(".zxs-layout/database-installs")
            .join(self.binding()?);
        root.private_dir(&generation)?;
        let generations = root.private_child(&generation)?;
        let attempt = generations.path().join(crate::new_id("req")?);
        let identity = generations.create_private_directory_synced(&attempt)?;
        let stage = generations.private_child_matching(&attempt, identity)?;
        // A death here leaves an orphan outside the entity. It is preserved and
        // counted, but cannot supply authority or block entity namespace scans.
        stage.create_install_file("payload", &self.bytes)?;
        observe("after_database_stage")?;
        let staged = HeldRegular::capture(&stage, "payload", self.maximum)?
            .ok_or_else(|| invalid("new database stage disappeared"))?;
        if staged.bytes() != self.bytes {
            return Err(invalid("new database stage changed"));
        }
        let envelope = Envelope {
            format: FORMAT.into(),
            source: self.source.clone(),
            stage_device: identity.device,
            stage_inode: identity.inode,
            new: staged.stamp().clone(),
            old: self.old.as_ref().map(|old| old.stamp().clone()),
        };
        let encoded = crate::disk::encode_metadata(&envelope, MAX_ENVELOPE)?;
        stage.write_install_intent(&encoded, observe)?;
        observe("after_database_intent")?;
        self.finish(stage, envelope, observe)
    }

    fn recover_envelope(&self, generation: &DiskRoot) -> Result<Option<(DiskRoot, Envelope)>> {
        let mut found = None;
        let mut count = 0;
        let target = HeldRegular::capture(&self.entity, self.source.scope.leaf(), self.maximum)?;
        for entry in generation.entries(generation.path())? {
            count += 1;
            if count > MAX_RETAINED_ENTRIES {
                return Err(retention_limit());
            }
            let path = generation.path().join(entry?.as_path());
            let stage = generation.private_child(&path)?;
            let Some(intent) = HeldRegular::capture(&stage, "intent.json", MAX_ENVELOPE)? else {
                continue; // unregistered orphan: never adopt, move, or delete
            };
            let envelope: Envelope = storage_contract::decode_strict(intent.bytes())?;
            let identity = stage.directory_identity(stage.path())?;
            if envelope.format != FORMAT
                || envelope.source != self.source
                || envelope.stage_device != identity.device
                || envelope.stage_inode != identity.inode
                || envelope.new.device != self.source.device
                || envelope.new.digest != self.source.digest
                || envelope.new.bytes != self.source.bytes
            {
                return Err(invalid(
                    "installation envelope differs from the reconstructed admitted source",
                ));
            }
            let payload = HeldRegular::capture(&stage, "payload", self.maximum)?;
            let payload_stamp = payload.as_ref().map(HeldRegular::stamp);
            let target_stamp = target.as_ref().map(HeldRegular::stamp);
            if payload_stamp == envelope.old.as_ref() && target_stamp != Some(&envelope.new) {
                // This attempt no longer carries the new material. Its known
                // displaced old file (or first-create absence) remains retained.
                // It cannot authorize a replay. The current admitted State may
                // independently repair a missing/revalidated target with a new
                // no-clobber/preserving attempt; do not resurrect this receipt.
                continue;
            }
            if payload_stamp != Some(&envelope.new) && payload_stamp != envelope.old.as_ref() {
                return Err(invalid(
                    "installation contains an unknown displaced object; all material retained",
                ));
            }
            if found.replace((stage, envelope)).is_some() {
                return Err(invalid("multiple installation intents claim one source"));
            }
        }
        Ok(found)
    }

    fn finish(
        self,
        stage: DiskRoot,
        envelope: Envelope,
        observe: &mut impl FnMut(&'static str) -> Result<()>,
    ) -> Result<()> {
        let target = HeldRegular::capture(&self.entity, self.source.scope.leaf(), self.maximum)?;
        let payload = HeldRegular::capture(&stage, "payload", self.maximum)?;
        let actual_target = target.as_ref().map(HeldRegular::stamp);
        let actual_payload = payload.as_ref().map(HeldRegular::stamp);
        if actual_target == Some(&envelope.new) && actual_payload == envelope.old.as_ref() {
            // Includes first-create where the source name is absent. Complete
            // both durability tails even when the earlier result was unknown.
            self.entity.sync_dir(self.entity.path())?;
            stage.sync_dir(stage.path())?;
            return Ok(());
        }
        if actual_target != envelope.old.as_ref() || actual_payload != Some(&envelope.new) {
            return Err(invalid(
                "database installation has an unknown resource; all material retained",
            ));
        }
        self.entity.publish_install_preserving(
            &stage,
            self.source.scope.leaf(),
            InstallExchange {
                staged: &envelope.new,
                old: envelope.old.as_ref(),
                maximum: self.maximum,
            },
            observe,
        )
    }
}

fn retention_limit() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "retained database installations require explicit review before more writes",
    )
}

/// Bounded accounting, not recognition or cleanup. Unknown bytes stay intact.
/// No recursive attacker-defined paths: the fixed hierarchy is generation,
/// attempt, leaf. Refuse deeper objects instead of interpreting their contents.
pub(crate) fn check_budget(
    root: &DiskRoot,
    history: &Path,
    reserved: u64,
    reserved_entries: usize,
) -> Result<()> {
    if reserved > MAX_RETAINED_BYTES || reserved_entries > MAX_RETAINED_ENTRIES {
        return Err(retention_limit());
    }
    if reserved == 0 && reserved_entries == 0 {
        return Ok(());
    }
    let parent = history.join(".zxs-layout/database-installs");
    if root.optional_metadata(&parent)?.is_none() {
        return Ok(());
    }
    let held = root.private_child(&parent)?;
    let mut stack = vec![(held, 0usize)];
    let mut entries = reserved_entries;
    let mut bytes = reserved;
    while let Some((directory, depth)) = stack.pop() {
        for entry in directory.entries(directory.path())? {
            entries += 1;
            if entries > MAX_RETAINED_ENTRIES {
                return Err(retention_limit());
            }
            let path = directory.path().join(entry?.as_path());
            let metadata = directory
                .optional_metadata(&path)?
                .ok_or_else(|| invalid("retained installation disappeared during accounting"))?;
            if metadata.is_dir() && depth < 2 {
                stack.push((directory.private_child(&path)?, depth + 1));
            } else if metadata.is_file() {
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or_else(retention_limit)?;
                if bytes > MAX_RETAINED_BYTES {
                    return Err(retention_limit());
                }
            } else {
                return Err(invalid(
                    "unknown installation object retained for inspection",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../test/engine/unit/database_install_tests.rs"]
mod tests;

//! Installation of reconstructed, scoped databases, not database-based State
//! election. Only decided publication / current-State repair invokes `apply`.
//! All staging is outside the managed entity; old and unknown objects are kept.
use crate::{
    Error, ErrorCode, Result,
    disk::{DiskRoot, FileStamp, HeldRegular, InstallExchange},
};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{EntryName, linux::FileIdentity};
use std::path::{Path, PathBuf};

pub(crate) const DISK_DATABASE_NAME: &str = ".zxs-disk.sqlite";
pub(crate) const DISK_HISTORY_DATABASE_NAME: &str = ".zxs-disk-history.sqlite";

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
    Disk {
        identity: crate::DiskRegistrationIdentity,
    },
    DiskHistory {
        identity: crate::DiskRegistrationIdentity,
    },
    File {
        history: String,
        file: String,
    },
    Directory {
        history: String,
        repository: String,
        directory: String,
    },
}
impl Scope {
    pub(crate) fn disk(identity: crate::DiskRegistrationIdentity) -> Self {
        Self::Disk { identity }
    }
    pub(crate) fn disk_history(identity: crate::DiskRegistrationIdentity) -> Self {
        Self::DiskHistory { identity }
    }
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
    pub(crate) fn file(history: &str, file: &str) -> Result<Self> {
        storage_contract::validate_id(history, "history")?;
        storage_contract::validate_id(file, "file")?;
        Ok(Self::File {
            history: history.into(),
            file: file.into(),
        })
    }
    fn leaf(&self) -> &'static str {
        match self {
            Self::Disk { .. } => DISK_DATABASE_NAME,
            Self::DiskHistory { .. } => DISK_HISTORY_DATABASE_NAME,
            Self::Directory { .. } => crate::container_storage::directory_database_name(),
            Self::File { .. } => crate::container_storage::file_database::NAME,
        }
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
    completed_tails: Vec<(PathBuf, FileIdentity, Envelope)>,
    already_current: bool,
    recovered_displaced: bool,
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
        // Capture is also used by cold read-only move verification. Creating
        // retention directories belongs to prepare_stage, never to observation.
        let mut prepared = Self {
            entity: held,
            source,
            bytes,
            old,
            history: history.to_owned(),
            maximum,
            resume: None,
            completed_tails: Vec::new(),
            already_current,
            recovered_displaced: false,
            admission,
        };
        let generation = path.join(prepared.binding()?);
        if installations_exist(root, history)? && root.optional_metadata(&generation)?.is_some() {
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

    pub(crate) fn byte_len(&self) -> usize {
        self.bytes.len()
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
        self.prepare_stage(root, observe)?;
        for (path, identity, envelope) in std::mem::take(&mut self.completed_tails) {
            let stage = root.private_child_matching(&path, identity)?;
            if stage.read_bytes(&path.join("intent.json"), MAX_ENVELOPE)?
                != crate::disk::encode_metadata(&envelope, MAX_ENVELOPE)?
            {
                return Err(invalid(
                    "completed installation intent changed before its durability tail",
                ));
            }
            self.finish(stage, envelope, observe)?;
        }
        match self.resume.take() {
            Some((stage, envelope)) => self.finish(stage, envelope, observe),
            None => Ok(()),
        }
    }

    /// Freeze the normal preserving-exchange intent without touching the live
    /// alias. Only a later independently admitted joint decision may call apply.
    /// In particular an already-exchanged candidate is not an unpublished stage.
    pub(crate) fn stage_unpublished(
        &mut self,
        root: &DiskRoot,
        observe: &mut impl FnMut(&'static str) -> Result<()>,
    ) -> Result<()> {
        if self.already_current || self.recovered_displaced {
            return Err(invalid(
                "unpublished database has an exposed or displaced original install; all files retained",
            ));
        }
        self.prepare_stage(root, observe)?;
        self.verify_unpublished(root)?;
        let (stage, _) = self
            .resume
            .as_ref()
            .expect("invariant: verified unpublished intent exists");
        for leaf in ["payload", "intent.json"] {
            stage
                .open_read_regular(&stage.path().join(leaf))?
                .sync_all()?;
        }
        stage.sync_dir(stage.path())?;
        root.sync_dir(
            stage
                .path()
                .parent()
                .expect("invariant: stage has a generation parent"),
        )?;
        root.sync_dir(&self.history.join(".zxs-layout/database-installs"))?;
        root.sync_dir(&self.history.join(".zxs-layout"))?;
        root.sync_dir(&self.history)?;
        self.verify_unpublished(root)
    }

    pub(crate) fn verify_unpublished(&self, root: &DiskRoot) -> Result<()> {
        if self.already_current || self.recovered_displaced {
            return Err(invalid(
                "an exposed or displaced database is not an unpublished intent",
            ));
        }
        let (stage, envelope) = self
            .resume
            .as_ref()
            .ok_or_else(|| invalid("unpublished database has no original install intent"))?;
        let inode = root.directory_identity(stage.path())?;
        if inode.device != envelope.stage_device
            || inode.inode != envelope.stage_inode
            || stage.read_bytes(&stage.path().join("intent.json"), MAX_ENVELOPE)?
                != crate::disk::encode_metadata(envelope, MAX_ENVELOPE)?
        {
            return Err(invalid(
                "unpublished install intent or its original directory changed",
            ));
        }
        let target = HeldRegular::capture(&self.entity, self.source.scope.leaf(), self.maximum)?;
        let payload = HeldRegular::capture(stage, "payload", self.maximum)?;
        if target.as_ref().map(HeldRegular::stamp) != envelope.old.as_ref()
            || payload.as_ref().map(HeldRegular::stamp) != Some(&envelope.new)
        {
            return Err(invalid(
                "unpublished database no longer retains the original old/new pair",
            ));
        }
        Ok(())
    }

    /// Bind the original old/new inode pair and retained attempt, not just the
    /// expected SQLite bytes. A move freezes this identity before reporting a
    /// complete preparation; it cannot replace a lost pair on a later retry.
    pub(crate) fn unpublished_identity(&self, root: &DiskRoot) -> Result<String> {
        self.verify_unpublished(root)?;
        let (_, envelope) = self
            .resume
            .as_ref()
            .expect("invariant: verified unpublished install has an envelope");
        Ok(storage_contract::object_hash(
            "scoped-database-unpublished",
            envelope,
        )?)
    }

    fn prepare_stage(
        &mut self,
        root: &DiskRoot,
        observe: &mut impl FnMut(&'static str) -> Result<()>,
    ) -> Result<()> {
        if let Some(old) = &self.old {
            old.check_running()?;
        }
        if self.resume.is_some() {
            return Ok(());
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
        self.resume = Some((stage, envelope));
        Ok(())
    }

    fn recover_envelope(&mut self, generation: &DiskRoot) -> Result<Option<(DiskRoot, Envelope)>> {
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
            if payload_stamp == envelope.old.as_ref() && target_stamp == Some(&envelope.new) {
                // This exchange is complete, not an unpublished source candidate.
                // Repeated current-State repairs may reuse a deleted target inode,
                // so more than one retained completed receipt can name this stamp.
                // Keep each identity without retaining a descriptor per receipt,
                // then re-hold it and complete every durability tail in apply;
                // capture remains read-only and never adopts a second pending pair.
                self.completed_tails.push((path, identity, envelope));
                continue;
            }
            if payload_stamp == envelope.old.as_ref() && target_stamp != Some(&envelope.new) {
                // This attempt no longer carries the new material. Its known
                // displaced old file (or first-create absence) remains retained.
                // It cannot authorize a replay. The current admitted State may
                // independently repair a missing/revalidated target with a new
                // no-clobber/preserving attempt; do not resurrect this receipt.
                // An unpublished move has no such repair authority: remember
                // the displaced intent so it cannot silently create a new pair.
                self.recovered_displaced = true;
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
        &self,
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
            &EntryName::new(self.source.scope.leaf())
                .expect("invariant: every database scope selects one canonical entry"),
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

fn installations_exist(root: &DiskRoot, history: &Path) -> Result<bool> {
    // optional_metadata handles a missing final component; it does not hide a
    // missing intermediate directory. Observe this fixed two-level hierarchy
    // in order without creating it or suppressing other filesystem failures.
    let layout = history.join(".zxs-layout");
    if root.optional_metadata(&layout)?.is_none() {
        return Ok(false);
    }
    Ok(root
        .optional_metadata(&layout.join("database-installs"))?
        .is_some())
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
    if !installations_exist(root, history)? {
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

/// One combined per-history allowance for all consumers in a publication. Each
/// class must contribute before the decision; separate class quotas cannot
/// protect a shared retention directory.
#[derive(Default)]
pub(crate) struct Reservations {
    histories: std::collections::BTreeMap<(String, PathBuf), HistoryReservation>,
}
struct HistoryReservation {
    disk: std::sync::Arc<crate::disk::DiskHandle>,
    bytes: u64,
    entries: usize,
}
impl Reservations {
    pub(crate) fn add(
        &mut self,
        disk: &std::sync::Arc<crate::disk::DiskHandle>,
        history: &Path,
        reservation: (u64, usize),
    ) -> Result<()> {
        let entry = self
            .histories
            .entry((disk.seal.disk_id.clone(), history.to_owned()))
            .or_insert_with(|| HistoryReservation {
                disk: std::sync::Arc::clone(disk),
                bytes: 0,
                entries: 0,
            });
        entry.bytes = entry
            .bytes
            .checked_add(reservation.0)
            .ok_or_else(retention_limit)?;
        entry.entries = entry
            .entries
            .checked_add(reservation.1)
            .ok_or_else(retention_limit)?;
        Ok(())
    }
    pub(crate) fn check(self, admission: InstallAdmission) -> Result<()> {
        for ((_, history), reservation) in self.histories {
            admission.check(
                &reservation.disk.root,
                &history,
                reservation.bytes,
                reservation.entries,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/database_install_tests.rs"]
mod tests;

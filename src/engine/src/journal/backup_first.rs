//! A bounded, prescribed Backup first slot under the *original* writer leases.
//! This is not an active-generation JournalWriter. Only the prescribed first
//! ACCEPT/FINALIZE/Publication slot is writable; it cannot append a later slot,
//! extract its writer, install an active marker, or clear the source fence.
use super::*;
use crate::engine::membership_transition::{PreparedBackupFirstCandidate, pending::activation};

const MAX_SLOT_BYTES: usize = 16 * 1024;

enum SlotState {
    Scanned(OpenJournal),
    Synchronized(JournalWriter),
}

pub(crate) struct BackupFirstJournal<'a> {
    candidate: &'a PreparedBackupFirstCandidate,
    disk: DiskConfig,
    state: Option<SlotState>,
}
impl<'a> BackupFirstJournal<'a> {
    /// Read the actual completed ordinary-address journal through one retained
    /// descriptor. No copied receipt vector may create or repair this file.
    pub(crate) fn published_binding(
        root: &DiskRoot,
        disk: &DiskConfig,
        candidate: &PreparedBackupFirstCandidate,
        seal: &PublicationSeal,
        stop: &std::sync::atomic::AtomicBool,
    ) -> Result<(platform_io::fs_safety::linux::FileIdentity, String)> {
        use platform_io::fs_safety::linux::FileIdentity;
        use std::os::unix::fs::MetadataExt;
        Self::require_member(root, disk, candidate)?;
        if seal.candidate_hash != candidate.hash() {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "observed Backup journal seal names another first candidate",
            ));
        }
        let path =
            activation::generation_directory(disk, candidate.authority_hash()).join("journal.bin");
        let mut file = root.open_read_regular(&path)?;
        let metadata = file.metadata()?;
        let identity = FileIdentity::from_metadata(&metadata);
        if metadata.nlink() != 1 || metadata.len() > MAX_SLOT_BYTES as u64 {
            return Err(Error::new(
                ErrorCode::LogCorrupt,
                "Backup first journal is not one bounded original file",
            ));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        (&mut file)
            .take(MAX_SLOT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_SLOT_BYTES || bytes.len() as u64 != metadata.len() {
            return Err(Error::new(
                ErrorCode::LogCorrupt,
                "Backup original journal changed its bounded length",
            ));
        }
        let scan = scan_frames_observed(bytes.as_slice(), JournalNamespace::Active, &mut |_| {
            crate::history_package::preparation_checkpoint(stop)
        })?;
        validate_prefix(&scan, candidate)?;
        if scan.valid_bytes != metadata.len() || scan.torn_tail || scan.records.len() != 3 {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "Backup original control requires the complete unchanged first journal",
            ));
        }
        let (record, frame_hash) = &scan.records[2];
        let receipt = Receipt {
            disk_id: disk.disk_id.clone(),
            slot_id: disk.slot_id.clone(),
            record: record.clone(),
            frame_hash: frame_hash.clone(),
        };
        if !seal.receipts.iter().any(|expected| expected == &receipt)
            || record.object_hash != seal.publication_hash
            || file.metadata()?.len() != metadata.len()
            || FileIdentity::from_metadata(&root.open_read_regular(&path)?.metadata()?) != identity
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "Backup ordinary journal differs from its real first Publication or original inode",
            ));
        }
        file.check_running()?;
        Ok((identity, blake3::hash(&bytes).to_hex().to_string()))
    }

    /// No directory creation, repair or fsync. Missing names remain missing.
    pub(crate) fn inspect(
        root: &DiskRoot,
        disk: &DiskConfig,
        candidate: &'a PreparedBackupFirstCandidate,
    ) -> Result<Option<Self>> {
        let directory = activation::generation_directory(disk, candidate.authority_hash());
        if root.optional_metadata(&directory)?.is_none() {
            return Ok(None);
        }
        Self::open(root, disk, candidate).map(Some)
    }

    /// Explicitly creates only the selected generation's directory. The caller
    /// has already checked ALL original choices, capacity and candidate bytes.
    pub(crate) fn prepare(
        root: &DiskRoot,
        disk: &DiskConfig,
        candidate: &'a PreparedBackupFirstCandidate,
    ) -> Result<Self> {
        Self::require_member(root, disk, candidate)?;
        root.private_dir(&activation::generation_directory(
            disk,
            candidate.authority_hash(),
        ))?;
        Self::open(root, disk, candidate)
    }
    fn require_member(
        root: &DiskRoot,
        disk: &DiskConfig,
        candidate: &PreparedBackupFirstCandidate,
    ) -> Result<()> {
        // No unbound bootstrap or prepared target can borrow an old member's ID.
        root.io_binding()?;
        let expected = candidate
            .original_config()
            .metadata_members()
            .find(|member| member.disk_id == disk.disk_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ConfigConflict,
                    "Backup first slot requires an original metadata member",
                )
            })?;
        if root.path() != disk.root.as_path() || canonical_json(expected)? != canonical_json(disk)?
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "Backup first slot member differs from the complete original binding",
            ));
        }
        Ok(())
    }
    fn open(
        root: &DiskRoot,
        disk: &DiskConfig,
        candidate: &'a PreparedBackupFirstCandidate,
    ) -> Result<Self> {
        Self::require_member(root, disk, candidate)?;
        let directory = activation::generation_directory(disk, candidate.authority_hash());
        let mut io = root.journal_directory(&directory)?;
        // The existing source gate still owns these writes; no successor
        // authority is installed in the resource merely to reach this path.
        io.require_configured(candidate.original_config())?;
        let scan = io.scan_bounded(JournalNamespace::Active, MAX_SLOT_BYTES)?;
        validate_prefix(&scan, candidate)?;
        Ok(Self {
            candidate,
            disk: disk.clone(),
            state: Some(SlotState::Scanned(OpenJournal {
                namespace: JournalNamespace::Active,
                io,
                scan,
                #[cfg(feature = "fault-injection")]
                opening_fault: None,
            })),
        })
    }
    fn records(&self) -> Result<&[(Record, String)]> {
        match &self.state {
            Some(SlotState::Scanned(opened)) => Ok(&opened.scan.records),
            Some(SlotState::Synchronized(writer)) => Ok(writer.records()),
            None => Err(Error::new(
                ErrorCode::RecoveryRequired,
                "Backup first journal failed; reopen the original task",
            )),
        }
    }
    pub(crate) fn accepted(&self) -> Result<Option<Receipt>> {
        Ok(self.records()?.first().map(|(record, frame_hash)| Receipt {
            disk_id: self.disk.disk_id.clone(),
            slot_id: self.disk.slot_id.clone(),
            frame_hash: frame_hash.clone(),
            record: record.clone(),
        }))
    }
    pub(crate) fn finalized_hash(&self) -> Result<Option<&str>> {
        Ok(self
            .records()?
            .get(1)
            .map(|(record, _)| record.object_hash.as_str()))
    }
    pub(crate) fn require_finalized(&self, hash: &str) -> Result<Receipt> {
        // A torn third frame is not a lost second-frame FINALIZE. It can only
        // be repaired by the exact publication path after complete proof I/O.
        if self.finalized_hash()? != Some(hash) {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "original member has no complete exact Backup FINALIZE",
            ));
        }
        let (record, frame_hash) = &self.records()?[1];
        Ok(Receipt {
            disk_id: self.disk.disk_id.clone(),
            slot_id: self.disk.slot_id.clone(),
            frame_hash: frame_hash.clone(),
            record: record.clone(),
        })
    }
    pub(crate) fn published_hash(&self) -> Result<Option<&str>> {
        Ok(self
            .records()?
            .get(2)
            .map(|(record, _)| record.object_hash.as_str()))
    }
    pub(crate) fn publication_started(&self) -> Result<bool> {
        Ok(self.records()?.len() == 3
            || matches!(&self.state, Some(SlotState::Scanned(opened)) if opened.scan.records.len() == 2 && opened.scan.torn_tail)
            || matches!(&self.state, Some(SlotState::Synchronized(writer)) if writer.records().len() == 2 && writer.needs_recovery()))
    }
    pub(crate) fn require_published(&self, hash: &str) -> Result<Receipt> {
        if self.published_hash()? != Some(hash) {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "original member has no exact first Backup Publication",
            ));
        }
        let (record, frame_hash) = &self.records()?[2];
        Ok(Receipt {
            disk_id: self.disk.disk_id.clone(),
            slot_id: self.disk.slot_id.clone(),
            frame_hash: frame_hash.clone(),
            record: record.clone(),
        })
    }
    pub(crate) fn accept(&mut self) -> Result<Receipt> {
        let hash = self.candidate.hash().to_owned();
        self.append(RecordType::Accept, &hash)
    }
    pub(crate) fn finalize(&mut self, finalize: &Finalize) -> Result<Receipt> {
        let accept = self.accepted()?.ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "FINALIZE cannot precede this member's actual ACCEPT",
            )
        })?;
        if finalize.candidate_hash != self.candidate.hash()
            || finalize.recovery_id.is_some()
            || !finalize.accepts.iter().any(|actual| actual == &accept)
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup FINALIZE does not bind this member's actual ACCEPT",
            ));
        }
        let hash = object_hash("finalize", finalize)?;
        if self.finalized_hash()?.is_some_and(|saved| saved != hash) {
            return Err(Error::new(
                ErrorCode::LogFork,
                "Backup first slot already finalized another receipt vector",
            ));
        }
        self.append(RecordType::Finalize, &hash)
    }
    pub(crate) fn publish(&mut self, publication: &Publication) -> Result<Receipt> {
        let own = self.require_finalized(&object_hash("finalize", &publication.finalize)?)?;
        if publication.candidate_hash != self.candidate.hash()
            || publication.finalize.candidate_hash != self.candidate.hash()
            || publication.recovery.is_some()
            || !publication
                .finalize_receipts
                .iter()
                .any(|receipt| receipt == &own)
            || canonical_json(&publication.result)? != canonical_json(self.candidate.result())?
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup Publication must bind the actual original FINALIZE and exact first result",
            ));
        }
        let hash = object_hash("publication", publication)?;
        if self.published_hash()?.is_some_and(|saved| saved != hash) {
            return Err(Error::new(
                ErrorCode::LogFork,
                "Backup first slot published another immutable result",
            ));
        }
        self.append(RecordType::Publication, &hash)
    }
    fn append(&mut self, kind: RecordType, hash: &str) -> Result<Receipt> {
        // Only the three private callers above select a record kind. A failure or
        // unwind consumes this writer until a fresh bounded prefix admission.
        let existing = self
            .records()?
            .iter()
            .find(|(record, _)| record.record_type == kind)
            .map(|(record, frame_hash)| Receipt {
                disk_id: self.disk.disk_id.clone(),
                slot_id: self.disk.slot_id.clone(),
                frame_hash: frame_hash.clone(),
                record: record.clone(),
            });
        if existing
            .as_ref()
            .is_some_and(|receipt| receipt.record.object_hash != hash)
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "Backup first-slot append conflicts with its existing frame",
            ));
        }
        let state = self.state.take().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Backup first journal needs reopen",
            )
        })?;
        let mut writer = match state {
            SlotState::Scanned(opened) => opened.into_writer()?.0,
            SlotState::Synchronized(writer) => writer,
        };
        writer.io.verify_singleton()?;
        let receipt = match existing {
            // Idempotent ACCEPT/FINALIZE must never erase a torn Publication.
            // Sync the actual existing prefix, but leave its suffix untouched.
            Some(receipt) => receipt,
            None => {
                writer.repair_torn_tail()?;
                writer.append(
                    &self.disk,
                    kind,
                    self.candidate.sequence(),
                    self.candidate.hash(),
                    hash,
                )?
            }
        };
        // Repeat both real barriers even for an existing receipt. Cache presence
        // is not a completed fsync after an earlier uncertain create/append.
        writer.io.synchronize(
            #[cfg(feature = "fault-injection")]
            None,
        )?;
        writer.io.verify_singleton()?;
        self.state = Some(SlotState::Synchronized(writer));
        Ok(receipt)
    }
}

fn validate_prefix(scan: &Scan, candidate: &PreparedBackupFirstCandidate) -> Result<()> {
    if scan.records.len() == 3 && scan.torn_tail {
        return Err(Error::new(
            ErrorCode::LogCorrupt,
            "bytes after complete Backup Publication cannot be repaired as a known first-slot tail",
        ));
    }
    if scan.records.len() > 3 {
        return Err(Error::new(
            ErrorCode::UnsupportedOperation,
            "Backup first-slot recovery cannot consume a later slot",
        ));
    }
    for (index, (record, _)) in scan.records.iter().enumerate() {
        let kind = match index {
            0 => RecordType::Accept,
            1 => RecordType::Finalize,
            2 => RecordType::Publication,
            _ => unreachable!("invariant: first slot has at most three frames"),
        };
        if record.record_type != kind
            || record.sequence != candidate.sequence()
            || record.candidate_hash != candidate.hash()
            || (index == 0 && record.object_hash != candidate.hash())
            || record.object_hash.len() != 64
            || !record
                .object_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "Backup first journal is not the exact ACCEPT/FINALIZE/Publication prefix",
            ));
        }
    }
    Ok(())
}

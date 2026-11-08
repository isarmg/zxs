//! A bounded, prescribed Backup first slot under the *original* writer leases.
//! This is not an active-generation JournalWriter. No caller can append a
//! Publication or a later slot, extract its writer, or clear the source fence.
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
        let torn = matches!(&self.state, Some(SlotState::Scanned(opened)) if opened.scan.torn_tail);
        if torn || self.finalized_hash()? != Some(hash) {
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
    fn append(&mut self, kind: RecordType, hash: &str) -> Result<Receipt> {
        // Only the two private callers above select a record kind. A failure or
        // unwind consumes this writer until a fresh bounded prefix admission.
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
        writer.repair_torn_tail()?;
        let receipt = writer.append(
            &self.disk,
            kind,
            self.candidate.sequence(),
            self.candidate.hash(),
            hash,
        )?;
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
    if scan.records.len() == 2 && scan.torn_tail {
        return Err(Error::new(
            ErrorCode::LogCorrupt,
            "bytes after complete Backup FINALIZE cannot be repaired as a known first-slot tail",
        ));
    }
    if scan.records.len() > 2 {
        return Err(Error::new(
            ErrorCode::UnsupportedOperation,
            "Backup first-slot recovery cannot consume a Publication or later record",
        ));
    }
    for (index, (record, _)) in scan.records.iter().enumerate() {
        let kind = if index == 0 {
            RecordType::Accept
        } else {
            RecordType::Finalize
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
                "Backup first journal is not the exact ACCEPT/FINALIZE prefix",
            ));
        }
    }
    Ok(())
}

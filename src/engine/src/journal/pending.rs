use super::*;

/// A single control bootstrap or member-bound joint accept in separate inactive
/// membership namespaces.
/// Opening only scans: repair, creation and durability fences require explicit synchronization.
/// The caller owns serialization and validates the archive before synchronization.
pub(crate) struct PendingJournal {
    state: Option<PendingState>,
    sequence: u64,
    candidate_hash: String,
    object_hash: String,
    namespace: JournalNamespace,
    #[cfg(feature = "fault-injection")]
    pub(crate) fault: Option<JournalFault>,
    #[cfg(feature = "fault-injection")]
    pub(crate) opening_fault: Option<OpeningFault>,
}

enum PendingState {
    Scanned(OpenJournal),
    Synchronized {
        writer: JournalWriter,
        directory: std::fs::File,
    },
}

impl PendingJournal {
    pub(crate) fn open_at(
        root: &DiskRoot,
        pending_directory: &Path,
        sequence: u64,
        object_hash: &str,
    ) -> Result<Self> {
        Self::open_binding(
            root,
            pending_directory,
            sequence,
            object_hash,
            object_hash,
            JournalNamespace::Pending,
        )
    }

    pub(crate) fn open_joint_at(
        root: &DiskRoot,
        directory: &Path,
        sequence: u64,
        proposal_hash: &str,
        acceptance_hash: &str,
    ) -> Result<Self> {
        Self::open_binding(
            root,
            directory,
            sequence,
            proposal_hash,
            acceptance_hash,
            JournalNamespace::Joint,
        )
    }

    fn open_binding(
        root: &DiskRoot,
        directory: &Path,
        sequence: u64,
        candidate_hash: &str,
        object_hash: &str,
        namespace: JournalNamespace,
    ) -> Result<Self> {
        let opened = OpenJournal::scan_directory(root.directory(directory)?, namespace)?;
        if opened.scan.records.iter().any(|(record, _)| {
            record.sequence != sequence
                || record.candidate_hash != candidate_hash
                || record.object_hash != object_hash
        }) {
            return Err(Error::new(
                ErrorCode::LogFork,
                "membership singleton journal binding differs",
            ));
        }
        Ok(Self {
            state: Some(PendingState::Scanned(opened)),
            sequence,
            candidate_hash: candidate_hash.into(),
            object_hash: object_hash.into(),
            namespace,
            #[cfg(feature = "fault-injection")]
            fault: None,
            #[cfg(feature = "fault-injection")]
            opening_fault: None,
        })
    }

    /// Inspection cannot manufacture a missing vote or repair torn bytes.
    pub(crate) fn require_complete(&self) -> Result<()> {
        match &self.state {
            Some(PendingState::Scanned(opened))
                if opened.scan.records.len() == 1 && !opened.scan.torn_tail =>
            {
                Ok(())
            }
            Some(PendingState::Synchronized { .. }) => Ok(()),
            _ => Err(Error::new(
                ErrorCode::RecoveryRequired,
                "membership journal is not a complete synchronized singleton",
            )),
        }
    }

    pub(crate) fn synchronize(&mut self, disk: &DiskConfig) -> Result<Receipt> {
        let state = self.state.take().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "pending journal I/O failed; reopen before synchronization",
            )
        })?;
        let (mut writer, directory) = match state {
            PendingState::Scanned(opened) => {
                #[cfg(feature = "fault-injection")]
                let opened = {
                    let mut opened = opened;
                    opened.opening_fault = self.opening_fault.take();
                    opened
                };
                let directory = opened.directory.try_clone()?;
                (opened.into_writer()?.0, directory)
            }
            PendingState::Synchronized { writer, directory } => {
                sync_open_journal(
                    &writer.file,
                    &directory,
                    #[cfg(feature = "fault-injection")]
                    self.opening_fault.take(),
                )?;
                (writer, directory)
            }
        };
        #[cfg(feature = "fault-injection")]
        {
            writer.fault = self.fault.take();
        }
        verify_named_singleton(&writer.file, &directory)?;
        writer.repair_torn_tail()?;
        let (kind, mode) = match self.namespace {
            JournalNamespace::Pending => {
                (RecordType::MembershipControlBootstrap, AppendMode::Pending)
            }
            JournalNamespace::Joint => (RecordType::MembershipJointAccept, AppendMode::Joint),
            JournalNamespace::Active => {
                unreachable!("invariant: membership journal is never active")
            }
        };
        let receipt = writer.append_mode(
            disk,
            kind,
            self.sequence,
            &self.candidate_hash,
            &self.object_hash,
            mode,
        )?;
        verify_named_singleton(&writer.file, &directory)?;
        // Retain the same descriptors and verified singleton. A repeated call must fence
        // both the file and its directory again before returning the existing receipt.
        self.state = Some(PendingState::Synchronized { writer, directory });
        Ok(receipt)
    }
}

fn verify_named_singleton(file: &std::fs::File, directory: &std::fs::File) -> Result<()> {
    use platform_io::fs_safety::linux::FileIdentity;
    let named = open_existing_journal(directory, &journal_name())?.ok_or_else(|| {
        Error::new(
            ErrorCode::LogFork,
            "membership journal name disappeared before receipt",
        )
    })?;
    if FileIdentity::from_fd(&named)? != FileIdentity::from_fd(file)? {
        return Err(Error::new(
            ErrorCode::LogFork,
            "membership journal name changed before receipt",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/journal/pending_tests.rs"]
mod tests;

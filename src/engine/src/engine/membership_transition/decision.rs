//! One immutable choice per intent and actual member root. Joint, certified
//! Abort and backup registry/placement share this name; staging objects and status caches never vote.
use super::*;
use crate::persist::layout;
use pending::MemberIdentity;
use platform_io::fs_safety::{AtomicFile, EntryName, linux::FileIdentity};
use std::path::PathBuf;

const DECISION_KIND: &str = "membership-decision";
const BINDING_FILE: &str = "binding.json";
const DECISION_FILE: &str = "decision.json";
const NAMESPACE: &str = "metadata/membership-decisions";

/// The domain choice is explicit at every caller. The stored Choice enum below
/// retains the exact native tags and core/proposal field names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DecisionKind {
    BackupRegistryPlacement,
    Readmission,
    Joint,
    Abort,
    TerminalResume,
    TerminalChainReadmission,
    TerminalChainIoAdmission,
    TerminalResumeCancel,
}
impl DecisionKind {
    fn record(
        self,
        transition: &str,
        identity: &MemberIdentity,
        value: &str,
        acceptance: &str,
    ) -> Result<DecisionRecord> {
        // Portable entry points admit the transition digest; stored requests
        // borrow it from the immutable binding admitted by DecisionStore::open.
        validate_hash(value)?;
        validate_hash(acceptance)?;
        let value = value.to_owned();
        let acceptance_hash = acceptance.to_owned();
        let choice = match self {
            Self::BackupRegistryPlacement => Choice::BackupRegistryPlacement {
                core_hash: value,
                acceptance_hash,
            },
            Self::Readmission => Choice::Readmission {
                proposal_hash: value,
                acceptance_hash,
            },
            Self::Joint => Choice::Joint {
                proposal_hash: value,
                acceptance_hash,
            },
            Self::Abort => Choice::Abort {
                core_hash: value,
                acceptance_hash,
            },
            Self::TerminalResume => Choice::TerminalResume {
                core_hash: value,
                acceptance_hash,
            },
            Self::TerminalChainReadmission => Choice::TerminalChainReadmission {
                core_hash: value,
                acceptance_hash,
            },
            Self::TerminalChainIoAdmission => Choice::TerminalChainIoAdmission {
                core_hash: value,
                acceptance_hash,
            },
            Self::TerminalResumeCancel => Choice::TerminalResumeCancel {
                core_hash: value,
                acceptance_hash,
            },
        };
        Ok(DecisionRecord {
            format: DecisionFormat::Decision,
            transition_hash: transition.to_owned(),
            identity: identity.clone(),
            choice,
        })
    }

    fn conflict(self) -> &'static str {
        match self {
            Self::BackupRegistryPlacement => "registry/placement member decision differs",
            Self::Readmission => "readmission decision differs",
            Self::Joint => "joint decision evidence differs",
            Self::Abort => "abort decision evidence differs",
            Self::TerminalResume => "terminal resume decision evidence differs",
            Self::TerminalChainReadmission => "terminal chain target decision differs",
            Self::TerminalChainIoAdmission => "terminal chain I/O decision differs",
            Self::TerminalResumeCancel => "terminal resume cancellation decision differs",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum SlotFormat {
    #[serialization(rename = "fenced-membership-slot")]
    Slot,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Binding {
    format: SlotFormat,
    transition_hash: String,
    identity: MemberIdentity,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum DecisionFormat {
    #[serialization(rename = "fenced-membership-decision")]
    Decision,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", deny_unknown_fields)]
enum Choice {
    BackupRegistryPlacement {
        core_hash: String,
        acceptance_hash: String,
    },
    Readmission {
        proposal_hash: String,
        acceptance_hash: String,
    },
    Joint {
        proposal_hash: String,
        acceptance_hash: String,
    },
    Abort {
        core_hash: String,
        acceptance_hash: String,
    },
    TerminalResume {
        core_hash: String,
        acceptance_hash: String,
    },
    TerminalChainReadmission {
        core_hash: String,
        acceptance_hash: String,
    },
    TerminalChainIoAdmission {
        core_hash: String,
        acceptance_hash: String,
    },
    TerminalResumeCancel {
        core_hash: String,
        acceptance_hash: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct DecisionRecord {
    format: DecisionFormat,
    transition_hash: String,
    identity: MemberIdentity,
    choice: Choice,
}
impl DecisionRecord {
    fn validate(&self, binding: &Binding) -> Result<()> {
        if self.transition_hash != binding.transition_hash || self.identity != binding.identity {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "membership decision differs from its intent or actual member identity",
            ));
        }
        let (value, acceptance) = match &self.choice {
            Choice::Joint {
                proposal_hash,
                acceptance_hash,
            }
            | Choice::Readmission {
                proposal_hash,
                acceptance_hash,
            } => (proposal_hash, acceptance_hash),
            Choice::BackupRegistryPlacement {
                core_hash,
                acceptance_hash,
            }
            | Choice::Abort {
                core_hash,
                acceptance_hash,
            }
            | Choice::TerminalResume {
                core_hash,
                acceptance_hash,
            }
            | Choice::TerminalResumeCancel {
                core_hash,
                acceptance_hash,
            }
            | Choice::TerminalChainIoAdmission {
                core_hash,
                acceptance_hash,
            }
            | Choice::TerminalChainReadmission {
                core_hash,
                acceptance_hash,
            } => (core_hash, acceptance_hash),
        };
        validate_hash(value)?;
        validate_hash(acceptance)
    }
}

/// Portable evidence embeds the complete decision, so its certificate's map
/// closure contains the choice without an out-of-band file dependency. Parsing
/// this value is not a local durability proof; only DecisionStore can confirm it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct DecisionReceipt {
    record_hash: String,
    record: DecisionRecord,
}
impl DecisionReceipt {
    /// Deterministic portable evidence for budgeting and certificate checking.
    /// Only DecisionStore establishes an actual local, synchronized choice.
    pub(super) fn plan(
        kind: DecisionKind,
        transition: &str,
        identity: &MemberIdentity,
        value: &str,
        acceptance: &str,
    ) -> Result<Self> {
        validate_hash(transition)?;
        let record = kind.record(transition, identity, value, acceptance)?;
        Ok(Self {
            record_hash: object_hash(DECISION_KIND, &record)?,
            record,
        })
    }

    pub(super) fn verify(
        &self,
        kind: DecisionKind,
        transition: &str,
        identity: &MemberIdentity,
        value: &str,
        acceptance: &str,
    ) -> Result<()> {
        validate_hash(transition)?;
        let expected = kind.record(transition, identity, value, acceptance)?;
        if self.record != expected || self.record_hash != object_hash(DECISION_KIND, &expected)? {
            return Err(Error::new(ErrorCode::LogFork, kind.conflict()));
        }
        Ok(())
    }

    pub(super) fn hash(&self) -> &str {
        &self.record_hash
    }
}

#[cfg(feature = "fault-injection")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DecisionFault {
    StagingWrite,
    PreparedFileSync,
    NoReplace,
    PublishedReply,
    UnknownPublication,
    UnknownPublicationFileSync,
    PublishedDurability,
    PublishedDurabilityDirectorySync,
    FileSync,
    DirectorySync,
}

pub(super) struct DecisionStore {
    disk: DiskRoot,
    root: DiskRoot,
    binding: Binding,
}
pub(super) enum TerminalResumeChoice {
    Vacant,
    Resume,
    Cancel,
}

impl DecisionStore {
    /// All choices use the same durable slot. Callers retain their distinct
    /// quorum, fresh-admission and Abort-minority policy before selecting it.
    pub(super) fn receipt(
        &self,
        kind: DecisionKind,
        value: &str,
        acceptance: &str,
        install: bool,
        #[cfg(feature = "fault-injection")] fault: Option<DecisionFault>,
    ) -> Result<DecisionReceipt> {
        let record = kind.record(
            &self.binding.transition_hash,
            &self.binding.identity,
            value,
            acceptance,
        )?;
        self.select(
            &record,
            install,
            #[cfg(feature = "fault-injection")]
            fault,
        )
    }

    pub(super) fn terminal_chain_io_slot_present(disk: &DiskRoot, epoch: &str) -> Result<bool> {
        validate_hash(epoch)?;
        Ok(disk
            .optional_metadata(&decision_directory(disk, epoch))?
            .is_some())
    }

    pub(super) fn require_backup_allowed(&self, core: &str, acceptance: &str) -> Result<()> {
        let expected = DecisionKind::BackupRegistryPlacement.record(
            &self.binding.transition_hash,
            &self.binding.identity,
            core,
            acceptance,
        )?;
        if let Some((file, record)) = self.observe()? {
            self.verify_named_record(&file, &record)?;
            if record != expected {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "another immutable decision blocks the registry/placement choice",
                ));
            }
        }
        Ok(())
    }

    /// A missing exact slot is neutral. Existing neutral binding staging is
    /// inspected without installing, synchronizing or choosing anything.
    pub(super) fn observe_terminal_resume(
        disk: &DiskRoot,
        identity: &MemberIdentity,
        transition: &str,
        proposal: &str,
        resume_acceptance: &str,
        cancellation: &str,
        cancel_acceptance: &str,
    ) -> Result<TerminalResumeChoice> {
        identity.verify_physical_root(disk)?;
        let mut path = disk.path().to_path_buf();
        for component in ["metadata", "membership-decisions", transition] {
            path.push(component);
            if disk.optional_metadata(&path)?.is_none() {
                return Ok(TerminalResumeChoice::Vacant);
            }
            disk.directory_identity(&path)?;
        }
        Self::inspect_preparation(disk, identity, transition)?;
        if disk
            .open_optional_read_regular(&path.join(BINDING_FILE))?
            .is_none()
        {
            return Ok(TerminalResumeChoice::Vacant);
        }
        let store = Self::open(disk, identity, transition, false)?;
        let Some((file, record)) = store.observe()? else {
            return Ok(TerminalResumeChoice::Vacant);
        };
        store.verify_named_record(&file, &record)?;
        let receipt = DecisionReceipt {
            record_hash: object_hash(DECISION_KIND, &record)?,
            record,
        };
        match &receipt.record.choice {
            Choice::TerminalResume { .. } => {
                receipt.verify(
                    DecisionKind::TerminalResume,
                    transition,
                    identity,
                    proposal,
                    resume_acceptance,
                )?;
                Ok(TerminalResumeChoice::Resume)
            }
            Choice::TerminalResumeCancel { .. } => {
                receipt.verify(
                    DecisionKind::TerminalResumeCancel,
                    transition,
                    identity,
                    cancellation,
                    cancel_acceptance,
                )?;
                Ok(TerminalResumeChoice::Cancel)
            }
            _ => Err(Error::new(
                ErrorCode::LogFork,
                "terminal resume slot contains a different decision kind",
            )),
        }
    }

    pub(super) fn open(
        disk: &DiskRoot,
        identity: &MemberIdentity,
        transition: &str,
        install: bool,
    ) -> Result<Self> {
        validate_hash(transition)?;
        identity.verify_physical_root(disk)?;
        reject_legacy_joint(disk, transition)?;
        let path = decision_directory(disk, transition);
        if install {
            disk.private_dir(&path)?;
        }
        let root = disk.private_child(&path)?;
        let binding = Binding {
            format: SlotFormat::Slot,
            transition_hash: transition.to_owned(),
            identity: identity.clone(),
        };
        let store = Self {
            disk: disk.private_child(disk.path())?,
            root,
            binding,
        };
        let binding_path = store.root.path().join(BINDING_FILE);
        let names = store.names()?;
        match store.root.open_optional_read_regular(&binding_path)? {
            Some(file) => {
                let saved: Binding = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
                if saved != store.binding {
                    return Err(Error::new(
                        ErrorCode::LogFork,
                        "membership decision slot binding differs",
                    ));
                }
            }
            None if install && names.iter().all(AtomicFile::is_temporary_name) => {
                store.verify_binding_staging(&names)?;
            }
            None => {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "membership decision slot lacks binding",
                ));
            }
        }
        if install {
            store
                .root
                .write_json(&binding_path, &store.binding, false)?;
        }
        store.observe()?;
        store.verify_names()?;
        Ok(store)
    }

    /// Enrollment may see an interrupted neutral-directory/binding install.
    /// Only empty directories or exact binding staging prefixes are unfinished
    /// preparation; a named decision without its binding is never Empty.
    pub(super) fn inspect_preparation(
        disk: &DiskRoot,
        identity: &MemberIdentity,
        transition: &str,
    ) -> Result<()> {
        validate_hash(transition)?;
        identity.verify_physical_root(disk)?;
        reject_legacy_joint(disk, transition)?;
        let root = disk.private_child(&decision_directory(disk, transition))?;
        let store = Self {
            disk: disk.private_child(disk.path())?,
            root,
            binding: Binding {
                format: SlotFormat::Slot,
                transition_hash: transition.to_owned(),
                identity: identity.clone(),
            },
        };
        if store
            .root
            .open_optional_read_regular(&store.root.path().join(BINDING_FILE))?
            .is_some()
        {
            store.observe()?;
        } else {
            store.verify_binding_staging(&store.names()?)?;
        }
        Ok(())
    }
    fn verify_binding_staging(&self, names: &[EntryName]) -> Result<()> {
        let bytes = encode_metadata(&self.binding, MAX_METADATA)?;
        for name in names {
            if !AtomicFile::is_temporary_name(name)
                || !bytes.starts_with(
                    &self
                        .root
                        .read_bytes(&self.root.path().join(name.as_path()), MAX_METADATA)?,
                )
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "unbound membership slot contains more than neutral preparation",
                ));
            }
        }
        Ok(())
    }
    fn names(&self) -> Result<Vec<EntryName>> {
        let names = self
            .root
            .entries(self.root.path())?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(fs_error)?;
        for name in &names {
            if AtomicFile::is_temporary_name(name) {
                // A complete or torn temp never occupies the decision slot.
                // Admit the regular leaf without promoting, repairing or deleting it.
                let path = self.root.path().join(name.as_path());
                let read =
                    self.root
                        .open_optional_read_regular(&path)
                        .and_then(|file| match file {
                            Some(file) => read_file_bytes(file, MAX_METADATA).map(|_| ()),
                            None => Ok(()),
                        });
                // Another chooser can unlink its losing temp between open
                // and single-link validation. Only proven name absence may
                // dismiss this non-voting staging error.
                if let Err(error) = read
                    && self.root.optional_metadata(&path)?.is_some()
                {
                    return Err(error);
                }
            } else if !matches!(
                name.as_os_str().to_str(),
                Some(BINDING_FILE | DECISION_FILE)
            ) {
                return Err(Error::new(
                    ErrorCode::UnsafePath,
                    "membership decision slot contains unrelated material",
                ));
            }
        }
        Ok(names)
    }
    pub(super) fn verify_names(&self) -> Result<()> {
        self.binding.identity.verify_physical_root(&self.disk)?;
        reject_legacy_joint(&self.disk, &self.binding.transition_hash)?;
        if self.disk.directory_identity(self.root.path())?
            != self.root.directory_identity(self.root.path())?
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "membership decision directory moved",
            ));
        }
        let saved: Binding = decode_strict(
            &self
                .root
                .read_bytes(&self.root.path().join(BINDING_FILE), MAX_METADATA)?,
        )?;
        if saved != self.binding {
            return Err(Error::new(
                ErrorCode::LogFork,
                "membership decision binding changed",
            ));
        }
        Ok(())
    }
    fn observe(&self) -> Result<Option<(ReadFile, DecisionRecord)>> {
        self.names()?;
        self.verify_names()?;
        let Some(file) = self
            .root
            .open_optional_read_regular(&self.root.path().join(DECISION_FILE))?
        else {
            return Ok(None);
        };
        let record: DecisionRecord =
            decode_strict(&read_file_bytes(file.try_clone()?, MAX_METADATA)?)?;
        record.validate(&self.binding)?;
        file.check_running()?;
        Ok(Some((file, record)))
    }
    /// Called for all original members before the first fresh Joint vote. The
    /// coordinator holds all original writer locks; online count never changes Q.
    pub(super) fn require_joint_allowed(&self) -> Result<()> {
        if let Some((_, record)) = self.observe()?
            && matches!(record.choice, Choice::Abort { .. })
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "membership abort choice already visible; joint voting is fenced",
            ));
        }
        Ok(())
    }
    /// Admit today's mutable chain only for an empty decision slot. An exact
    /// existing receipt remains immutable and is synchronized by receipt;
    /// another choice or an uncertain read is never treated as an empty slot.
    pub(super) fn admit_fresh_joint(
        &self,
        proposal: &str,
        acceptance: &str,
        admit: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let expected = DecisionKind::Joint.record(
            &self.binding.transition_hash,
            &self.binding.identity,
            proposal,
            acceptance,
        )?;
        match self.observe()? {
            None => admit(),
            Some((_, record)) if record == expected => Ok(()),
            Some(_) => Err(Error::new(
                ErrorCode::LogFork,
                "membership decision differs from the proposed joint choice",
            )),
        }
    }

    /// Read every available old slot before the first new choice. An impossible
    /// Abort quorum must not consume remaining slots and prevent a future Joint.
    pub(super) fn can_choose_abort(&self, core: &str, acceptance: &str) -> Result<bool> {
        let expected = self.abort_record(core, acceptance)?;
        match self.observe()? {
            Some((_, record))
                if matches!(
                    record.choice,
                    Choice::Joint { .. } | Choice::BackupRegistryPlacement { .. }
                ) =>
            {
                Ok(false)
            }
            Some((_, record)) if record != expected => Err(Error::new(
                ErrorCode::LogFork,
                "member selected another Abort core",
            )),
            _ => Ok(true),
        }
    }

    /// Joint/backup minorities are immutable, but do not prevent another fixed old
    /// majority choosing Abort. Any different Abort core remains a hard fork.
    pub(super) fn abort_receipt(
        &self,
        core: &str,
        acceptance: &str,
    ) -> Result<Option<DecisionReceipt>> {
        if let Some((_, record)) = self.observe()?
            && matches!(
                record.choice,
                Choice::Joint { .. } | Choice::BackupRegistryPlacement { .. }
            )
        {
            return Ok(None);
        }
        let record = self.abort_record(core, acceptance)?;
        self.select(
            &record,
            true,
            #[cfg(feature = "fault-injection")]
            None,
        )
        .map(Some)
    }

    pub(super) fn verify_abort_compatible(
        &self,
        core: &str,
        acceptance: &str,
        expected: Option<&DecisionReceipt>,
    ) -> Result<()> {
        if let Some(expected) = expected {
            return self.verify_receipt(expected, false);
        }
        if let Some((_, record)) = self.observe()?
            && matches!(record.choice, Choice::Abort { .. })
            && record != self.abort_record(core, acceptance)?
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "member selected another Abort core",
            ));
        }
        Ok(())
    }

    fn abort_record(&self, core: &str, acceptance: &str) -> Result<DecisionRecord> {
        DecisionKind::Abort.record(
            &self.binding.transition_hash,
            &self.binding.identity,
            core,
            acceptance,
        )
    }

    fn select(
        &self,
        expected: &DecisionRecord,
        install: bool,
        #[cfg(feature = "fault-injection")] fault: Option<DecisionFault>,
    ) -> Result<DecisionReceipt> {
        // This outer responsibility outlives every atomic staging descriptor,
        // uncertain-publication readback, sync and error cleanup in select_held.
        let activity = self.root.retain_io_activity()?;
        let result = self.select_held(
            expected,
            install,
            #[cfg(feature = "fault-injection")]
            fault,
        );
        match result {
            Ok(receipt) => {
                if let Some(activity) = &activity {
                    activity.check_running()?;
                }
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    }

    fn select_held(
        &self,
        expected: &DecisionRecord,
        install: bool,
        #[cfg(feature = "fault-injection")] fault: Option<DecisionFault>,
    ) -> Result<DecisionReceipt> {
        expected.validate(&self.binding)?;
        if let Some((file, record)) = self.observe()? {
            return self.confirm(
                file,
                record,
                expected,
                #[cfg(feature = "fault-injection")]
                fault,
            );
        }
        if !install {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "membership decision is absent",
            ));
        }
        let directory = self.root.private_directory(self.root.path())?;
        let name = EntryName::new(DECISION_FILE).map_err(fs_error)?;
        let staged = AtomicFile::stage(&directory, &name).map_err(fs_error)?;
        let bytes = encode_metadata(expected, MAX_METADATA)?;
        #[cfg(feature = "fault-injection")]
        if fault == Some(DecisionFault::StagingWrite) {
            // Interrupted bytes exist only in staging, never in the named slot.
            let _partial = staged
                .prepare(&bytes[..bytes.len() / 2])
                .map_err(fs_error)?;
            return Err(injected_io());
        }
        let prepared = staged.prepare(&bytes).map_err(fs_error)?;
        #[cfg(feature = "fault-injection")]
        if fault == Some(DecisionFault::PreparedFileSync) {
            // Fail at the prepared-file barrier, before the name can publish.
            return Err(injected_io());
        }
        self.verify_names()?;
        #[cfg(feature = "fault-injection")]
        if fault == Some(DecisionFault::NoReplace) {
            return Err(injected_io());
        }
        let publication = prepared.create();
        #[cfg(feature = "fault-injection")]
        let publication = publication.and_then(|file| {
            use platform_io::fs_safety::{AtomicCommitError, Error as FsError};
            // Simulate a lost syscall reply only after the actual atomic create.
            // Recovery below must reopen and fence the exact visible record.
            let error = || FsError::Io(std::io::Error::from_raw_os_error(5));
            match fault {
                Some(
                    DecisionFault::UnknownPublication | DecisionFault::UnknownPublicationFileSync,
                ) => Err(AtomicCommitError::Unknown(error())),
                Some(
                    DecisionFault::PublishedDurability
                    | DecisionFault::PublishedDurabilityDirectorySync,
                ) => Err(AtomicCommitError::Published(error())),
                _ => Ok(file),
            }
        });
        if let Err(error) = publication {
            // NOREPLACE collision and uncertain publication both require a real
            // read. Never overwrite, blindly retry, or infer Empty from errno.
            return match self.observe()? {
                Some((file, record)) => self.confirm(
                    file,
                    record,
                    expected,
                    #[cfg(feature = "fault-injection")]
                    fault,
                ),
                None => Err(fs_error(error.into_source())),
            };
        }
        #[cfg(feature = "fault-injection")]
        if fault == Some(DecisionFault::PublishedReply) {
            return Err(injected_io());
        }
        let (file, record) = self.observe()?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "published membership decision disappeared",
            )
        })?;
        self.confirm(
            file,
            record,
            expected,
            #[cfg(feature = "fault-injection")]
            fault,
        )
    }
    fn confirm(
        &self,
        file: ReadFile,
        record: DecisionRecord,
        expected: &DecisionRecord,
        #[cfg(feature = "fault-injection")] fault: Option<DecisionFault>,
    ) -> Result<DecisionReceipt> {
        if &record != expected {
            return Err(Error::new(
                ErrorCode::LogFork,
                "member already selected another membership decision",
            ));
        }
        #[cfg(feature = "fault-injection")]
        if matches!(
            fault,
            Some(DecisionFault::FileSync | DecisionFault::UnknownPublicationFileSync)
        ) {
            return Err(injected_io());
        }
        file.sync_all()?;
        #[cfg(feature = "fault-injection")]
        if matches!(
            fault,
            Some(DecisionFault::DirectorySync | DecisionFault::PublishedDurabilityDirectorySync)
        ) {
            return Err(injected_io());
        }
        self.root.sync_dir(self.root.path())?;
        self.verify_named_record(&file, &record)?;
        file.check_running()?;
        Ok(DecisionReceipt {
            record_hash: object_hash(DECISION_KIND, &record)?,
            record,
        })
    }
    fn verify_named_record(&self, file: &ReadFile, record: &DecisionRecord) -> Result<()> {
        self.verify_names()?;
        let (named, read_back) = self.observe()?.ok_or_else(|| {
            Error::new(ErrorCode::LogFork, "membership decision name disappeared")
        })?;
        if FileIdentity::from_metadata(&file.metadata()?)
            != FileIdentity::from_metadata(&named.metadata()?)
            || &read_back != record
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "membership decision changed before confirmation",
            ));
        }
        Ok(())
    }
    pub(super) fn verify_receipt(
        &self,
        expected: &DecisionReceipt,
        synchronize: bool,
    ) -> Result<()> {
        let (file, record) = self.observe()?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "certificate has no actual member decision",
            )
        })?;
        let actual = if synchronize {
            self.confirm(
                file,
                record,
                &expected.record,
                #[cfg(feature = "fault-injection")]
                None,
            )?
        } else {
            self.verify_named_record(&file, &record)?;
            DecisionReceipt {
                record_hash: object_hash(DECISION_KIND, &record)?,
                record,
            }
        };
        if actual != *expected {
            return Err(Error::new(
                ErrorCode::LogFork,
                "certificate differs from actual member decision",
            ));
        }
        self.verify_names()
    }
}
fn validate_hash(hash: &str) -> Result<()> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "invalid membership decision hash",
        ));
    }
    Ok(())
}
fn decision_directory(disk: &DiskRoot, transition: &str) -> PathBuf {
    disk.path().join(NAMESPACE).join(transition)
}
fn reject_legacy_joint(disk: &DiskRoot, transition: &str) -> Result<()> {
    let mut path = disk.path().to_path_buf();
    for component in ["metadata", "membership-pending", transition, "joint"] {
        path.push(component);
        if disk.optional_metadata(&path)?.is_none() {
            return Ok(());
        }
        disk.directory_identity(&path)?;
    }
    if disk.optional_metadata(&path.join(layout::JOURNAL.as_path()))?.is_some() {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "legacy joint journal cannot authorize or coexist with a shared decision slot",
        ));
    }
    Ok(())
}
#[cfg(feature = "fault-injection")]
fn injected_io() -> Error {
    std::io::Error::from_raw_os_error(5).into()
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/membership_decision_tests.rs"]
mod tests;

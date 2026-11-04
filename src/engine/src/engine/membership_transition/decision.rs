//! One immutable choice per intent and actual member root. Joint and future
//! certified abort share this name; staging objects and status caches never vote.
use super::*;
use pending::MemberIdentity;
use platform_io::fs_safety::{AtomicFile, EntryName, linux::FileIdentity};
use std::{fs::File, path::PathBuf};

const DECISION_KIND: &str = "membership-decision";
const BINDING_FILE: &str = "binding.json";
const DECISION_FILE: &str = "decision.json";
const NAMESPACE: &str = "metadata/membership-decisions";

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
    Joint {
        proposal_hash: String,
        acceptance_hash: String,
    },
    // Recognized to veto Joint and preserve the shared exclusion rule. There is
    // intentionally no production constructor or abort/cancellation authority.
    Abort {
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
            } => (proposal_hash, acceptance_hash),
            Choice::Abort {
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
    pub(super) fn hash(&self) -> &str {
        &self.record_hash
    }
    pub(super) fn verify_joint(
        &self,
        transition: &str,
        identity: &MemberIdentity,
        proposal: &str,
        acceptance: &str,
    ) -> Result<()> {
        let expected = joint_record(transition, identity, proposal, acceptance)?;
        if self.record != expected || self.record_hash != object_hash(DECISION_KIND, &expected)? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "joint decision evidence differs",
            ));
        }
        Ok(())
    }
    pub(super) fn verify_abort(
        &self,
        transition: &str,
        identity: &MemberIdentity,
        core: &str,
        acceptance: &str,
    ) -> Result<()> {
        validate_hash(transition)?;
        validate_hash(core)?;
        validate_hash(acceptance)?;
        let expected = DecisionRecord {
            format: DecisionFormat::Decision,
            transition_hash: transition.to_owned(),
            identity: identity.clone(),
            choice: Choice::Abort {
                core_hash: core.to_owned(),
                acceptance_hash: acceptance.to_owned(),
            },
        };
        if self.record != expected || self.record_hash != object_hash(DECISION_KIND, &expected)? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "abort decision evidence differs",
            ));
        }
        Ok(())
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
impl DecisionStore {
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
        match store.root.open_optional_regular(&binding_path)? {
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
            .open_optional_regular(&store.root.path().join(BINDING_FILE))?
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
                let read = self
                    .root
                    .open_optional_regular(&path)
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
    fn observe(&self) -> Result<Option<(File, DecisionRecord)>> {
        self.names()?;
        self.verify_names()?;
        let Some(file) = self
            .root
            .open_optional_regular(&self.root.path().join(DECISION_FILE))?
        else {
            return Ok(None);
        };
        let record: DecisionRecord =
            decode_strict(&read_file_bytes(file.try_clone()?, MAX_METADATA)?)?;
        record.validate(&self.binding)?;
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
    pub(super) fn joint_receipt(
        &self,
        proposal: &str,
        acceptance: &str,
        install: bool,
        #[cfg(feature = "fault-injection")] fault: Option<DecisionFault>,
    ) -> Result<DecisionReceipt> {
        let record = joint_record(
            &self.binding.transition_hash,
            &self.binding.identity,
            proposal,
            acceptance,
        )?;
        self.select(
            &record,
            install,
            #[cfg(feature = "fault-injection")]
            fault,
        )
    }
    #[cfg(test)]
    pub(super) fn abort_for_test(&self, core: &str, acceptance: &str) -> Result<DecisionReceipt> {
        let record = DecisionRecord {
            format: DecisionFormat::Decision,
            transition_hash: self.binding.transition_hash.clone(),
            identity: self.binding.identity.clone(),
            choice: Choice::Abort {
                core_hash: core.to_owned(),
                acceptance_hash: acceptance.to_owned(),
            },
        };
        self.select(
            &record,
            true,
            #[cfg(feature = "fault-injection")]
            None,
        )
    }
    fn select(
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
        file: File,
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
        Ok(DecisionReceipt {
            record_hash: object_hash(DECISION_KIND, &record)?,
            record,
        })
    }
    fn verify_named_record(&self, file: &File, record: &DecisionRecord) -> Result<()> {
        self.verify_names()?;
        let (named, read_back) = self.observe()?.ok_or_else(|| {
            Error::new(ErrorCode::LogFork, "membership decision name disappeared")
        })?;
        if FileIdentity::from_fd(file)? != FileIdentity::from_fd(&named)? || &read_back != record {
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
fn joint_record(
    transition: &str,
    identity: &MemberIdentity,
    proposal: &str,
    acceptance: &str,
) -> Result<DecisionRecord> {
    validate_hash(transition)?;
    validate_hash(proposal)?;
    validate_hash(acceptance)?;
    Ok(DecisionRecord {
        format: DecisionFormat::Decision,
        transition_hash: transition.to_owned(),
        identity: identity.clone(),
        choice: Choice::Joint {
            proposal_hash: proposal.to_owned(),
            acceptance_hash: acceptance.to_owned(),
        },
    })
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
    if disk.optional_metadata(&path.join("journal.bin"))?.is_some() {
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

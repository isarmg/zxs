//! Independent control metadata archive. The root is not a transition decision,
//! target readiness, complete evacuation archive, or a grant of write authority.
use super::*;
use crate::metadata_map::{Encoder, ProofMeasure, Reference};
use platform_io::fs_safety::linux::FileIdentity;
use platform_io::fs_safety::{AdvisoryLock, RelativePath};
use std::path::{Path, PathBuf};

#[path = "namespace_replay.rs"]
pub(in crate::engine::membership_transition) mod namespace_replay;

const ROOT_KIND: &str = "membership-control-archive";
pub(super) const SEAL_KIND: &str = "membership-publication-seal";
const ROOT_FILE: &str = "archive-root.json";
const BINDING_FILE: &str = "archive-binding.json";
const LOCK_FILE: &str = ".archive.lock";
const EVIDENCE_DIRECTORY: &str = "authority-evidence";

#[path = "../abort/archive_view.rs"]
pub(in crate::engine::membership_transition) mod abort_view;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "frozen-control-metadata")]
    FrozenControlMetadata,
    #[serialization(rename = "retained-exit-quorum-control-metadata")]
    RetainedExitQuorum,
}
impl Format {
    fn for_intent(intent: &TransitionIntent) -> Self {
        match intent.fence_scope {
            None => Self::FrozenControlMetadata,
            Some(FenceScope::RetainedExitQuorum) => Self::RetainedExitQuorum,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct ArchiveBinding {
    format: Format,
    transition_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine::membership_transition) struct ArchiveRoot {
    format: Format,
    transition_hash: String,
    members: Reference,
    pub(in crate::engine::membership_transition) publication_seals: Reference,
}
impl ArchiveRoot {
    pub(in crate::engine::membership_transition) fn verify_scope(
        &self,
        intent: &TransitionIntent,
    ) -> Result<()> {
        intent.verify_fence_scope()?;
        if self.format != Format::for_intent(intent)
            || self.transition_hash != object_hash(INTENT_KIND, intent)?
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "archive proof scope differs from its exact transition",
            ));
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct ArchivedMember {
    journal: MembershipJournalInspection,
    config_bytes_hash: String,
    seal_bytes_hash: String,
    fence: Receipt,
}

/// Keep these digests in a trusted caller context. A directory's self-declared
/// root is never an authority to select a transition or permit new writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipControlArchive {
    /// Diagnostic locator only; root_hash remains the independent trust anchor.
    pub archive_path: PathBuf,
    pub root_hash: String,
    pub transition_hash: String,
}

pub(super) struct ArchiveStore {
    root: DiskRoot,
}
pub(super) fn predecessor_object(
    engine: &Engine,
    kind: &'static str,
    hash: &str,
) -> Result<AuthenticatedMetadataObject> {
    let generation = engine.config.generation().ok_or_else(|| {
        Error::new(
            ErrorCode::ObjectCorrupt,
            "Genesis has no predecessor archive",
        )
    })?;
    let transition = object_hash(INTENT_KIND, &generation.portable.intent()?)?;
    let disk = engine
        .config
        .metadata_members()
        .find(|disk| engine.disk_resources.available(disk).is_some())
        .expect("invariant: fixed metadata membership");
    let store = ArchiveStore {
        root: engine.disk_resources.get(disk)?.root.private_child(
            &pending::member_directory(disk, &transition).join(pending::CONTROL_ARCHIVE_DIRECTORY),
        )?,
    };
    store.load(kind, hash)
}
impl ArchiveStore {
    /// Destination must already be a private, empty directory or this exact
    /// in-progress binding. No chmod, mounting, overwrites or foreign cleanup.
    fn admit(root: DiskRoot, transition: &str, format: Format) -> Result<(Self, AdvisoryLock)> {
        let store = Self { root };
        let names = store
            .root
            .entries(store.root.path())?
            .map(|entry| entry.map_err(fs_error))
            .collect::<Result<Vec<_>>>()?;
        if !names.is_empty() {
            let binding: ArchiveBinding = decode_strict(
                &store
                    .root
                    .read_bytes(&store.path(BINDING_FILE), MAX_METADATA)?,
            )?;
            if binding.transition_hash != transition || binding.format != format {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "archive belongs to another transition",
                ));
            }
            for name in names {
                // Only after the exact binding is admitted, preserve reserved
                // atomic staging left by an uncertain publication. It is never
                // evidence, overwritten, renamed into place, or cleaned here.
                if platform_io::fs_safety::AtomicFile::is_temporary_name(&name) {
                    store
                        .root
                        .open_read_regular(&store.root.path().join(name.as_path()))?;
                    continue;
                }
                if !matches!(
                    name.as_os_str().to_str(),
                    Some(
                        BINDING_FILE
                            | LOCK_FILE
                            | ROOT_FILE
                            | "objects"
                            | "journals"
                            | EVIDENCE_DIRECTORY
                    )
                ) {
                    return Err(Error::new(
                        ErrorCode::UnsafePath,
                        "archive destination contains unrelated entries",
                    ));
                }
            }
        }
        store.root.write_json(
            &store.path(BINDING_FILE),
            &ArchiveBinding {
                format,
                transition_hash: transition.to_owned(),
            },
            false,
        )?;
        let directory = store.root.private_directory(store.root.path())?;
        let lock =
            AdvisoryLock::acquire(&directory, &RelativePath::new(LOCK_FILE).map_err(fs_error)?)
                .map_err(fs_error)?;
        Ok((store, lock))
    }
    fn path(&self, path: &str) -> PathBuf {
        self.root.path().join(path)
    }
    fn object_path(&self, kind: &str, hash: &str) -> Result<PathBuf> {
        if !valid_hash(hash)
            || kind.is_empty()
            || !kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "invalid archive object address",
            ));
        }
        Ok(self
            .path("objects")
            .join(kind)
            .join(&hash[..2])
            .join(format!("{hash}.json")))
    }
    pub(super) fn object_bytes(&self, kind: &str, hash: &str, bytes: &[u8]) -> Result<()> {
        self.root
            .write_atomic(&self.object_path(kind, hash)?, bytes, false)
    }
    fn evidence_path(&self, hash: &str) -> Result<PathBuf> {
        if !valid_hash(hash) {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "invalid authority evidence address",
            ));
        }
        Ok(self.path(EVIDENCE_DIRECTORY).join(format!("{hash}.json")))
    }
    fn evidence(
        &self,
        hash: &str,
        observe: &mut dyn FnMut(usize) -> Result<()>,
    ) -> Result<AbortAuthorityEvidence> {
        let bytes = self.root.read_bytes_observed(
            &self.evidence_path(hash)?,
            MAX_METADATA,
            &mut |bytes| observe(bytes),
        )?;
        decode_strict(&bytes).map_err(Into::into)
    }
    /// The name binds G, not a hash of this envelope. Different valid witnesses
    /// retain the first complete bytes; corrupt or rebound evidence is not replaced.
    pub(super) fn install_evidence(
        &self,
        hash: &str,
        evidence: &AbortAuthorityEvidence,
    ) -> Result<()> {
        PortableAuthority::verify_detached_evidence(self.load(AUTHORITY_KIND, hash)?, evidence)?;
        let path = self.evidence_path(hash)?;
        self.root.private_dir(&self.path(EVIDENCE_DIRECTORY))?;
        if let Some(file) = self.root.open_optional_read_regular(&path)? {
            let bytes = read_file_bytes(file, MAX_METADATA)?;
            let saved: AbortAuthorityEvidence = decode_strict(&bytes)?;
            PortableAuthority::verify_detached_evidence(self.load(AUTHORITY_KIND, hash)?, &saved)?;
            // Reuse the held operation's immutable comparison and both sync
            // tails; a raw File must not outlive its disk I/O admission here.
            return self.root.write_atomic(&path, &bytes, false);
        }
        self.root.write_json(&path, evidence, false)
    }
    fn verify_evidence_names(&self, expected: &BTreeSet<String>) -> Result<()> {
        let path = self.path(EVIDENCE_DIRECTORY);
        if self.root.optional_metadata(&path)?.is_none() {
            return if expected.is_empty() {
                Ok(())
            } else {
                Err(Error::new(
                    ErrorCode::ObjectCorrupt,
                    "archive lacks authority evidence directory",
                ))
            };
        }
        let mut actual = BTreeSet::new();
        for entry in self.root.entries(&path)? {
            let entry = entry.map_err(fs_error)?;
            // Atomic publication may have reached an uncertain rename result
            // or stopped before rename. Preserve its regular staging leaf,
            // but never count it as authority evidence or promote it here.
            if platform_io::fs_safety::AtomicFile::is_temporary_name(&entry) {
                self.root.open_read_regular(&path.join(entry.as_path()))?;
                continue;
            }
            let name = entry.as_os_str().to_str().ok_or_else(|| {
                Error::new(ErrorCode::UnsafePath, "invalid authority evidence name")
            })?;
            let hash = name
                .strip_suffix(".json")
                .filter(|hash| valid_hash(hash))
                .ok_or_else(|| {
                    Error::new(ErrorCode::UnsafePath, "unrelated authority evidence entry")
                })?;
            self.root.open_read_regular(&path.join(entry.as_path()))?;
            actual.insert(hash.to_owned());
        }
        if &actual != expected {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "archive authority evidence set differs from exact ancestry",
            ));
        }
        Ok(())
    }
    fn load(&self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        let bytes = self
            .root
            .read_bytes(&self.object_path(kind, hash)?, MAX_METADATA)?;
        AuthenticatedMetadataObject::authenticate(kind, hash, decode_strict(&bytes)?)
    }
    fn load_observed(
        &self,
        kind: &'static str,
        hash: &str,
        observe: &mut dyn FnMut(usize) -> Result<()>,
    ) -> Result<AuthenticatedMetadataObject> {
        let bytes = self.root.read_bytes_observed(
            &self.object_path(kind, hash)?,
            MAX_METADATA,
            &mut |bytes| observe(bytes),
        )?;
        AuthenticatedMetadataObject::authenticate(kind, hash, decode_strict(&bytes)?)
    }
    // Components come from the independently validated original Config, not
    // from a file path in the archive manifest. This is never an active journal.
    fn member_directory(&self, disk: &DiskConfig, transition: Option<&str>) -> PathBuf {
        let root = self.path("journals");
        let root = transition.map_or_else(|| root.clone(), |hash| root.join(hash));
        root.join(&disk.disk_id)
            .join(&disk.slot_id)
            .join(disk.binding_generation.to_string())
    }
    fn map(&self, values: json_codec::Map<String, json_codec::Value>) -> Result<Reference> {
        let mut encoder = Encoder::new(MAX_METADATA);
        let root = encoder
            .map_root(values, ProofMeasure::Plain)?
            .reference()
            .clone();
        for object in encoder.finish() {
            self.object_bytes(object.kind(), &object.hash, object.bytes())?;
        }
        Ok(root)
    }
    fn copy_member(
        &self,
        engine: &Engine,
        journal: &MembershipJournalInspection,
    ) -> Result<ArchivedMember> {
        let disk = engine
            .config
            .metadata_members()
            .find(|disk| disk.disk_id == journal.disk_id)
            .expect("invariant: inspection returned an original member");
        let handle = engine.disk_resources.get(disk)?;
        let transition = engine
            .membership_fence
            .as_ref()
            .expect("invariant: archived member is fenced");
        let namespace = transition
            .intent
            .origin
            .predecessor()
            .map(|_| transition.hash.as_str());
        let directory = self.member_directory(disk, namespace);
        let config = handle.root.read_bytes(
            &disk.root.join("metadata/pool-membership/config.json"),
            MAX_METADATA,
        )?;
        let saved: ValidatedConfig = decode_strict(&config)?;
        match engine.config.generation() {
            Some(generation) => generation
                .original()
                .verify_saved_metadata_membership(&saved)?,
            None => engine.config.verify_saved_metadata_membership(&saved)?,
        }
        let seal = handle
            .root
            .read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?;
        validate_stored_seal(&decode_seal(&seal)?, &handle.seal)?;
        self.root
            .write_atomic(&directory.join("config.json"), &config, false)?;
        self.root
            .write_atomic(&directory.join("disk.json"), &seal, false)?;
        self.copy_journal(
            &directory,
            handle
                .root
                .open_read_regular(&pending::activation::log_path(&engine.config, disk))?,
            journal,
        )?;
        Ok(ArchivedMember {
            journal: journal.clone(),
            config_bytes_hash: content_hash(&config),
            seal_bytes_hash: content_hash(&seal),
            fence: engine
                .membership_fence
                .as_ref()
                .expect("invariant: archive requires fence")
                .receipts[&disk.disk_id]
                .clone(),
        })
    }
    fn copy_journal(
        &self,
        directory: &Path,
        mut input: ReadFile,
        expected: &MembershipJournalInspection,
    ) -> Result<()> {
        let digest = blake3::Hash::from_hex(&expected.content_hash)
            .expect("invariant: journal inspection produced BLAKE3 hex");
        self.root.copy_verified_bytes_synced(
            &directory.join("journal.bin"),
            &mut input,
            platform_io::payload::Expectation {
                bytes: expected.bytes,
                blake3: Some(digest),
            },
        )
    }
}
impl AuthorityResolver for &ArchiveStore {
    fn core_object(&mut self, hash: &str) -> Result<AuthenticatedMetadataObject> {
        self.load(AUTHORITY_KIND, hash)
    }
    fn abort_evidence(&mut self, hash: &str) -> Result<AbortAuthorityEvidence> {
        self.evidence(hash, &mut |_| Ok(()))
    }
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}
struct ArchivedSource<'a> {
    store: &'a ArchiveStore,
    observe: &'a mut dyn FnMut(usize) -> Result<()>,
    object_budget: Option<usize>,
    seals: BTreeMap<String, String>,
    destination: Option<&'a ArchiveStore>,
    copied: BTreeSet<(&'static str, String)>,
    namespace: Option<String>,
}
impl ArchivedSource<'_> {
    fn remember(&mut self, object: &AuthenticatedMetadataObject) -> Result<()> {
        let address = (object.kind(), object.hash().to_owned());
        if !self.copied.contains(&address) {
            if self
                .object_budget
                .is_some_and(|limit| self.copied.len() == limit)
            {
                return Err(Error::new(
                    ErrorCode::ResourceLimit,
                    "frozen archive scrub object budget exhausted",
                ));
            }
            if let Some(destination) = self.destination {
                destination.object_bytes(object.kind(), object.hash(), object.bytes())?;
            }
            self.copied.insert(address);
        }
        Ok(())
    }
}
impl ControlSource for ArchivedSource<'_> {
    fn predecessor_evidence(&mut self, hash: &str) -> Result<AbortAuthorityEvidence> {
        self.observe(0)?;
        let evidence = self.store.evidence(hash, self.observe)?;
        let address = (EVIDENCE_DIRECTORY, hash.to_owned());
        if !self.copied.contains(&address) {
            if self
                .object_budget
                .is_some_and(|limit| self.copied.len() == limit)
            {
                return Err(Error::new(
                    ErrorCode::ResourceLimit,
                    "frozen archive scrub evidence budget exhausted",
                ));
            }
            if let Some(destination) = self.destination {
                destination.install_evidence(hash, &evidence)?;
            }
            self.copied.insert(address);
        }
        Ok(evidence)
    }
    fn observe(&mut self, bytes: usize) -> Result<()> {
        (self.observe)(bytes)
    }
    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        (self.observe)(0)?;
        let object = self.store.load_observed(kind, hash, self.observe)?;
        self.remember(&object)?;
        Ok(object)
    }
    fn seal(
        &mut self,
        candidate: &Candidate,
        publication: &Publication,
    ) -> Result<PublicationSeal> {
        let candidate_hash = object_hash("candidate", candidate)?;
        let hash = self.seals.get(&candidate_hash).ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "archive omits publication seal",
            )
        })?;
        let hash = hash.clone();
        let seal = json_codec::from_value(self.object(SEAL_KIND, &hash)?.into_value())?;
        verify_publication_seal(candidate, publication, &seal)?;
        Ok(seal)
    }
    fn journal(&self, disk: &DiskConfig) -> Result<ReadFile> {
        self.store.root.open_read_regular(
            &self
                .store
                .member_directory(disk, self.namespace.as_deref())
                .join("journal.bin"),
        )
    }
}

impl Engine {
    fn managed_control_archive_path(&self, intent: &TransitionIntent) -> PathBuf {
        self.root
            .join("membership-control-archives")
            .join(intent.operation_id.to_string())
    }

    /// The only system-root exception is this operation's reserved namespace.
    /// No archive may alias or overlap an original or proposed storage root.
    fn require_control_archive_location(
        &self,
        destination: &Path,
        intent: &TransitionIntent,
    ) -> Result<()> {
        for root in self.config.disks.iter().chain(intent.target.disks.iter()) {
            let root = absolute_path(&root.root)?;
            if destination.starts_with(&root) || root.starts_with(destination) {
                return Err(Error::new(
                    ErrorCode::UnsafePath,
                    "control archive overlaps a registered storage directory",
                ));
            }
        }
        let system = absolute_path(&self.root)?;
        if system.starts_with(destination)
            || (destination.starts_with(&system)
                && destination != self.managed_control_archive_path(intent))
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "control archive is outside the reserved operation namespace",
            ));
        }
        Ok(())
    }

    /// Build the frozen control evidence in this operation's system-managed
    /// private directory. Users need not provision a second storage directory.
    /// This does not enrol another disk or create a deregistration backup.
    pub fn build_managed_membership_control_archive(&mut self) -> Result<MembershipControlArchive> {
        let fence = self.membership_fence.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "archive requires a membership fence",
            )
        })?;
        let path = self.managed_control_archive_path(&fence.intent);
        self.require_control_archive_location(&path, &fence.intent)?;
        let system = DiskRoot::open(&self.root)?;
        let named_lock = system.open_read_regular(&self.root.join("writer.lock"))?;
        if FileIdentity::from_metadata(&named_lock.metadata()?) != self._locks.system_identity()? {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "system writer identity changed before archive preparation",
            ));
        }
        system.private_dir(&path)?;
        self.build_membership_control_archive(&path)
    }

    /// A rename after Engine admission must not defeat the lexical exclusion
    /// above. Reopen only original names to compare with held identities; never
    /// use those reopened handles as sources. Target paths are not admitted here.
    fn admit_abort_control_archive_directory(&self, destination: &Path) -> Result<DiskRoot> {
        let archive = DiskRoot::open(destination)?;
        let identity = FileIdentity::from_fd(archive.directory(archive.path())?)?;
        // A complete F does not need every original to remain reachable. Compare
        // against their already-admitted identities, then recheck each live voter.
        if self.disk_resources.handles().any(|handle| {
            handle.seal.device == identity.device && handle.seal.inode == identity.inode
        }) {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "Abort archive aliases an original root",
            ));
        }
        let system = DiskRoot::open(&self.root)?;
        let lock = system.open_read_regular(&self.root.join("writer.lock"))?;
        if FileIdentity::from_metadata(&lock.metadata()?) != self._locks.system_identity()?
            || identity == FileIdentity::from_fd(system.directory(system.path())?)?
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "Abort archive aliases or differs from held system identity",
            ));
        }
        Ok(archive)
    }

    fn admit_control_archive_directory(&self, destination: &Path) -> Result<DiskRoot> {
        if self
            .membership_fence
            .as_ref()
            .is_some_and(|fence| fence.intent.fence_scope.is_some())
        {
            // The scoped journal admission separately checks every retained
            // named root. Do not probe the explicitly absent departing member.
            return self.admit_abort_control_archive_directory(destination);
        }
        let destination = DiskRoot::open(destination)?;
        let destination_identity =
            FileIdentity::from_fd(destination.directory(destination.path())?)?;
        for handle in self.disk_resources.handles() {
            let activity = handle.root.admit_io()?;
            let held_identity = FileIdentity::from_fd(handle.root.directory(handle.root.path())?)?;
            activity.check_running()?;
            let named = DiskRoot::open(handle.root.path())?;
            let named_identity = FileIdentity::from_fd(named.directory(named.path())?)?;
            if named_identity != held_identity || destination_identity == held_identity {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "original directory moved or aliases archive destination",
                ));
            }
            activity.check_running()?;
            // `named` drops before its activity on success and every error.
        }
        let system = DiskRoot::open(&self.root)?;
        let named_lock = system.open_regular(&self.root.join("writer.lock"))?;
        if FileIdentity::from_fd(named_lock)? != self._locks.system_identity()?
            || destination_identity == FileIdentity::from_fd(system.directory(system.path())?)?
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "system directory moved or aliases archive destination",
            ));
        }
        Ok(destination)
    }
    /// Materialize a frozen control archive in an explicitly supplied private
    /// directory. Errors leave source and already installed archive bytes intact.
    /// Retry only this same transition. Completion grants no new write authority.
    pub fn build_membership_control_archive(
        &mut self,
        destination: &Path,
    ) -> Result<MembershipControlArchive> {
        let destination = absolute_path(destination)?;
        let fence = self.membership_fence.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "archive requires a membership fence",
            )
        })?;
        self.require_control_archive_location(&destination, &fence.intent)?;
        let format = Format::for_intent(&fence.intent);
        let inspected = self.inspect_membership_metadata_archive()?;
        let destination = self.admit_control_archive_directory(&destination)?;
        let (store, _lock) = ArchiveStore::admit(destination, &inspected.transition_hash, format)?;
        if let Some(generation) = self.config.generation() {
            let (previous, previous_intent) = generation
                .portable
                .archives()?
                .pop()
                .expect("invariant: active authority has its frozen archive");
            let transition = object_hash(INTENT_KIND, &previous_intent)?;
            let disk = self
                .config
                .metadata_members()
                .find(|disk| self.disk_resources.available(disk).is_some())
                .expect("invariant: fixed metadata membership");
            let source = ArchiveStore {
                root: self.disk_resources.get(disk)?.root.private_child(
                    &pending::member_directory(
                        disk,
                        &object_hash(INTENT_KIND, &generation.portable.intent()?)?,
                    )
                    .join(pending::CONTROL_ARCHIVE_DIRECTORY),
                )?,
            };
            verify_root_copying(
                &source,
                &previous,
                &transition,
                Some(&store),
                &mut |_| Ok(()),
                None,
            )?;
            for object in generation.portable.node_objects()? {
                store.object_bytes(object.kind(), &object.hash, object.bytes())?;
            }
            for (hash, evidence) in generation.portable.evidence_entries() {
                store.install_evidence(hash, evidence)?;
            }
        }
        let mut members = json_codec::Map::new();
        for journal in &inspected.journals {
            members.insert(
                journal.disk_id.clone(),
                json_codec::to_value(store.copy_member(self, journal)?)?,
            );
        }
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_membership_archive_journals")?;
        let fence = self
            .membership_fence
            .as_ref()
            .expect("invariant: archive requires fence");
        let mut source = LiveSource {
            engine: self,
            members: fence.intent.fence_members_at(&self.config)?,
            destination: Some(&store),
            seals: BTreeMap::new(),
            copied: BTreeSet::new(),
        };
        let copied = inspect_control(&self.config, fence, &mut source)?;
        if copied != inspected {
            return Err(Error::new(
                ErrorCode::LogFork,
                "source changed while building control archive",
            ));
        }
        let seals = source
            .seals
            .into_iter()
            .map(|(key, hash)| (key, hash.into()))
            .collect();
        let root = ArchiveRoot {
            format,
            transition_hash: inspected.transition_hash.clone(),
            members: store.map(members)?,
            publication_seals: store.map(seals)?,
        };
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_membership_archive_metadata")?;
        // No live disks are passed to this check. A good source replica cannot
        // mask a missing or corrupt copied object or journal.
        let verified = verify_root(&store, &root, &inspected.transition_hash)?;
        if verified != inspected {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "independent archive inspection differs",
            ));
        }
        let prepared = PreparedMetadataObject::new(ROOT_KIND, &root, MAX_METADATA)?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("before_membership_archive_complete")?;
        store
            .root
            .write_atomic(&store.path(ROOT_FILE), prepared.bytes(), false)?;
        Ok(MembershipControlArchive {
            archive_path: store.root.path().to_owned(),
            root_hash: prepared.hash,
            transition_hash: inspected.transition_hash,
        })
    }
}

/// A held, independently verified archive admitted against the Engine's actual
/// old fence. Only the pending-member consumer can install it below target roots.
pub(in crate::engine::membership_transition) struct PendingControlArchive {
    store: ArchiveStore,
    root: ArchiveRoot,
    expected_root: String,
    inspection: MembershipArchiveInspection,
}
impl PendingControlArchive {
    pub(in crate::engine::membership_transition) fn open(
        engine: &Engine,
        path: &Path,
        expected_root: &str,
    ) -> Result<Self> {
        let archive = Self::open_abort(engine, path, expected_root)?;
        archive.verify_current_fence(engine)?;
        Ok(archive)
    }

    /// Complete independent F admission. Abort checks only its actual available
    /// original voters against F, without requiring every old or target root.
    pub(in crate::engine::membership_transition) fn open_abort(
        engine: &Engine,
        path: &Path,
        expected_root: &str,
    ) -> Result<Self> {
        if !valid_hash(expected_root) {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "invalid expected archive root",
            ));
        }
        let path = absolute_path(path)?;
        let fence = engine.membership_fence.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "pending enrollment requires original fence",
            )
        })?;
        engine.require_control_archive_location(&path, &fence.intent)?;
        let store = ArchiveStore {
            root: engine.admit_abort_control_archive_directory(&path)?,
        };
        let root = json_codec::from_value(
            AuthenticatedMetadataObject::authenticate(
                ROOT_KIND,
                expected_root,
                decode_strict(
                    &store
                        .root
                        .read_bytes(&store.path(ROOT_FILE), MAX_METADATA)?,
                )?,
            )?
            .into_value(),
        )?;
        let inspection = verify_root(&store, &root, &fence.hash)?;
        let archive = Self {
            store,
            root,
            expected_root: expected_root.to_owned(),
            inspection,
        };
        Ok(archive)
    }

    pub(in crate::engine::membership_transition) fn from_installed(
        directory: DiskRoot,
        expected_root: &str,
        transition: &str,
    ) -> Result<Self> {
        let store = ArchiveStore { root: directory };
        let root: ArchiveRoot = json_codec::from_value(
            AuthenticatedMetadataObject::authenticate(
                ROOT_KIND,
                expected_root,
                decode_strict(
                    &store
                        .root
                        .read_bytes(&store.path(ROOT_FILE), MAX_METADATA)?,
                )?,
            )?
            .into_value(),
        )?;
        let inspection = verify_root(&store, &root, transition)?;
        Ok(Self {
            store,
            root,
            expected_root: expected_root.to_owned(),
            inspection,
        })
    }

    pub(in crate::engine::membership_transition) fn abort_material(
        &self,
    ) -> Result<(
        abort_view::VerifiedAbortArchive,
        abort_view::PortableAbortArchive,
    )> {
        let expected = MembershipControlArchive {
            archive_path: self.store.root.path().to_owned(),
            root_hash: self.expected_root.clone(),
            transition_hash: self.root.transition_hash.clone(),
        };
        let verified = abort_view::VerifiedAbortArchive::read(
            self.store.root.private_child(self.store.root.path())?,
            &expected,
        )?;
        let portable = abort_view::PortableAbortArchive::read(
            self.store.root.private_child(self.store.root.path())?,
            &expected,
        )?;
        Ok((verified, portable))
    }

    pub(in crate::engine::membership_transition) fn identity(&self) -> Result<FileIdentity> {
        Ok(FileIdentity::from_fd(
            self.store.root.directory(self.store.root.path())?,
        )?)
    }

    pub(in crate::engine::membership_transition) fn verify_current_fence(
        &self,
        engine: &Engine,
    ) -> Result<()> {
        let named = engine.admit_control_archive_directory(self.store.root.path())?;
        if FileIdentity::from_fd(named.directory(named.path())?)? != self.identity()? {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "source archive moved after admission",
            ));
        }
        let current = engine.inspect_membership_metadata_archive()?;
        let archived = verify_root(&self.store, &self.root, &current.transition_hash)?;
        if archived != current || archived != self.inspection {
            return Err(Error::new(
                ErrorCode::LogFork,
                "archive differs from current original fence receipts",
            ));
        }
        Ok(())
    }

    /// Read this target's complete closure using only its held directory.
    pub(in crate::engine::membership_transition) fn verify_installed(
        &self,
        root: DiskRoot,
    ) -> Result<DiskRoot> {
        let store = ArchiveStore { root };
        let value = AuthenticatedMetadataObject::authenticate(
            ROOT_KIND,
            &self.expected_root,
            decode_strict(
                &store
                    .root
                    .read_bytes(&store.path(ROOT_FILE), MAX_METADATA)?,
            )?,
        )?;
        let saved: ArchiveRoot = json_codec::from_value(value.into_value())?;
        if verify_root(&store, &saved, &self.root.transition_hash)? != self.inspection {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "target control archive differs",
            ));
        }
        Ok(store.root)
    }

    pub(in crate::engine::membership_transition) fn install(
        &self,
        root: DiskRoot,
    ) -> Result<DiskRoot> {
        // Retain the archive advisory lock and every copy/sync/cleanup tail
        // through one admitted responsibility on an already-bound old root.
        let activity = root.retain_io_activity()?;
        let (destination, lock) =
            ArchiveStore::admit(root, &self.root.transition_hash, self.root.format)?;
        let copied = verify_root_copying(
            &self.store,
            &self.root,
            &self.root.transition_hash,
            Some(&destination),
            &mut |_| Ok(()),
            None,
        )?;
        // This second check has no source fallback. Missing target pages cannot
        // be satisfied by the original archive or another pending target.
        let installed = verify_root(&destination, &self.root, &self.root.transition_hash)?;
        if copied != self.inspection || installed != self.inspection {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "target control archive differs",
            ));
        }
        let prepared = PreparedMetadataObject::new(ROOT_KIND, &self.root, MAX_METADATA)?;
        if prepared.hash != self.expected_root {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "target archive root differs",
            ));
        }
        destination
            .root
            .write_atomic(&destination.path(ROOT_FILE), prepared.bytes(), false)?;
        drop(lock);
        if let Some(activity) = &activity {
            activity.check_running()?;
        }
        Ok(destination.root)
    }
}

impl ArchiveStore {
    fn copy_archived_member(
        &self,
        source: &Self,
        disk: &DiskConfig,
        member: &ArchivedMember,
        namespace: Option<&str>,
    ) -> Result<()> {
        let from = source.member_directory(disk, namespace);
        let to = self.member_directory(disk, namespace);
        for name in ["config.json", "disk.json"] {
            let bytes = source.root.read_bytes(&from.join(name), MAX_METADATA)?;
            self.root.write_atomic(&to.join(name), &bytes, false)?;
        }
        self.copy_journal(
            &to,
            source.root.open_read_regular(&from.join("journal.bin"))?,
            &member.journal,
        )
    }
}

/// Verify using only this directory and externally trusted digests. Neither a
/// self-declared root nor the binding file can supply authorization. No writes,
/// physical disk identity fabrication, replica fallback, or target admission.
pub fn verify_membership_control_archive(
    destination: &Path,
    expected_root: &str,
    expected_transition: &str,
) -> Result<MembershipArchiveInspection> {
    if !valid_hash(expected_root) || !valid_hash(expected_transition) {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "invalid expected archive binding",
        ));
    }
    let store = ArchiveStore {
        root: DiskRoot::open(destination)?,
    };
    let bytes = store
        .root
        .read_bytes(&store.path(ROOT_FILE), MAX_METADATA)?;
    let root = AuthenticatedMetadataObject::authenticate(
        ROOT_KIND,
        expected_root,
        decode_strict(&bytes)?,
    )?;
    let root: ArchiveRoot = json_codec::from_value(root.into_value())?;
    verify_root(&store, &root, expected_transition)
}

fn verify_root(
    store: &ArchiveStore,
    root: &ArchiveRoot,
    expected_transition: &str,
) -> Result<MembershipArchiveInspection> {
    verify_root_copying(
        store,
        root,
        expected_transition,
        None,
        &mut |_| Ok(()),
        None,
    )
}

fn verify_root_copying(
    store: &ArchiveStore,
    root: &ArchiveRoot,
    expected_transition: &str,
    destination: Option<&ArchiveStore>,
    observe: &mut dyn FnMut(usize) -> Result<()>,
    object_budget: Option<usize>,
) -> Result<MembershipArchiveInspection> {
    if root.transition_hash != expected_transition {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "archive transition differs from trusted binding",
        ));
    }
    let mut source = ArchivedSource {
        store,
        observe,
        object_budget,
        seals: BTreeMap::new(),
        destination,
        copied: BTreeSet::new(),
        namespace: None,
    };
    source.observe(0)?;
    let intent: TransitionIntent = json_codec::from_value(
        source
            .object(INTENT_KIND, expected_transition)?
            .into_value(),
    )?;
    let mut evidence = BTreeSet::new();
    if let Some(hash) = intent.origin.predecessor() {
        let predecessor = PortableAuthority::load(hash, &mut source)?;
        evidence.extend(predecessor.evidence_entries().keys().cloned());
        for (previous, previous_intent) in predecessor.archives()? {
            let hash = object_hash(ROOT_KIND, &previous)?;
            let stored = source.object(ROOT_KIND, &hash)?;
            let previous: ArchiveRoot = json_codec::from_value(stored.into_value())?;
            verify_epoch(&mut source, &previous, previous_intent)?;
        }
    }
    let value = AuthenticatedMetadataObject::authenticate(
        ROOT_KIND,
        &object_hash(ROOT_KIND, root)?,
        json_codec::to_value(root)?,
    )?;
    source.remember(&value)?;
    let inspected = verify_epoch(&mut source, root, intent)?;
    store.verify_evidence_names(&evidence)?;
    Ok(inspected)
}

fn verify_epoch(
    source: &mut ArchivedSource<'_>,
    root: &ArchiveRoot,
    intent: TransitionIntent,
) -> Result<MembershipArchiveInspection> {
    root.verify_scope(&intent)?;
    let epoch = FrozenEpoch::read(&intent, source)?;
    let config = epoch.config;
    Engine::require_actor(&intent.principal)?;
    validate_transition_target(config, &intent.target, intent.effect.as_ref())?;
    if intent.fence_sequence == u64::MAX
        || intent.fence_sequence <= intent.boundary.as_ref().map_or(0, |base| base.sequence)
    {
        return Err(Error::new(
            ErrorCode::LogFork,
            "invalid archived transition intent",
        ));
    }
    source.namespace = intent
        .origin
        .predecessor()
        .map(|_| root.transition_hash.clone());
    let members =
        crate::metadata_map::read_map(&root.members, |kind, hash| source.object(kind, hash))?;
    let selected = intent.fence_members();
    if members.len() != selected.len() {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "archive original member set differs",
        ));
    }
    let mut receipts = BTreeMap::new();
    let mut journals = Vec::new();
    for disk in &selected {
        let member: ArchivedMember = json_codec::from_value(
            members
                .get(&disk.disk_id)
                .ok_or_else(|| {
                    Error::new(ErrorCode::ConfigConflict, "archive original member missing")
                })?
                .clone(),
        )?;
        verify_member(
            source.store,
            &epoch,
            disk,
            &member,
            source.namespace.as_deref(),
            source.observe,
        )?;
        if let Some(destination) = source.destination {
            destination.copy_archived_member(
                source.store,
                disk,
                &member,
                source.namespace.as_deref(),
            )?;
        }
        receipts.insert(disk.disk_id.clone(), member.fence);
        journals.push(member.journal);
    }
    journals.sort_by(|left, right| left.disk_id.cmp(&right.disk_id));
    let seals = crate::metadata_map::read_map(&root.publication_seals, |kind, hash| {
        source.object(kind, hash)
    })?
    .into_iter()
    .map(|(key, value)| Ok((key, json_codec::from_value(value)?)))
    .collect::<Result<BTreeMap<String, String>>>()?;
    source.seals = seals;
    let config = config.clone();
    let fence = AdmittedFence {
        intent,
        hash: root.transition_hash.clone(),
        receipts,
        prepared_roots: BTreeMap::new(),
    };
    let inspected = inspect_control(&config, &fence, source)?;
    if inspected.journals != journals || inspected.published_candidates != source.seals.len() {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "archive journal bytes or publication seal set differs",
        ));
    }
    Ok(inspected)
}

fn verify_member(
    store: &ArchiveStore,
    epoch: &FrozenEpoch<'_>,
    disk: &DiskConfig,
    member: &ArchivedMember,
    namespace: Option<&str>,
    observe: &mut dyn FnMut(usize) -> Result<()>,
) -> Result<()> {
    let config = epoch.config;
    let directory = store.member_directory(disk, namespace);
    observe(0)?;
    let config_bytes = store.root.read_bytes_observed(
        &directory.join("config.json"),
        MAX_METADATA,
        &mut |bytes| observe(bytes),
    )?;
    let seal_bytes = store.root.read_bytes_observed(
        &directory.join("disk.json"),
        MAX_METADATA,
        &mut |bytes| observe(bytes),
    )?;
    if content_hash(&config_bytes) != member.config_bytes_hash
        || content_hash(&seal_bytes) != member.seal_bytes_hash
    {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "archive original identity bytes differ",
        ));
    }
    let saved: ValidatedConfig = decode_strict(&config_bytes)?;
    epoch.genesis.verify_saved_metadata_membership(&saved)?;
    let seal = decode_seal(&seal_bytes)?;
    verify_archived_member_identity(config, epoch.authority_hash.as_deref(), disk, member, &seal)
}

fn verify_archived_member_identity(
    config: &ValidatedConfig,
    generation: Option<&str>,
    disk: &DiskConfig,
    member: &ArchivedMember,
    seal: &DiskSeal,
) -> Result<()> {
    let expected_format = match generation {
        Some(hash) => DiskSealFormat::MembershipGeneration {
            authority_hash: hash.to_owned(),
        },
        None => DiskSealFormat::ManagedDirectory,
    };
    if seal.format != expected_format {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "archived seal belongs to another generation",
        ));
    }
    if seal.pool_id != config.pool_id
        || seal.disk_id != disk.disk_id
        || seal.slot_id != disk.slot_id
        || seal.binding_generation != disk.binding_generation
        || seal.simulation != (config.mode == Mode::Development)
        || member.journal.disk_id != disk.disk_id
        || member.journal.slot_id != disk.slot_id
        || member.journal.binding_generation != disk.binding_generation
    {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "archive original member identity differs",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/membership_abort_archive_tests.rs"]
mod abort_tests;
#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/membership_control_archive_tests.rs"]
mod tests;

/// The authenticated archive root is portable membership evidence. It contains
/// page references, not business State, paths to bodies, or copied file content.
impl PendingControlArchive {
    pub(in crate::engine::membership_transition) fn generation_root(&self) -> ArchiveRoot {
        self.root.clone()
    }
    pub(in crate::engine::membership_transition) fn install_generation_objects(
        &self,
        root: &DiskRoot,
        disk: &DiskConfig,
        fence: &AdmittedFence,
    ) -> Result<()> {
        for (epoch, intent) in archive_epochs(&self.store, &self.root, &fence.intent)? {
            let mut source = ActiveObjects {
                store: &self.store,
                root,
                disk,
                seals: archive_seals(&self.store, &epoch)?,
                namespace: intent
                    .origin
                    .predecessor()
                    .map(|_| epoch.transition_hash.clone()),
            };
            if disk.metadata_member {
                let members = crate::metadata_map::read_map(&epoch.members, |kind, hash| {
                    self.store.load(kind, hash)
                })?;
                let receipts = members
                    .into_iter()
                    .map(|(id, value)| {
                        let member: ArchivedMember = json_codec::from_value(value)?;
                        Ok((id, member.fence))
                    })
                    .collect::<Result<BTreeMap<_, _>>>()?;
                let original = intent.origin.config().clone();
                let archived_fence = AdmittedFence {
                    intent,
                    hash: epoch.transition_hash.clone(),
                    receipts,
                    prepared_roots: BTreeMap::new(),
                };
                inspect_control(&original, &archived_fence, &mut source)?;
            }
            for hash in source.seals.keys() {
                let (_, proof) = crate::metadata_map::read_proof(
                    &epoch.publication_seals,
                    hash,
                    |kind, hash| self.store.load(kind, hash),
                )?;
                root.write_json(
                    &disk
                        .root
                        .join("metadata/membership-history")
                        .join(format!("{hash}.json")),
                    &proof,
                    false,
                )?;
            }
        }
        Ok(())
    }
    pub(in crate::engine::membership_transition) fn generation_base(
        &self,
        intent: &TransitionIntent,
    ) -> Result<(State, BTreeMap<u64, String>)> {
        generation_base(&self.store, &self.root, intent)
    }
}
fn archive_seals(store: &ArchiveStore, root: &ArchiveRoot) -> Result<BTreeMap<String, String>> {
    crate::metadata_map::read_map(&root.publication_seals, |kind, hash| store.load(kind, hash))?
        .into_iter()
        .map(|(key, value)| Ok((key, json_codec::from_value(value)?)))
        .collect()
}
fn generation_base(
    store: &ArchiveStore,
    root: &ArchiveRoot,
    intent: &TransitionIntent,
) -> Result<(State, BTreeMap<u64, String>)> {
    let mut history = BTreeMap::new();
    for (epoch, _) in archive_epochs(store, root, intent)? {
        for hash in archive_seals(store, &epoch)?.keys() {
            let candidate: Candidate =
                json_codec::from_value(store.load("candidate", hash)?.into_value())?;
            if history.insert(candidate.sequence, hash.clone()).is_some() {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "archived generations repeat publication sequence",
                ));
            }
        }
    }
    let state = frozen_state_with(root, intent, |kind, hash| store.load(kind, hash))?;
    Ok((state, history))
}

pub(super) fn frozen_state_with(
    root: &ArchiveRoot,
    intent: &TransitionIntent,
    mut load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<State> {
    let seals =
        crate::metadata_map::read_map(&root.publication_seals, |kind, hash| load(kind, hash))?
            .into_iter()
            .map(|(key, value)| Ok((key, json_codec::from_value(value)?)))
            .collect::<Result<BTreeMap<String, String>>>()?;
    let state = match &intent.boundary {
        Some(boundary) => {
            let snapshot: Snapshot =
                json_codec::from_value(load("snapshot", &boundary.snapshot_hash)?.into_value())?;
            let mut state =
                resolve_admitted_snapshot(snapshot, |kind, hash| load(kind, hash))?.into_state();
            // The exact disk snapshot predates an explicit degraded outcome.
            // Reproduce ordinary recovery's visible-operation overlay without
            // relabelling that original snapshot or changing any stored bytes.
            let seal_hash = seals.get(&boundary.candidate_hash).ok_or_else(|| {
                Error::new(
                    ErrorCode::PublicationInvalid,
                    "frozen boundary lacks its exact publication seal",
                )
            })?;
            let seal: PublicationSeal =
                json_codec::from_value(load(SEAL_KIND, seal_hash)?.into_value())?;
            let candidate: Candidate =
                json_codec::from_value(load("candidate", &boundary.candidate_hash)?.into_value())?;
            let publication: Publication =
                json_codec::from_value(load("publication", &seal.publication_hash)?.into_value())?;
            verify_publication_seal(&candidate, &publication, &seal)?;
            if publication.recovery.is_some() {
                state
                    .operations
                    .insert(candidate.operation_id, publication.result);
            }
            state
        }
        None if intent.origin.predecessor().is_some() => {
            return Err(Error::new(
                ErrorCode::LogFork,
                "successor archive has no published boundary",
            ));
        }
        None => State {
            topology_generation: 1,
            ..State::default()
        },
    };
    Ok(state)
}

fn archive_epochs(
    store: &ArchiveStore,
    root: &ArchiveRoot,
    intent: &TransitionIntent,
) -> Result<Vec<(ArchiveRoot, TransitionIntent)>> {
    let mut epochs = match intent.origin.predecessor() {
        Some(hash) => PortableAuthority::load(hash, &mut &*store)?.archives()?,
        None => Vec::new(),
    };
    epochs.push((root.clone(), intent.clone()));
    Ok(epochs)
}
struct ActiveObjects<'a> {
    store: &'a ArchiveStore,
    root: &'a DiskRoot,
    disk: &'a DiskConfig,
    seals: BTreeMap<String, String>,
    namespace: Option<String>,
}
impl ControlSource for ActiveObjects<'_> {
    fn predecessor_evidence(&mut self, hash: &str) -> Result<AbortAuthorityEvidence> {
        // The installed generation marker carries these envelopes. Active
        // objects still resolve them only from this independently admitted F.
        self.store.evidence(hash, &mut |_| Ok(()))
    }
    fn observe(&mut self, _bytes: usize) -> Result<()> {
        Ok(())
    }

    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        let object = self.store.load(kind, hash)?;
        self.root
            .write_atomic(&object_path(self.disk, kind, hash), object.bytes(), false)?;
        Ok(object)
    }
    fn seal(
        &mut self,
        candidate: &Candidate,
        publication: &Publication,
    ) -> Result<PublicationSeal> {
        let hash = object_hash("candidate", candidate)?;
        let seal_hash = self.seals.get(&hash).ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "archive publication seal missing",
            )
        })?;
        let seal: PublicationSeal =
            json_codec::from_value(self.store.load(SEAL_KIND, seal_hash)?.into_value())?;
        verify_publication_seal(candidate, publication, &seal)?;
        self.root
            .write_json(&publication_seal_path(self.disk, &hash), &seal, false)?;
        Ok(seal)
    }
    fn journal(&self, disk: &DiskConfig) -> Result<ReadFile> {
        self.store.root.open_read_regular(
            &self
                .store
                .member_directory(disk, self.namespace.as_deref())
                .join("journal.bin"),
        )
    }
}
pub(in crate::engine::membership_transition) fn read_generation_base(
    root: DiskRoot,
    expected: &str,
    transition: &str,
    intent: &TransitionIntent,
) -> Result<(State, BTreeMap<u64, String>)> {
    let store = ArchiveStore { root };
    let root: ArchiveRoot = json_codec::from_value(
        AuthenticatedMetadataObject::authenticate(
            ROOT_KIND,
            expected,
            decode_strict(
                &store
                    .root
                    .read_bytes(&store.path(ROOT_FILE), MAX_METADATA)?,
            )?,
        )?
        .into_value(),
    )?;
    verify_root(&store, &root, transition)?;
    generation_base(&store, &root, intent)
}

/// Explicit activation recovery reinstalls only the exact objects proven by a
/// local immutable archive. It never interprets arbitrary root entries as data.
pub(in crate::engine::membership_transition) fn install_generation_archive(
    source: DiskRoot,
    root: &DiskRoot,
    disk: &DiskConfig,
    expected: &str,
    intent: &TransitionIntent,
) -> Result<()> {
    let store = ArchiveStore { root: source };
    let archived: ArchiveRoot = json_codec::from_value(
        AuthenticatedMetadataObject::authenticate(
            ROOT_KIND,
            expected,
            decode_strict(
                &store
                    .root
                    .read_bytes(&store.path(ROOT_FILE), MAX_METADATA)?,
            )?,
        )?
        .into_value(),
    )?;
    let transition = object_hash(INTENT_KIND, intent)?;
    let inspection = verify_root(&store, &archived, &transition)?;
    let members =
        crate::metadata_map::read_map(&archived.members, |kind, hash| store.load(kind, hash))?;
    let receipts = members
        .into_iter()
        .map(|(id, value)| {
            let member: ArchivedMember = json_codec::from_value(value)?;
            Ok((id, member.fence))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let fence = AdmittedFence {
        intent: intent.clone(),
        hash: transition,
        receipts,
        prepared_roots: BTreeMap::new(),
    };
    let archive = PendingControlArchive {
        store,
        root: archived,
        expected_root: expected.to_owned(),
        inspection,
    };
    archive.install_generation_objects(root, disk, &fence)
}

/// Scrub the original independently recoverable closure through the caller's
/// cancellation/rate/budget observer, without source fallback or repair.
pub(in crate::engine) fn verify_generation_archive_observed(
    root: DiskRoot,
    expected: &str,
    transition: &str,
    observe: &mut impl FnMut(usize) -> Result<()>,
    maximum_objects: usize,
) -> Result<MembershipArchiveInspection> {
    let store = ArchiveStore { root };
    observe(0)?;
    let bytes = store
        .root
        .read_bytes_observed(&store.path(ROOT_FILE), MAX_METADATA, observe)?;
    let root: ArchiveRoot = json_codec::from_value(
        AuthenticatedMetadataObject::authenticate(ROOT_KIND, expected, decode_strict(&bytes)?)?
            .into_value(),
    )?;
    verify_root_copying(
        &store,
        &root,
        transition,
        None,
        observe,
        Some(maximum_objects),
    )
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/membership_control_probe_tests.rs"]
mod control_probe_tests;

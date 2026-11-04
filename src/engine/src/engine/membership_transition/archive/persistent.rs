//! Independent control metadata archive. The root is not a transition decision,
//! target readiness, complete evacuation archive, or a grant of write authority.
use super::*;
use crate::metadata_map::{Encoder, ProofMeasure, Reference};
use platform_io::fs_safety::linux::FileIdentity;
use platform_io::fs_safety::{AdvisoryLock, RelativePath};
use std::path::{Path, PathBuf};

const ROOT_KIND: &str = "membership-control-archive";
pub(super) const SEAL_KIND: &str = "membership-publication-seal";
const ROOT_FILE: &str = "archive-root.json";
const BINDING_FILE: &str = "archive-binding.json";
const LOCK_FILE: &str = ".archive.lock";

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "frozen-control-metadata")]
    FrozenControlMetadata,
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
    pub root_hash: String,
    pub transition_hash: String,
}

pub(super) struct ArchiveStore {
    root: DiskRoot,
}
impl ArchiveStore {
    /// Destination must already be a private, empty directory or this exact
    /// in-progress binding. No chmod, mounting, overwrites or foreign cleanup.
    fn admit(root: DiskRoot, transition: &str) -> Result<(Self, AdvisoryLock)> {
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
            if binding.transition_hash != transition {
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
                        .open_regular(&store.root.path().join(name.as_path()))?;
                    continue;
                }
                if !matches!(
                    name.as_os_str().to_str(),
                    Some(BINDING_FILE | LOCK_FILE | ROOT_FILE | "objects" | "journals")
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
                format: Format::FrozenControlMetadata,
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
    fn member_directory(&self, disk: &DiskConfig) -> PathBuf {
        self.path("journals")
            .join(&disk.disk_id)
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
        let directory = self.member_directory(disk);
        let config = handle.root.read_bytes(
            &disk.root.join("metadata/pool-membership/config.json"),
            MAX_METADATA,
        )?;
        let saved: ValidatedConfig = decode_strict(&config)?;
        engine.config.verify_saved_metadata_membership(&saved)?;
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
            handle.root.open_regular(&log_path(disk))?,
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
        mut input: File,
        expected: &MembershipJournalInspection,
    ) -> Result<()> {
        let path = directory.join("journal.bin");
        let directory = self.root.private_directory(directory)?;
        match self.root.open_optional_regular(&path)? {
            Some(file) => {
                let (bytes, hash) = hash_file(&file, expected.bytes)?;
                if bytes != expected.bytes || hash != expected.content_hash {
                    return Err(Error::new(
                        ErrorCode::ObjectCollision,
                        "existing archived journal differs",
                    ));
                }
                file.sync_all()?;
                directory.sync().map_err(fs_error)
            }
            None => {
                let digest = blake3::Hash::from_hex(&expected.content_hash)
                    .expect("invariant: journal inspection produced BLAKE3 hex");
                platform_io::payload::write_full(
                    &directory,
                    &platform_io::fs_safety::EntryName::new("journal.bin").map_err(fs_error)?,
                    &mut input,
                    platform_io::payload::Expectation {
                        bytes: expected.bytes,
                        blake3: Some(digest),
                    },
                )?;
                Ok(())
            }
        }
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
fn hash_file(file: &File, expected_bytes: u64) -> Result<(u64, String)> {
    let limit = expected_bytes.checked_add(1).ok_or_else(|| {
        Error::new(
            ErrorCode::ResourceLimit,
            "archive journal byte limit overflow",
        )
    })?;
    let mut digest = blake3::Hasher::new();
    let mut reader = HashingReader {
        reader: file.take(limit),
        digest: &mut digest,
    };
    let bytes = std::io::copy(&mut reader, &mut std::io::sink())?;
    Ok((bytes, digest.finalize().to_hex().to_string()))
}

struct ArchivedSource<'a> {
    store: &'a ArchiveStore,
    observe: &'a mut dyn FnMut(usize) -> Result<()>,
    object_budget: Option<usize>,
    seals: BTreeMap<String, String>,
    destination: Option<&'a ArchiveStore>,
    copied: BTreeSet<(&'static str, String)>,
}
impl ControlSource for ArchivedSource<'_> {
    fn observe(&mut self, bytes: usize) -> Result<()> {
        (self.observe)(bytes)
    }
    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        (self.observe)(0)?;
        let object = self.store.load_observed(kind, hash, self.observe)?;
        let address = (kind, hash.to_owned());
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
                destination.object_bytes(kind, hash, object.bytes())?;
            }
            if self.destination.is_some() || self.object_budget.is_some() {
                self.copied.insert(address);
            }
        }
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
    fn journal(&self, disk: &DiskConfig) -> Result<File> {
        self.store
            .root
            .open_regular(&self.store.member_directory(disk).join("journal.bin"))
    }
}

impl Engine {
    /// A rename after Engine admission must not defeat the lexical exclusion
    /// above. Reopen only original names to compare with held identities; never
    /// use those reopened handles as sources. Target paths are not admitted here.
    fn admit_control_archive_directory(&self, destination: &Path) -> Result<DiskRoot> {
        let destination = DiskRoot::open(destination)?;
        let destination_identity =
            FileIdentity::from_fd(destination.directory(destination.path())?)?;
        for handle in self.disk_resources.handles() {
            let named = DiskRoot::open(handle.root.path())?;
            let named_identity = FileIdentity::from_fd(named.directory(named.path())?)?;
            let held_identity = FileIdentity::from_fd(handle.root.directory(handle.root.path())?)?;
            if named_identity != held_identity || destination_identity == held_identity {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "original directory moved or aliases archive destination",
                ));
            }
        }
        let system = DiskRoot::open(&self.root)?;
        let named_lock = system.open_regular(&self.root.join("writer.lock"))?;
        let held_lock = self
            ._locks
            .first()
            .expect("invariant: Engine holds its system writer lock");
        if FileIdentity::from_fd(named_lock)? != FileIdentity::from_fd(held_lock)?
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
        for path in self
            .config
            .disks
            .iter()
            .chain(fence.intent.target.disks.iter())
            .map(|disk| &disk.root)
            .chain(std::iter::once(&self.root))
        {
            let path = absolute_path(path)?;
            if destination.starts_with(&path) || path.starts_with(&destination) {
                return Err(Error::new(
                    ErrorCode::UnsafePath,
                    "control archive must be separate from original, target and system directories",
                ));
            }
        }
        let inspected = self.inspect_membership_metadata_archive()?;
        let destination = self.admit_control_archive_directory(&destination)?;
        let (store, _lock) = ArchiveStore::admit(destination, &inspected.transition_hash)?;
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
            members: self.members(),
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
            format: Format::FrozenControlMetadata,
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
        for root in engine
            .config
            .disks
            .iter()
            .chain(fence.intent.target.disks.iter())
            .map(|disk| &disk.root)
            .chain(std::iter::once(&engine.root))
        {
            if path.starts_with(root) || root.starts_with(&path) {
                return Err(Error::new(
                    ErrorCode::UnsafePath,
                    "source archive overlaps a pool directory",
                ));
            }
        }
        let store = ArchiveStore {
            root: engine.admit_control_archive_directory(&path)?,
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
        archive.verify_current_fence(engine)?;
        Ok(archive)
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
        let (destination, _lock) = ArchiveStore::admit(root, &self.root.transition_hash)?;
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
        Ok(destination.root)
    }
}

impl ArchiveStore {
    fn copy_archived_member(
        &self,
        source: &Self,
        disk: &DiskConfig,
        member: &ArchivedMember,
    ) -> Result<()> {
        let from = source.member_directory(disk);
        let to = self.member_directory(disk);
        for name in ["config.json", "disk.json"] {
            let bytes = source.root.read_bytes(&from.join(name), MAX_METADATA)?;
            self.root.write_atomic(&to.join(name), &bytes, false)?;
        }
        self.copy_journal(
            &to,
            source.root.open_regular(&from.join("journal.bin"))?,
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
    observe(0)?;
    let intent_object = store.load_observed(INTENT_KIND, expected_transition, observe)?;
    let intent: TransitionIntent = json_codec::from_value(intent_object.into_value())?;
    let Origin::Genesis { config } = &intent.origin;
    Engine::require_actor(&intent.principal)?;
    if config.pool_id != intent.target.pool_id
        || config.same_metadata_membership(&intent.target)
        || intent.fence_sequence == u64::MAX
        || intent.fence_sequence <= intent.boundary.as_ref().map_or(0, |base| base.sequence)
    {
        return Err(Error::new(
            ErrorCode::LogFork,
            "invalid archived transition intent",
        ));
    }
    let mut source = ArchivedSource {
        store,
        observe,
        object_budget,
        seals: BTreeMap::new(),
        destination,
        copied: BTreeSet::new(),
    };
    let members =
        crate::metadata_map::read_map(&root.members, |kind, hash| source.object(kind, hash))?;
    if members.len() != config.metadata_members().len() {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "archive original member set differs",
        ));
    }
    let mut receipts = BTreeMap::new();
    let mut journals = Vec::new();
    for disk in config.metadata_members() {
        let member: ArchivedMember = json_codec::from_value(
            members
                .get(&disk.disk_id)
                .ok_or_else(|| {
                    Error::new(ErrorCode::ConfigConflict, "archive original member missing")
                })?
                .clone(),
        )?;
        verify_member(store, config, disk, &member, source.observe)?;
        if let Some(destination) = destination {
            destination.copy_archived_member(store, disk, &member)?;
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
        hash: expected_transition.to_owned(),
        receipts,
    };
    let inspected = inspect_control(&config, &fence, &mut source)?;
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
    config: &ValidatedConfig,
    disk: &DiskConfig,
    member: &ArchivedMember,
    observe: &mut dyn FnMut(usize) -> Result<()>,
) -> Result<()> {
    let directory = store.member_directory(disk);
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
    config.verify_saved_metadata_membership(&saved)?;
    let seal = decode_seal(&seal_bytes)?;
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
        let mut source = ActiveObjects {
            store: &self.store,
            root,
            disk,
            seals: archive_seals(&self.store, &self.root)?,
        };
        if disk.metadata_member {
            inspect_control(
                &match &fence.intent.origin {
                    Origin::Genesis { config } => config.clone(),
                },
                fence,
                &mut source,
            )?;
        }
        for hash in source.seals.keys() {
            let (_, proof) = crate::metadata_map::read_proof(
                &self.root.publication_seals,
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
    let seals = archive_seals(store, root)?;
    let mut history = BTreeMap::new();
    for hash in seals.keys() {
        let candidate: Candidate =
            json_codec::from_value(store.load("candidate", hash)?.into_value())?;
        if history.insert(candidate.sequence, hash.clone()).is_some() {
            return Err(Error::new(
                ErrorCode::LogFork,
                "archived generation repeats publication sequence",
            ));
        }
    }
    let state = match &intent.boundary {
        Some(boundary) => {
            let snapshot: Snapshot = json_codec::from_value(
                store
                    .load("snapshot", &boundary.snapshot_hash)?
                    .into_value(),
            )?;
            let mut state =
                resolve_admitted_snapshot(snapshot, |kind, hash| store.load(kind, hash))?
                    .into_state();
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
                json_codec::from_value(store.load(SEAL_KIND, seal_hash)?.into_value())?;
            let candidate: Candidate = json_codec::from_value(
                store
                    .load("candidate", &boundary.candidate_hash)?
                    .into_value(),
            )?;
            let publication: Publication = json_codec::from_value(
                store
                    .load("publication", &seal.publication_hash)?
                    .into_value(),
            )?;
            verify_publication_seal(&candidate, &publication, &seal)?;
            if publication.recovery.is_some() {
                state
                    .operations
                    .insert(candidate.operation_id, publication.result);
            }
            state
        }
        None => State {
            topology_generation: 1,
            ..State::default()
        },
    };
    Ok((state, history))
}
struct ActiveObjects<'a> {
    store: &'a ArchiveStore,
    root: &'a DiskRoot,
    disk: &'a DiskConfig,
    seals: BTreeMap<String, String>,
}
impl ControlSource for ActiveObjects<'_> {
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
    fn journal(&self, disk: &DiskConfig) -> Result<File> {
        self.store
            .root
            .open_regular(&self.store.member_directory(disk).join("journal.bin"))
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

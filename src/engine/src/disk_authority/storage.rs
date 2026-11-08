use super::{
    codec,
    model::Object,
    verify::{self, ReadStore},
    *,
};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{
    AdvisoryLock, AtomicFile, EntryName, InventoryLimits, PayloadOpenMode, PrivateDirectory,
    RelativePath,
};
use std::{
    collections::BTreeMap,
    io::{Read, Seek},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Header {
    format: Format,
    identity: Identity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct LocalBinding {
    path: PathBuf,
    device: u64,
    inode: u64,
    mount_root: location::MountRoot,
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
struct Initialization {
    format: Format,
    identity: Identity,
    target: LocalBinding,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct LocalExecution {
    format: Format,
    review: LocalReview,
    intent: Digest,
}

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct LocalReview {
    target: LocalBinding,
    sources: Vec<LocalSource>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct LocalSource {
    identity: Digest,
    checkpoint: Digest,
    binding: LocalBinding,
}

/// Read-only observations, never accepted by the writer as a capability.
#[derive(Debug)]
pub enum OperationEvidence {
    Incomplete {
        operation_id: Token,
        local_target_matches: Option<bool>,
    },
    Intent {
        intent: PublicationIntent,
        local_target_matches: bool,
        base_matches: bool,
    },
    Prepared {
        intent: PublicationIntent,
        prepared: PreparedEvidence,
        local_target_matches: bool,
        base_matches: bool,
        missing_required_bodies: Vec<Digest>,
    },
    Published(DiskPublicationReceipt),
}

pub struct Disk {
    root: PrivateDirectory,
    objects: PrivateDirectory,
    bodies: PrivateDirectory,
    operations: PrivateDirectory,
    publications: PrivateDirectory,
    identity: Identity,
    binding: LocalBinding,
    known_checkpoint: Option<Digest>,
}

/// One fresh metadata observation and the held directory that produced it.
/// Only Disk constructs this pair; callers cannot splice in another location.
pub(super) struct CapturedSourceCandidate {
    location: location::Location,
    observation: SourceCandidateObservation,
}
impl CapturedSourceCandidate {
    pub(super) fn into_parts(self) -> (location::Location, SourceCandidateObservation) {
        (self.location, self.observation)
    }
}

impl Disk {
    /// Initialize only an empty new-format directory, or resume this exact
    /// physical initialization. The completion record is distinct from identity.
    pub fn initialize(path: &Path, identity: Identity) -> Result<Self> {
        identity.validate()?;
        let root = PrivateDirectory::create(path)?;
        let held = platform_io::fs_safety::linux::OpenAt2Root::from_private_directory(
            root,
            platform_io::fs_safety::linux::MountPolicy::SameMount,
        )?;
        // Share the directory-before-writer ordering with legacy initialization.
        // No format-specific sidecar may be created until this lock is held.
        let _initialization =
            platform_io::fs_safety::linux::AdvisoryLock::directory(held.directory())?;
        let root = PrivateDirectory::from_directory(held.into_directory(), path.to_owned())?;
        root.verify_identity()?;
        let header = Header {
            format: Format::Independent,
            identity: identity.clone(),
        };
        let header_bytes = codec::encode(&header)?;
        if read_optional(&root, &name("initialized.json")?)?.is_some() {
            return Self::open(path, &identity);
        }
        let selected = location::Location::open(root.path())?;
        let metadata = std::fs::symlink_metadata(root.path())?;
        let intent = Initialization {
            format: Format::Independent,
            identity: identity.clone(),
            target: LocalBinding {
                path: root.path().to_owned(),
                device: metadata.dev(),
                inode: metadata.ino(),
                mount_root: selected.mount_root(),
            },
        };
        root.verify_identity()?;
        let prior = read_optional(&root, &name("initialization.json")?)?;
        if let Some(bytes) = prior {
            if codec::decode::<Initialization>(&bytes)? != intent {
                return Err(Error::Identity);
            }
        } else if std::fs::read_dir(path)?.next().transpose()?.is_some() {
            return Err(Error::Invalid("new independent-format disk must be empty"));
        }
        // No lock sidecar precedes the durable, exact first ownership record.
        save_exact(
            &root,
            &name("initialization.json")?,
            &codec::encode(&intent)?,
        )?;
        fault::after(fault::Boundary::InitializationIntent)?;
        let lock = AdvisoryLock::acquire(&root, &RelativePath::new("writer.lock")?)?;
        save_exact(
            &root,
            &name("initialization.json")?,
            &codec::encode(&intent)?,
        )?;
        save_exact(&root, &name("identity.json")?, &header_bytes)?;
        fault::after(fault::Boundary::InitializationIdentity)?;
        for child in ["objects", "bodies", "operations", "publications"] {
            root.create_child(&name(child)?)?;
        }
        fault::after(fault::Boundary::InitializationDirectories)?;
        save_exact(
            &root,
            &name("initialized.json")?,
            &codec::encode(&codec::address("initialized", &header_bytes))?,
        )?;
        fault::after(fault::Boundary::InitializationPublished)?;
        drop(lock);
        root.verify_identity()?;
        let disk = Self::open(path, &identity)?;
        if disk.binding != intent.target {
            return Err(Error::Identity);
        }
        Ok(disk)
    }
    pub fn open(path: &Path, expected: &Identity) -> Result<Self> {
        let root = PrivateDirectory::open_existing(path)?;
        let header: Header =
            codec::decode(&root.read_bounded(&name("identity.json")?, MAX_BYTES)?)?;
        let initialized: Digest =
            codec::decode(&root.read_bounded(&name("initialized.json")?, MAX_BYTES)?)?;
        if initialized != codec::address("initialized", &codec::encode(&header)?) {
            return Err(Error::Invalid(
                "initialization is incomplete or differs from the identity",
            ));
        }
        if &header.identity != expected {
            return Err(Error::Identity);
        }
        header.identity.validate()?;
        let selected = location::Location::open(root.path())?;
        let metadata = std::fs::symlink_metadata(root.path())?;
        let binding = LocalBinding {
            path: root.path().to_owned(),
            device: metadata.dev(),
            inode: metadata.ino(),
            mount_root: selected.mount_root(),
        };
        root.verify_identity()?;
        Ok(Self {
            objects: PrivateDirectory::open_existing(path.join("objects"))?,
            bodies: PrivateDirectory::open_existing(path.join("bodies"))?,
            operations: PrivateDirectory::open_existing(path.join("operations"))?,
            publications: PrivateDirectory::open_existing(path.join("publications"))?,
            identity: header.identity,
            root,
            binding,
            known_checkpoint: None,
        })
    }
    /// Construct read resources from the caller's admitted descriptor. The
    /// diagnostic path is checked, never used to substitute a new root/child.
    /// Metadata evidence still comes only from source_topology/observation.
    pub(super) fn from_held_directory(root: PrivateDirectory) -> Result<Self> {
        use platform_io::fs_safety::linux::{MountPolicy, OpenAt2Root};
        root.verify_identity()?;
        let path = root.path().to_owned();
        let selected = location::Location::from_root(OpenAt2Root::from_private_directory(
            root,
            MountPolicy::SameMount,
        )?)?;
        let root = selected.private_directory(&path)?;
        let header: Header =
            codec::decode(&root.read_bounded(&name("identity.json")?, MAX_BYTES)?)?;
        let initialized: Digest =
            codec::decode(&root.read_bounded(&name("initialized.json")?, MAX_BYTES)?)?;
        if initialized != codec::address("initialized", &codec::encode(&header)?) {
            return Err(Error::Invalid(
                "initialization is incomplete or differs from the identity",
            ));
        }
        header.identity.validate()?;
        let physical = selected.identity();
        let disk = Self {
            objects: selected.private_child(&name("objects")?, &path)?,
            bodies: selected.private_child(&name("bodies")?, &path)?,
            operations: selected.private_child(&name("operations")?, &path)?,
            publications: selected.private_child(&name("publications")?, &path)?,
            identity: header.identity,
            root,
            binding: LocalBinding {
                path,
                device: physical.device,
                inode: physical.inode,
                mount_root: selected.mount_root(),
            },
            known_checkpoint: None,
        };
        disk.check_identity()?;
        Ok(disk)
    }
    pub(super) fn pinned(mut self, checkpoint: Option<Digest>) -> Result<Self> {
        self.known_checkpoint = checkpoint;
        self.check_metadata(None)?;
        Ok(self)
    }
    pub(super) fn location(&self) -> Result<location::Location> {
        self.check_identity()?;
        let selected = location::Location::open(self.root.path())?;
        let identity = selected.identity();
        if (identity.device, identity.inode, selected.mount_root())
            != (
                self.binding.device,
                self.binding.inode,
                self.binding.mount_root,
            )
        {
            return Err(Error::Identity);
        }
        Ok(selected)
    }
    pub(super) fn path(&self) -> &Path {
        self.root.path()
    }
    pub(super) fn selected_source(&self, verified: &VerifiedDisk) -> Result<LocalSource> {
        self.check_identity()?;
        if verified.identity != self.identity
            || verified.local_binding.as_ref() != Some(&self.binding)
        {
            return Err(Error::Identity);
        }
        Ok(LocalSource {
            identity: self.identity.digest()?,
            checkpoint: verified.source_checkpoint()?.publication,
            binding: self.binding.clone(),
        })
    }
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    pub fn verify(&self, previously_observed: Option<Digest>) -> Result<VerifiedDisk> {
        self.verify_using(self, previously_observed)
    }
    pub(super) fn check_metadata(&self, previously_observed: Option<Digest>) -> Result<()> {
        self.verify_using(&MetadataOnly { disk: self }, previously_observed)
            .map(drop)
    }
    pub(super) fn source_topology(&self) -> Result<SourceTopology> {
        let verified = self.verify_using(&MetadataOnly { disk: self }, None)?;
        let topology = topology::project(&verified, self.binding.clone())?;
        self.check_identity()?;
        Ok(topology)
    }
    pub(super) fn source_inventory(&self) -> Result<SourceInventory> {
        let verified = self.verify_using(&MetadataOnly { disk: self }, None)?;
        let topology = topology::project(&verified, self.binding.clone())?;
        let inventory = SourceInventory::capture(&verified, topology);
        self.check_identity()?;
        Ok(inventory)
    }
    pub(super) fn content_snapshot(&self) -> Result<ContentSnapshot<'_>> {
        let verified = self.verify_using(&MetadataOnly { disk: self }, None)?;
        self.check_identity()?;
        Ok(ContentSnapshot::capture(self, verified))
    }
    pub(super) fn observe_source_node(
        &self,
        node: &SourceNode,
    ) -> Result<SourceCandidateObservation> {
        let verified = self.verify_using(&MetadataOnly { disk: self }, None)?;
        let observation = topology::observe_candidate(node, &verified, self.binding.clone())?;
        self.check_identity()?;
        Ok(observation)
    }
    pub(super) fn capture_source_candidate(
        &self,
        node: &SourceNode,
    ) -> Result<CapturedSourceCandidate> {
        let location = self.location()?;
        let observation = self.observe_source_node(node)?;
        Ok(CapturedSourceCandidate {
            location,
            observation,
        })
    }
    pub(super) fn read_content(&self, copy: Digest) -> Result<Vec<u8>> {
        self.content_snapshot()?.read_content(copy)
    }
    fn verify_using(
        &self,
        store: &impl ReadStore,
        previously_observed: Option<Digest>,
    ) -> Result<VerifiedDisk> {
        self.check_identity()?;
        let decisions = self.decisions()?;
        let Some((_, latest)) = decisions.last_key_value() else {
            if previously_observed.is_some() || self.known_checkpoint.is_some() {
                return Err(Error::StaleBase);
            }
            return Ok(VerifiedDisk {
                identity: self.identity.clone(),
                local_binding: Some(self.binding.clone()),
                checkpoint: None,
                sequence: 0,
                rows: vec![],
                bundle: Bundle::default(),
                publications: BTreeMap::new(),
            });
        };
        let mut verified = verify::verify(store, &self.identity, *latest)?;
        verified.local_binding = Some(self.binding.clone());
        let own = verified
            .publications
            .iter()
            .filter(|(_, p)| p.identity == self.identity)
            .map(|(hash, p)| (p.sequence, *hash))
            .collect::<BTreeMap<_, _>>();
        if own != decisions {
            return Err(Error::Invalid(
                "durable publication files do not equal the local proof chain",
            ));
        }
        if [self.known_checkpoint, previously_observed]
            .into_iter()
            .flatten()
            .any(|pin| !own.values().any(|hash| *hash == pin))
        {
            return Err(Error::StaleBase);
        }
        Ok(verified)
    }
    pub fn operation_evidence(&self, id: &Token) -> Result<Option<OperationEvidence>> {
        let current = self.verify(None)?;
        if let Some(receipt) = current.receipt(id) {
            return Ok(Some(OperationEvidence::Published(receipt)));
        }
        let operation =
            match PrivateDirectory::open_existing(self.operations.path().join(id.as_str())) {
                Ok(operation) => operation,
                Err(platform_io::fs_safety::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            };
        let Some(bytes) = read_optional(&operation, &name("intent.json")?)? else {
            let local_target_matches = read_optional(&operation, &name("local.json")?)?
                .map(|bytes| {
                    codec::decode::<LocalExecution>(&bytes)
                        .map(|local| local.review.target == self.binding)
                })
                .transpose()?;
            return Ok(Some(OperationEvidence::Incomplete {
                operation_id: id.clone(),
                local_target_matches,
            }));
        };
        let Object::Intent(intent) = codec::decode(&bytes)? else {
            return Err(Error::Invalid("expected intent evidence"));
        };
        if intent.target != self.identity || &intent.operation_id != id {
            return Err(Error::Identity);
        }
        if intent.request_digest != verify::request_digest(&intent)? {
            return Err(Error::Invalid("request digest mismatch"));
        }
        let intent_hash = codec::address("object", &bytes);
        let local: LocalExecution =
            codec::decode(&operation.read_bounded(&name("local.json")?, MAX_BYTES)?)?;
        if local.intent != intent_hash
            || codec::address("execution", &codec::encode(&local.review)?)
                != intent.execution_binding
        {
            return Err(Error::OperationConflict);
        }
        let local_target_matches = local.review.target == self.binding;
        let base_matches = current.checkpoint == intent.base;
        let Some(bytes) = read_optional(&operation, &name("prepared.json")?)? else {
            return Ok(Some(OperationEvidence::Intent {
                intent,
                local_target_matches,
                base_matches,
            }));
        };
        let Object::Prepared(prepared) = codec::decode(&bytes)? else {
            return Err(Error::Invalid("expected prepared evidence"));
        };
        let prepared_hash = codec::address("object", &bytes);
        let Object::Manifest(manifest) = codec::decode(&self.object(intent.manifest)?)? else {
            return Err(Error::Invalid("expected manifest"));
        };
        let publication = Publication {
            format: Format::Independent,
            identity: self.identity.clone(),
            sequence: manifest.sequence,
            previous: intent.base,
            operation_id: id.clone(),
            request_digest: intent.request_digest,
            intent: intent_hash,
            prepared: prepared_hash,
            manifest: intent.manifest,
        };
        let root = Object::Publication(publication.clone()).digest()?;
        verify::verify(
            &CandidateReader {
                disk: self,
                publication: &publication,
            },
            &self.identity,
            root,
        )?;
        let mut missing_required_bodies = Vec::new();
        for hash in prepared.required_bodies.keys() {
            if self.body(*hash)?.is_none() {
                missing_required_bodies.push(*hash);
            }
        }
        Ok(Some(OperationEvidence::Prepared {
            intent,
            prepared,
            local_target_matches,
            base_matches,
            missing_required_bodies,
        }))
    }
    pub fn writer(&self) -> Result<WriteSession<'_>> {
        self.check_identity()?;
        let lock = AdvisoryLock::acquire(&self.root, &RelativePath::new("writer.lock")?)?;
        Ok(WriteSession {
            disk: self,
            _lock: lock,
        })
    }
    pub(super) fn check_identity(&self) -> Result<()> {
        let selected = location::Location::open(self.root.path())?;
        let current = selected.identity();
        if (current.device, current.inode, selected.mount_root())
            != (
                self.binding.device,
                self.binding.inode,
                self.binding.mount_root,
            )
        {
            return Err(Error::Identity);
        }
        for directory in [
            &self.root,
            &self.objects,
            &self.bodies,
            &self.operations,
            &self.publications,
        ] {
            // This check compares an admitted directory's held inode with its
            // current name. The platform reports a mismatch as UnsafeFileType;
            // here it is a changed selection, not a device I/O disconnect.
            directory.verify_identity().map_err(|source| match source {
                platform_io::fs_safety::Error::UnsafeFileType(path) => {
                    platform_io::fs_safety::Error::IdentityChanged(path)
                }
                other => other,
            })?;
        }
        let header: Header =
            codec::decode(&self.root.read_bounded(&name("identity.json")?, MAX_BYTES)?)?;
        if header.identity != self.identity {
            return Err(Error::Identity);
        }
        let initialized: Digest = codec::decode(
            &self
                .root
                .read_bounded(&name("initialized.json")?, MAX_BYTES)?,
        )?;
        if initialized != codec::address("initialized", &codec::encode(&header)?) {
            return Err(Error::Invalid("initialization completion differs"));
        }
        Ok(())
    }
    fn decisions(&self) -> Result<BTreeMap<u64, Digest>> {
        let mut found = BTreeMap::new();
        for entry in self.publications.files(InventoryLimits {
            max_entries: MAX_PUBLICATIONS * 2,
            max_total_bytes: MAX_BYTES as u64,
        })? {
            if AtomicFile::is_temporary_name(&entry.name) {
                continue;
            }
            let text = entry
                .name
                .as_os_str()
                .to_str()
                .ok_or(Error::Invalid("non-UTF8 publication name"))?;
            let Some(number) = text.strip_suffix(".json") else {
                return Err(Error::Invalid("unknown publication file"));
            };
            let sequence: u64 = number
                .parse()
                .map_err(|_| Error::Invalid("invalid publication sequence name"))?;
            if text != format!("{sequence:020}.json") || sequence == 0 {
                return Err(Error::Invalid("noncanonical publication name"));
            }
            let raw = self.publications.read_bounded(&entry.name, MAX_BYTES)?;
            let Object::Publication(publication) = codec::decode(&raw)? else {
                return Err(Error::Invalid("decision is not a publication"));
            };
            if publication.sequence != sequence || publication.identity != self.identity {
                return Err(Error::Identity);
            }
            let hash = codec::address("object", &raw);
            found.insert(sequence, hash);
        }
        Ok(found)
    }
}
/// Complete metadata capture without a content availability claim. It is
/// never passed to publication or backup writers as a full-copy observation.
struct MetadataOnly<'a> {
    disk: &'a Disk,
}
impl ReadStore for MetadataOnly<'_> {
    fn object(&self, hash: Digest) -> Result<Vec<u8>> {
        self.disk.object(hash)
    }
    fn body(&self, _copy: Digest) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

impl ReadStore for Disk {
    fn object(&self, hash: Digest) -> Result<Vec<u8>> {
        if let Some(bytes) = read_optional(&self.objects, &name(format!("{hash}.json"))?)? {
            return Ok(bytes);
        }
        // Own publications are NEVER staged as ordinary objects. Their bytes
        // become readable only through a real durable decision name.
        for entry in self.publications.files(InventoryLimits {
            max_entries: MAX_PUBLICATIONS * 2,
            max_total_bytes: MAX_BYTES as u64,
        })? {
            if AtomicFile::is_temporary_name(&entry.name) {
                continue;
            }
            let raw = self.publications.read_bounded(&entry.name, MAX_BYTES)?;
            if codec::address("object", &raw) == hash {
                return Ok(raw);
            }
        }
        Err(Error::Missing(hash))
    }
    fn body(&self, hash: Digest) -> Result<Option<Vec<u8>>> {
        read_optional(&self.bodies, &name(format!("{hash}.bin"))?)
    }
}

/// Exact material reviewed for one disk's next authority publication. Building
/// it does no I/O and authorizes no write. It is not a product import plan.
pub struct PublicationRequest {
    local_binding: LocalBinding,
    review: LocalReview,
    intent: PublicationIntent,
    publication: Publication,
    material: Bundle,
}
impl PublicationRequest {
    pub fn new(
        before: &VerifiedDisk,
        operation_id: Token,
        authority: Authority,
        material: Bundle,
    ) -> Result<Self> {
        Self::selected(before, operation_id, authority, material, vec![])
    }
    pub(super) fn selected(
        before: &VerifiedDisk,
        operation_id: Token,
        authority: Authority,
        material: Bundle,
        sources: Vec<LocalSource>,
    ) -> Result<Self> {
        if match &authority {
            Authority::Primary { heads } => heads.len(),
            Authority::Secondary { sources } | Authority::Tertiary { sources } => sources.len(),
        } > MAX_OBJECTS
        {
            return Err(Error::Budget);
        }
        let local_binding = before.local_binding.clone().ok_or(Error::Invalid(
            "publication requires a local physical review",
        ))?;
        if authority.role() != before.identity.role {
            return Err(Error::Invalid(
                "role conversion requires a separate protocol",
            ));
        }
        let sequence = before.sequence.checked_add(1).ok_or(Error::Budget)?;
        if sequence > MAX_PUBLICATIONS as u64 {
            return Err(Error::Budget);
        }
        let mut all = before.bundle.clone();
        all.extend(&material)?;
        let manifest = all.insert(Object::Manifest(Manifest {
            format: Format::Independent,
            identity: before.identity.clone(),
            sequence,
            authority,
        }))?;
        let review = LocalReview {
            target: local_binding.clone(),
            sources,
        };
        let mut intent = PublicationIntent {
            format: Format::Independent,
            operation_id: operation_id.clone(),
            target: before.identity.clone(),
            base: before.checkpoint,
            manifest,
            request_digest: Digest([0; 32]),
            execution_binding: codec::address("execution", &codec::encode(&review)?),
            required_bodies: all
                .bodies
                .keys()
                .map(|key| {
                    let raw = all.objects.get(key).ok_or(Error::Missing(*key))?;
                    let Object::Commit(commit) = codec::decode(raw)? else {
                        return Err(Error::Invalid(
                            "body copy key must identify its immutable commit",
                        ));
                    };
                    Ok((*key, commit.body))
                })
                .collect::<Result<BTreeMap<_, _>>>()?,
        };
        intent.request_digest = verify::request_digest(&intent)?;
        let intent_hash = all.insert(Object::Intent(intent.clone()))?;
        let prepared = all.insert(Object::Prepared(PreparedEvidence {
            format: Format::Independent,
            intent: intent_hash,
            manifest,
            request_digest: intent.request_digest,
            required_bodies: intent.required_bodies.clone(),
        }))?;
        let publication = Publication {
            format: Format::Independent,
            identity: before.identity.clone(),
            sequence,
            previous: before.checkpoint,
            operation_id,
            request_digest: intent.request_digest,
            intent: intent_hash,
            prepared,
            manifest,
        };
        let root = all.insert(Object::Publication(publication.clone()))?;
        // Validate the complete hypothetical closure BEFORE writing any intent.
        // Unreferenced supplied material is rejected, never silently copied.
        let verified = verify::verify(&BundleReader(&all), &before.identity, root)?;
        if verified.bundle != all {
            return Err(Error::Invalid("unreferenced publication material"));
        }
        Ok(Self {
            local_binding,
            review,
            intent,
            publication,
            material: verified.bundle,
        })
    }
    pub fn intent(&self) -> &PublicationIntent {
        &self.intent
    }
    pub fn request_digest(&self) -> Digest {
        self.intent.request_digest
    }
    pub fn publication_digest(&self) -> Result<Digest> {
        Object::Publication(self.publication.clone()).digest()
    }
}
struct BundleReader<'a>(&'a Bundle);
impl ReadStore for BundleReader<'_> {
    fn object(&self, hash: Digest) -> Result<Vec<u8>> {
        self.0
            .objects
            .get(&hash)
            .cloned()
            .ok_or(Error::Missing(hash))
    }
    fn body(&self, hash: Digest) -> Result<Option<Vec<u8>>> {
        Ok(self.0.bodies.get(&hash).cloned())
    }
}

struct CandidateReader<'a> {
    disk: &'a Disk,
    publication: &'a Publication,
}
impl ReadStore for CandidateReader<'_> {
    fn object(&self, hash: Digest) -> Result<Vec<u8>> {
        let object = Object::Publication(self.publication.clone());
        if object.digest()? == hash {
            object.bytes()
        } else {
            self.disk.object(hash)
        }
    }
    fn body(&self, hash: Digest) -> Result<Option<Vec<u8>>> {
        self.disk.body(hash)
    }
}

pub struct WriteSession<'a> {
    disk: &'a Disk,
    _lock: AdvisoryLock,
}
/// Only issued after complete immutable dependencies and prepared evidence are
/// durable. A process crash discards this handle; recovery re-verifies bytes.
pub struct PreparedPublication {
    publication: Publication,
}
impl WriteSession<'_> {
    pub fn begin(&self, request: &PublicationRequest) -> Result<()> {
        self.disk.check_identity()?;
        if request.intent.target != self.disk.identity {
            return Err(Error::Identity);
        }
        let current = self.disk.verify(None)?;
        if let Some(receipt) = current.receipt(&request.intent.operation_id) {
            return if receipt.request_digest == request.intent.request_digest {
                Ok(())
            } else {
                Err(Error::OperationConflict)
            };
        }
        if request.local_binding != self.disk.binding {
            return Err(Error::Identity);
        }
        if current.checkpoint != request.intent.base {
            return Err(Error::StaleBase);
        }
        self.check_source_separation(&request.review)?;
        self.admit_retained(request)?;
        let operation = self.operation(&request.intent.operation_id)?;
        let intent_object = Object::Intent(request.intent.clone());
        let intent_hash = intent_object.digest()?;
        let local = LocalExecution {
            format: Format::Independent,
            review: request.review.clone(),
            intent: intent_hash,
        };
        if let Some(existing) = read_optional(&operation, &name("local.json")?)? {
            let existing: LocalExecution = codec::decode(&existing)?;
            if existing.review.target != local.review.target {
                return Err(Error::Identity);
            }
            if existing.intent != intent_hash {
                return Err(Error::OperationConflict);
            }
        }
        if let Some(existing) = read_optional(&operation, &name("intent.json")?)?
            && existing != intent_object.bytes()?
        {
            return Err(Error::OperationConflict);
        }
        save_exact(&operation, &name("local.json")?, &codec::encode(&local)?)?;
        save_exact(&operation, &name("intent.json")?, &intent_object.bytes()?)?;
        fault::after(fault::Boundary::Intent)?;
        Ok(())
    }
    pub fn prepare(&self, request: &PublicationRequest) -> Result<PreparedPublication> {
        self.begin(request)?;
        let current = self.disk.verify(None)?;
        if let Some(receipt) = current.receipt(&request.intent.operation_id) {
            if receipt.request_digest != request.intent.request_digest {
                return Err(Error::OperationConflict);
            }
            return Ok(PreparedPublication {
                publication: request.publication.clone(),
            });
        }
        if current.checkpoint != request.intent.base {
            return Err(Error::StaleBase);
        }
        for (hash, bytes) in &request.material.bodies {
            self.disk.check_identity()?;
            save_exact(&self.disk.bodies, &name(format!("{hash}.bin"))?, bytes)?;
        }
        let hash = request.publication_digest()?;
        for (object_hash, bytes) in &request.material.objects {
            if *object_hash == hash {
                continue;
            }
            self.disk.check_identity()?;
            save_exact(
                &self.disk.objects,
                &name(format!("{object_hash}.json"))?,
                bytes,
            )?;
        }
        verify::verify(
            &CandidateReader {
                disk: self.disk,
                publication: &request.publication,
            },
            &self.disk.identity,
            hash,
        )?;
        require_bodies(self.disk, &request.intent.required_bodies)?;
        fault::after(fault::Boundary::Material)?;
        let operation = self.operation(&request.intent.operation_id)?;
        save_exact(
            &operation,
            &name("prepared.json")?,
            &self.disk.object(request.publication.prepared)?,
        )?;
        fault::after(fault::Boundary::Prepared)?;
        Ok(PreparedPublication {
            publication: request.publication.clone(),
        })
    }
    pub fn publish(&self, prepared: &PreparedPublication) -> Result<DiskPublicationReceipt> {
        self.disk.check_identity()?;
        let publication = &prepared.publication;
        if publication.identity != self.disk.identity {
            return Err(Error::Identity);
        }
        let current = self.disk.verify(None)?;
        if let Some(receipt) = current.receipt(&publication.operation_id) {
            if receipt.request_digest != publication.request_digest {
                return Err(Error::OperationConflict);
            }
            let decided = &current.publications[&receipt.publication()];
            save_exact(
                &self.disk.publications,
                &name(format!("{:020}.json", decided.sequence))?,
                &Object::Publication(decided.clone()).bytes()?,
            )?;
            self.disk.check_identity()?;
            return self.confirm_publication(receipt, current.checkpoint);
        }
        if current.checkpoint != publication.previous {
            return Err(Error::StaleBase);
        }
        let operation = self.operation(&publication.operation_id)?;
        self.check_local(&operation, publication.intent)?;
        if operation.read_bounded(&name("intent.json")?, MAX_BYTES)?
            != self.disk.object(publication.intent)?
            || operation.read_bounded(&name("prepared.json")?, MAX_BYTES)?
                != self.disk.object(publication.prepared)?
        {
            return Err(Error::Invalid("operation evidence changed"));
        }
        let hash = Object::Publication(publication.clone()).digest()?;
        verify::verify(
            &CandidateReader {
                disk: self.disk,
                publication,
            },
            &self.disk.identity,
            hash,
        )?;
        let Object::Prepared(evidence) = codec::decode(&self.disk.object(publication.prepared)?)?
        else {
            return Err(Error::Invalid("expected prepared evidence"));
        };
        require_bodies(self.disk, &evidence.required_bodies)?;
        // This no-clobber, file+directory-synced decision is the sole commit.
        // No SQLite row or optional management receipt can replace it.
        save_exact(
            &self.disk.publications,
            &name(format!("{:020}.json", publication.sequence))?,
            &Object::Publication(publication.clone()).bytes()?,
        )?;
        fault::after(fault::Boundary::Published)?;
        self.disk.check_identity()?;
        self.confirm_publication(publication.receipt(hash), current.checkpoint)
    }
    fn confirm_publication(
        &self,
        receipt: DiskPublicationReceipt,
        previous: Option<Digest>,
    ) -> Result<DiskPublicationReceipt> {
        // A successful barrier does not prove bytes remained unchanged during
        // I/O. ACK only the exact receipt in a freshly verified chain that still
        // contains the pre-barrier pin and every FULL promised by this decision.
        let observed = self.disk.verify(previous)?;
        if observed.receipt(receipt.operation_id()).as_ref() != Some(&receipt) {
            return Err(Error::Invalid(
                "publication receipt is absent from the final verified chain",
            ));
        }
        let publication = &observed.publications[&receipt.publication()];
        let Object::Prepared(prepared) =
            codec::decode(&observed.bundle.objects[&publication.prepared])?
        else {
            return Err(Error::Invalid("expected final prepared evidence"));
        };
        for body in prepared.required_bodies.keys() {
            if !observed.bundle.bodies.contains_key(body) {
                return Err(Error::RequiredBodyMissing(*body));
            }
        }
        Ok(receipt)
    }
    pub fn resume_prepared(&self, operation_id: &Token) -> Result<PreparedPublication> {
        self.disk.check_identity()?;
        let current = self.disk.verify(None)?;
        if let Some(publication) = current
            .publications
            .values()
            .find(|p| p.identity == self.disk.identity && &p.operation_id == operation_id)
        {
            return Ok(PreparedPublication {
                publication: publication.clone(),
            });
        }
        let operation = PrivateDirectory::open_existing(
            self.disk.operations.path().join(operation_id.as_str()),
        )?;
        let Object::Intent(intent) =
            codec::decode(&operation.read_bounded(&name("intent.json")?, MAX_BYTES)?)?
        else {
            return Err(Error::Invalid("expected intent evidence"));
        };
        let intent_hash = Object::Intent(intent.clone()).digest()?;
        self.check_local(&operation, intent_hash)?;
        if intent.target != self.disk.identity || &intent.operation_id != operation_id {
            return Err(Error::Identity);
        }
        if current.checkpoint != intent.base {
            return Err(Error::StaleBase);
        }
        let Object::Prepared(prepared) =
            codec::decode(&operation.read_bounded(&name("prepared.json")?, MAX_BYTES)?)?
        else {
            return Err(Error::Invalid("expected prepared evidence"));
        };
        require_bodies(self.disk, &prepared.required_bodies)?;
        let prepared_hash = Object::Prepared(prepared).digest()?;
        let Object::Manifest(manifest) = self
            .disk
            .object(intent.manifest)
            .and_then(|raw| codec::decode(&raw))?
        else {
            return Err(Error::Invalid("expected manifest evidence"));
        };
        let publication = Publication {
            format: Format::Independent,
            identity: self.disk.identity.clone(),
            sequence: manifest.sequence,
            previous: intent.base,
            operation_id: intent.operation_id,
            request_digest: intent.request_digest,
            intent: intent_hash,
            prepared: prepared_hash,
            manifest: intent.manifest,
        };
        let root = Object::Publication(publication.clone()).digest()?;
        verify::verify(
            &CandidateReader {
                disk: self.disk,
                publication: &publication,
            },
            &self.disk.identity,
            root,
        )?;
        Ok(PreparedPublication { publication })
    }
    /// Charge all on-disk residue, not just the reachable graph. The bounded
    /// offline slice has no deletion/GC path; stop or failure never frees quota.
    /// This is a conservative capacity gate, not a promised future reservation.
    fn admit_retained(&self, request: &PublicationRequest) -> Result<()> {
        const MAX_RETAINED_BYTES: u64 = 64 * 1024 * 1024;
        const MAX_RETAINED_ENTRIES: usize = MAX_OBJECTS * 8;
        self.disk.check_identity()?;
        let retained = platform_io::fs_safety::bounded_directory_inventory(
            self.disk.root.path(),
            InventoryLimits {
                max_entries: MAX_RETAINED_ENTRIES,
                max_total_bytes: MAX_RETAINED_BYTES,
            },
        )?;
        self.disk.check_identity()?;
        // The whole reviewed bundle plus evidence and a second largest-file
        // staging allowance must fit even if none of its bytes are reusable.
        // Exact retries can therefore fail closed under low free capacity.
        let material_bytes: usize = request
            .material
            .objects
            .values()
            .chain(request.material.bodies.values())
            .map(Vec::len)
            .sum();
        let evidence_bytes = codec::encode(&request.intent)?
            .len()
            .checked_mul(3)
            .ok_or(Error::Budget)?;
        let additional = material_bytes
            .checked_add(evidence_bytes)
            .and_then(|n| n.checked_add(MAX_BYTES))
            .ok_or(Error::Budget)?;
        let additional_entries = request
            .material
            .objects
            .len()
            .checked_add(request.material.bodies.len())
            .and_then(|n| n.checked_add(8))
            .ok_or(Error::Budget)?;
        if retained
            .total_bytes
            .checked_add(additional as u64)
            .is_none_or(|n| n > MAX_RETAINED_BYTES)
            || retained
                .entries
                .checked_add(additional_entries)
                .is_none_or(|n| n > MAX_RETAINED_ENTRIES)
        {
            return Err(Error::Budget);
        }
        Ok(())
    }
    fn check_source_separation(&self, review: &LocalReview) -> Result<()> {
        let target = self.disk.location()?;
        for source in &review.sources {
            let original = platform_io::fs_safety::linux::FileIdentity {
                device: source.binding.device,
                inode: source.binding.inode,
            };
            if target.descends_from(original, source.binding.mount_root)? {
                return Err(Error::Invalid("target now overlaps a reviewed source"));
            }
        }
        Ok(())
    }
    fn check_local(&self, operation: &PrivateDirectory, intent: Digest) -> Result<()> {
        let local: LocalExecution =
            codec::decode(&operation.read_bounded(&name("local.json")?, MAX_BYTES)?)?;
        self.check_source_separation(&local.review)?;
        if local.review.target != self.disk.binding {
            return Err(Error::Identity);
        }
        let bytes = operation.read_bounded(&name("intent.json")?, MAX_BYTES)?;
        let Object::Intent(evidence) = codec::decode(&bytes)? else {
            return Err(Error::Invalid("expected intent"));
        };
        if local.intent != intent
            || codec::address("object", &bytes) != intent
            || codec::address("execution", &codec::encode(&local.review)?)
                != evidence.execution_binding
        {
            return Err(Error::OperationConflict);
        }
        Ok(())
    }
    fn operation(&self, id: &Token) -> Result<PrivateDirectory> {
        Ok(self.disk.operations.create_child(&name(id.as_str())?)?)
    }
}

fn name(value: impl AsRef<std::ffi::OsStr>) -> Result<EntryName> {
    Ok(EntryName::new(value)?)
}
fn read_held(
    directory: &PrivateDirectory,
    name: &EntryName,
) -> Result<Option<(Vec<u8>, std::fs::File)>> {
    let mut file = match directory.open_payload(name, PayloadOpenMode::Existing) {
        Ok(file) => file,
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    if file.metadata()?.len() > MAX_BYTES as u64 {
        return Err(Error::Budget);
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(Error::Budget);
    }
    directory.verify_payload(name, &file)?;
    Ok(Some((bytes, file)))
}
fn read_optional(directory: &PrivateDirectory, name: &EntryName) -> Result<Option<Vec<u8>>> {
    // Authority inspection uses the platform's O_RDONLY bounded reader. A
    // read-only mounted disk must not need an O_RDWR descriptor to be verified.
    match directory.read_bounded(name, MAX_BYTES) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}
fn save_exact(directory: &PrivateDirectory, name: &EntryName, bytes: &[u8]) -> Result<()> {
    if let Some((mut existing, mut file)) = read_held(directory, name)? {
        if existing != bytes {
            return Err(Error::Corrupt(name.as_path().display().to_string()));
        }
        file.sync_all()?;
        directory.verify_payload(name, &file)?;
        directory.sync()?;
        file.rewind()?;
        existing.clear();
        file.by_ref()
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut existing)?;
        if existing != bytes {
            return Err(Error::Corrupt(name.as_path().display().to_string()));
        }
        directory.verify_payload(name, &file)?;
        Ok(())
    } else {
        Ok(AtomicFile::create(directory, name, bytes)?)
    }
}

fn require_bodies(store: &impl ReadStore, required: &BTreeMap<Digest, Body>) -> Result<()> {
    for (hash, body) in required {
        let bytes = store
            .body(*hash)?
            .ok_or(Error::RequiredBodyMissing(*hash))?;
        if bytes.len() as u64 != body.bytes || Digest::body(&bytes) != body.digest {
            return Err(Error::Corrupt(hash.to_string()));
        }
    }
    Ok(())
}

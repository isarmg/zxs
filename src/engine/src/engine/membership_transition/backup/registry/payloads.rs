//! Genuine per-route byte verification for the reserved Backup first slot.
//! Selected targets use their retained native version packages, not an image
//! digest, another target, or the old terminal as a substitute for this copy.
use super::*;
use crate::engine::membership_transition::PreparedBackupFirstCandidate;
use crate::file_stamp::FileStamp;
use std::path::PathBuf;

const MAX_RECEIPTS: usize = 65_536;

pub(super) struct PayloadSource {
    root: DiskRoot,
    disk: DiskConfig,
    identity: MemberIdentity,
    seal: Vec<u8>,
    lease: DiskIoLease,
}
impl PayloadSource {
    pub(super) fn capture(engine: &Engine, disk: &DiskConfig) -> Result<Self> {
        let handle = engine.disk_resources.get(disk)?;
        let root = &handle.root;
        let lease = root.io_binding()?.admit()?;
        engine._locks.verify_disk(disk, root)?;
        let identity =
            admission::member_identity(&engine.config, disk, admission::verify_named_root(root)?);
        let seal = root.read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?;
        validate_stored_seal(&decode_seal(&seal)?, &handle.seal)?;
        Ok(Self {
            root: root.private_child(root.path())?,
            disk: disk.clone(),
            identity,
            seal,
            lease,
        })
    }
    fn recheck(&self) -> Result<()> {
        self.lease.check_running()?;
        self.identity.verify_physical_root(&self.root)?;
        if self
            .root
            .read_bytes(&self.disk.root.join("disk.json"), MAX_METADATA)?
            != self.seal
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "retained Backup payload source changed its original seal",
            ));
        }
        Ok(())
    }
}

struct Location<'a> {
    root: &'a DiskRoot,
    source: Option<&'a PayloadSource>,
    disk_id: &'a str,
    slot_id: &'a str,
    evidence: &'a VersionEvidence,
    directory: PathBuf,
}
impl Location<'_> {
    fn proof(
        &self,
        candidate: &PreparedBackupFirstCandidate,
        finalize: &Finalize,
    ) -> Result<CommitProof> {
        Ok(CommitProof {
            candidate_hash: candidate.hash().into(),
            finalize: finalize.clone(),
            version_hash: object_hash("version", &self.evidence.version)?,
            disk_id: self.disk_id.into(),
            slot_id: self.slot_id.into(),
            route_slot_ids: self.evidence.placement.route_slot_ids.clone(),
        })
    }
    fn receipt(&self, proof: &CommitProof) -> Result<PayloadReceipt> {
        Ok(PayloadReceipt {
            version_id: self.evidence.version.version_id.clone(),
            disk_id: self.disk_id.into(),
            slot_id: self.slot_id.into(),
            proof_hash: object_hash("commit-proof", proof)?,
        })
    }
    fn proof_directory(&self, candidate: &str) -> Result<PathBuf> {
        local_proof_directory_at(self.root, &self.directory, candidate)
    }
    /// The descriptor, checksum manifest and whole payload are read under the
    /// original held root. Reopen-by-name comparisons cannot stand in for this
    /// actual streaming verification. No completed progress is trusted.
    fn verify_body(&self, durable: bool, stop: &AtomicBool) -> Result<()> {
        preparation_checkpoint(stop)?;
        if let Some(source) = self.source {
            source.recheck()?;
        }
        let version = &self.evidence.version;
        let descriptor_path = self.directory.join("version.json");
        let mut descriptor = self.root.open_read_regular(&descriptor_path)?;
        let descriptor_stamp = FileStamp::from_metadata(&descriptor.metadata()?);
        let saved: FileVersion = decode_proof(&read_held(&mut descriptor, MAX_METADATA, stop)?)?;
        if object_hash("version", &saved)? != object_hash("version", version)? {
            return Err(Error::new(
                ErrorCode::ContentCorrupt,
                "first Backup payload descriptor differs from its exact candidate",
            ));
        }
        let checks_path = self.directory.join("checksums.bin");
        let mut checks = self.root.open_read_regular(&checks_path)?;
        let checks_stamp = FileStamp::from_metadata(&checks.metadata()?);
        let length = checksum_manifest_length(version)?;
        let checks_bytes = read_held(&mut checks, length, stop)?;
        if checks_bytes.len() != length
            || blake3::hash(&checks_bytes).to_hex().as_str() != version.checksum_manifest_hash
        {
            return Err(Error::new(
                ErrorCode::ChecksumMismatch,
                "Backup checksum bytes differ from the exact version",
            ));
        }
        let manifest = ChecksumManifest::decode(&checks_bytes)?;
        drop(checks_bytes);
        if manifest.logical_size != version.logical_size
            || manifest.chunk_bytes != version.checksum_chunk_bytes
        {
            return Err(Error::new(
                ErrorCode::ChecksumMismatch,
                "Backup checksum header differs from the exact version",
            ));
        }
        let body_path = match self.source {
            Some(_) => crate::container_storage::content_path_at(
                self.root,
                &self.directory,
                version,
                &mut |_| preparation_checkpoint(stop),
            )?,
            // A prepared target has no serving seal/locator. Do not resolve an
            // arbitrary newly appeared container or fall back to another copy.
            None => self.directory.join("content").join(&version.creation_name),
        };
        let body = self.root.open_read_regular(&body_path)?;
        let body_stamp = FileStamp::from_metadata(&body.metadata()?);
        let body = verify_open_payload_observed(body, manifest, version, &mut |_| {
            preparation_checkpoint(stop)
        })?;
        for (path, file, stamp) in [
            (&descriptor_path, &descriptor, &descriptor_stamp),
            (&checks_path, &checks, &checks_stamp),
            (&body_path, &body, &body_stamp),
        ] {
            if &FileStamp::from_metadata(&file.metadata()?) != stamp
                || &FileStamp::from_metadata(&self.root.open_read_regular(path)?.metadata()?)
                    != stamp
            {
                return Err(Error::new(
                    ErrorCode::ContentCorrupt,
                    "Backup payload inode or bound metadata changed during verification",
                ));
            }
            if durable {
                file.sync_all()?;
                self.root
                    .sync_dir(path.parent().expect("invariant: version file has a parent"))?;
            }
        }
        if let Some(source) = self.source {
            source.recheck()?;
        }
        preparation_checkpoint(stop)
    }
}

/// Not constructible from a JSON receipt vector or a capacity plan.
pub(super) struct VerifiedPayloadReceipts(Vec<PayloadReceipt>);
impl VerifiedPayloadReceipts {
    pub(super) fn into_receipts(self) -> Vec<PayloadReceipt> {
        self.0
    }
}

pub(super) struct PayloadPlan<'a> {
    prepared: &'a PreparedBackupRegistry,
    candidate: &'a PreparedBackupFirstCandidate,
    finalize: &'a Finalize,
    locations: Vec<Location<'a>>,
}
impl<'a> PayloadPlan<'a> {
    pub(super) fn new(
        choices: &'a BackupChoiceContext,
        prepared: &'a PreparedBackupRegistry,
        candidate: &'a PreparedBackupFirstCandidate,
        finalize: &'a Finalize,
    ) -> Result<Self> {
        let selected = prepared.projection().selected_nodes();
        let expected_slots = choices
            .payload_sources
            .keys()
            .map(String::as_str)
            .chain(selected.iter().map(|node| node.identity.slot_id.as_str()))
            .collect::<std::collections::BTreeSet<_>>();
        let mut locations = Vec::new();
        for evidence in &candidate.candidate().versions {
            let actual = evidence
                .placement
                .route_slot_ids
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>();
            if actual != expected_slots || actual.len() != evidence.placement.route_slot_ids.len() {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "first Backup payloads require every selected target and unchanged lower source, not a reduced route",
                ));
            }
            for slot in &evidence.placement.route_slot_ids {
                if locations.len() == MAX_RECEIPTS {
                    return Err(budget());
                }
                let generation = evidence
                    .placement
                    .binding_generations
                    .get(slot)
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::PublicationInvalid,
                            "Backup payload route lacks its exact binding",
                        )
                    })?;
                let (root, source, disk_id) = match choices.payload_sources.get(slot) {
                    Some(source) => {
                        if source.disk.binding_generation != *generation {
                            return Err(Error::new(
                                ErrorCode::DiskIdentityMismatch,
                                "Backup payload source binding changed",
                            ));
                        }
                        source.recheck()?;
                        (&source.root, Some(source), source.disk.disk_id.as_str())
                    }
                    None => {
                        let (root, identity) =
                            prepared.payload_target(slot, *generation)?.ok_or_else(|| {
                                Error::new(
                                    ErrorCode::PublicationInvalid,
                                    "Backup payload has no admitted source or selected target",
                                )
                            })?;
                        (root, None, identity.disk_id.as_str())
                    }
                };
                let directory = root
                    .path()
                    .join("histories")
                    .join(&evidence.version.history_tree_id)
                    .join("versions")
                    .join(&evidence.version.version_id);
                locations.push(Location {
                    root,
                    source,
                    disk_id,
                    slot_id: slot,
                    evidence,
                    directory,
                });
            }
        }
        Ok(Self {
            prepared,
            candidate,
            finalize,
            locations,
        })
    }
    /// All allocations and conservative free-space checks precede the first
    /// new CommitProof. Planned hashes below are used ONLY to calculate sizes.
    pub(super) fn require_capacity(
        &self,
        choices: &BackupChoiceContext,
        finals: &[Receipt],
    ) -> Result<()> {
        let mut planned = self
            .locations
            .iter()
            .map(|location| location.receipt(&location.proof(self.candidate, self.finalize)?))
            .collect::<Result<Vec<_>>>()?;
        sort(&mut planned);
        let publication = Publication {
            candidate_hash: self.candidate.hash().into(),
            finalize: self.finalize.clone(),
            finalize_receipts: finals.to_vec(),
            payload_receipts: planned,
            result: self.candidate.result().clone(),
            recovery: None,
        };
        let object = PreparedMetadataObject::new("publication", &publication, MAX_METADATA)?;
        let receipt_bound = choices.sources.len().checked_mul(2048).ok_or_else(budget)?;
        let mut by_root: BTreeMap<PathBuf, (&DiskRoot, u64)> = BTreeMap::new();
        for location in &self.locations {
            let proof = location.proof(self.candidate, self.finalize)?;
            let proof_bytes = encode_metadata(&proof, MAX_METADATA)?.len();
            let local = LocalPublicationRef {
                candidate: self.candidate.candidate(),
                publication: &publication,
                commit_proof: &proof,
                publication_receipts: &[],
            };
            let local_bound = encode_metadata(&local, MAX_METADATA)?
                .len()
                .checked_add(receipt_bound)
                .filter(|bytes| *bytes <= MAX_METADATA)
                .ok_or_else(budget)?;
            let charge = (proof_bytes as u64 + local_bound as u64) * 2 + 16_384;
            charge_root(&mut by_root, location.root, charge)?;
        }
        for source in choices.sources.values() {
            charge_root(
                &mut by_root,
                &source.root,
                object.bytes().len() as u64 * 2 + receipt_bound as u64 * 2 + 16_384,
            )?;
        }
        for (root, bytes) in by_root.values() {
            if root.available()? < *bytes {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "each Backup payload/publication root needs its own durable proof reserve",
                ));
            }
        }
        self.prepared.recheck_names()
    }
    pub(super) fn install(&self, stop: &AtomicBool) -> Result<VerifiedPayloadReceipts> {
        let mut receipts = Vec::with_capacity(self.locations.len());
        for location in &self.locations {
            self.prepared.recheck_names()?;
            location.verify_body(true, stop)?;
            let proof = location.proof(self.candidate, self.finalize)?;
            let path = location
                .proof_directory(self.candidate.hash())?
                .join("commit-proof.json");
            let bytes = encode_metadata(&proof, MAX_METADATA)?;
            install_preparation_bytes(location.root, &path, &bytes)?;
            if location.root.read_bytes(&path, MAX_METADATA)? != bytes {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "durable Backup CommitProof differs on readback",
                ));
            }
            receipts.push(location.receipt(&proof)?);
        }
        self.prepared.recheck_names()?;
        sort(&mut receipts);
        Ok(VerifiedPayloadReceipts(receipts))
    }
    pub(super) fn observe(&self, stop: &AtomicBool) -> Result<VerifiedPayloadReceipts> {
        let mut receipts = Vec::with_capacity(self.locations.len());
        for location in &self.locations {
            self.prepared.recheck_names()?;
            location.verify_body(false, stop)?;
            let proof = location.proof(self.candidate, self.finalize)?;
            let path = location
                .proof_directory(self.candidate.hash())?
                .join("commit-proof.json");
            if location.root.read_bytes(&path, MAX_METADATA)?
                != encode_metadata(&proof, MAX_METADATA)?
            {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "actual Backup CommitProof does not cover this complete route",
                ));
            }
            receipts.push(location.receipt(&proof)?);
        }
        self.prepared.recheck_names()?;
        sort(&mut receipts);
        Ok(VerifiedPayloadReceipts(receipts))
    }
    pub(super) fn local_publications(
        &self,
        publication: &Publication,
        seal: &PublicationSeal,
        write: bool,
        stop: &AtomicBool,
    ) -> Result<()> {
        verify_publication_seal(self.candidate.candidate(), publication, seal)?;
        for location in &self.locations {
            preparation_checkpoint(stop)?;
            self.prepared.recheck_names()?;
            let proof = location.proof(self.candidate, self.finalize)?;
            let local = LocalPublicationRef {
                candidate: self.candidate.candidate(),
                publication,
                commit_proof: &proof,
                publication_receipts: &seal.receipts,
            };
            let bytes = encode_metadata(&local, MAX_METADATA)?;
            let directory = location.proof_directory(self.candidate.hash())?;
            if location
                .root
                .read_bytes(&directory.join("commit-proof.json"), MAX_METADATA)?
                != encode_metadata(&proof, MAX_METADATA)?
            {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "Backup local publication lost its actual CommitProof",
                ));
            }
            let path = directory.join("publication-proof.json");
            if write {
                install_preparation_bytes(location.root, &path, &bytes)?;
            }
            if location.root.read_bytes(&path, MAX_METADATA)? != bytes {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "Backup local publication differs from actual original-member receipts",
                ));
            }
        }
        self.prepared.recheck_names()
    }
}
fn sort(receipts: &mut [PayloadReceipt]) {
    receipts.sort_by(|a, b| (&a.version_id, &a.disk_id).cmp(&(&b.version_id, &b.disk_id)));
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "Backup first payload proofs exceed the bounded native publication budget",
    )
}
fn read_held(file: &mut ReadFile, maximum: usize, stop: &AtomicBool) -> Result<Vec<u8>> {
    let length = usize::try_from(file.metadata()?.len())
        .ok()
        .filter(|length| *length <= maximum)
        .ok_or_else(budget)?;
    let mut bytes = vec![0; length];
    let mut offset = 0;
    while offset < length {
        preparation_checkpoint(stop)?;
        let end = (offset + 64 * 1024).min(length);
        let count = file.read(&mut bytes[offset..end])?;
        if count == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
        }
        offset += count;
    }
    preparation_checkpoint(stop)?;
    if file.read(&mut [0; 1])? != 0 {
        return Err(Error::new(
            ErrorCode::ContentCorrupt,
            "Backup metadata grew while its bound inode was read",
        ));
    }
    Ok(bytes)
}
fn charge_root<'a>(
    roots: &mut BTreeMap<PathBuf, (&'a DiskRoot, u64)>,
    root: &'a DiskRoot,
    bytes: u64,
) -> Result<()> {
    let entry = roots
        .entry(root.path().to_owned())
        .or_insert((root, RESERVE));
    entry.1 = entry.1.checked_add(bytes).ok_or_else(budget)?;
    Ok(())
}

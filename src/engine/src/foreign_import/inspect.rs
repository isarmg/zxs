use super::*;
use crate::engine::membership_transition::pending::activation;
use data_serialization::{Serialize, de::DeserializeOwned};
use std::{collections::BTreeSet, os::unix::fs::MetadataExt, path::Path};

struct Budget {
    remaining: usize,
}
impl Budget {
    fn observe(&mut self, count: usize) -> Result<()> {
        self.remaining = self.remaining.checked_sub(count).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "foreign history metadata budget exceeded",
            )
        })?;
        Ok(())
    }
}
fn retain_anchor(
    anchors: &mut BTreeMap<PathBuf, ForeignEvidenceDigest>,
    path: &Path,
    bytes: &[u8],
) -> Result<()> {
    let digest = ForeignEvidenceDigest::from_digest(blake3::hash(bytes));
    if anchors
        .insert(path.to_owned(), digest)
        .is_some_and(|old| old != digest)
    {
        return Err(invalid("source evidence changed during inspection"));
    }
    Ok(())
}

struct Evidence<'a> {
    root: &'a DiskRoot,
    disk: DiskConfig,
    budget: &'a mut Budget,
    anchors: BTreeMap<PathBuf, ForeignEvidenceDigest>,
}
impl Evidence<'_> {
    fn read(&mut self, path: &Path) -> Result<Vec<u8>> {
        let bytes = self
            .root
            .read_bytes_observed(path, MAX_METADATA, &mut |n| self.budget.observe(n))?;
        retain_anchor(&mut self.anchors, path, &bytes)?;
        Ok(bytes)
    }
    fn object<T: DeserializeOwned + Serialize>(&mut self, kind: &str, hash: &str) -> Result<T> {
        load_object_observed_at(self.root, &self.disk, kind, hash, &mut |n| {
            self.budget.observe(n)
        })
    }
    fn graph(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        load_authenticated_object_observed_at(self.root, &self.disk, kind, hash, &mut |n| {
            self.budget.observe(n)
        })
    }
    fn publication(
        &mut self,
        record: &Record,
        frame: &str,
        seal: &DiskSeal,
    ) -> Result<(Candidate, Publication)> {
        let candidate: Candidate = self.object("candidate", &record.candidate_hash)?;
        let publication: Publication = self.object("publication", &record.object_hash)?;
        let publication_seal: PublicationSeal =
            decode_proof(&self.read(&publication_seal_path(&self.disk, &record.candidate_hash))?)?;
        verify_publication_seal(&candidate, &publication, &publication_seal)?;
        let root_path = self.root.path().to_owned();
        activation::verify_local_membership(
            &root_path,
            &candidate,
            seal,
            &mut |path| self.read(path),
            None,
        )?;
        if candidate.pool_id != seal.pool_id
            || candidate.sequence != record.sequence
            || !publication_seal.receipts.iter().any(|receipt| {
                receipt.disk_id == seal.disk_id
                    && receipt.slot_id == seal.slot_id
                    && receipt.record == *record
                    && receipt.frame_hash == frame
            })
        {
            return Err(invalid(
                "publication does not bind this source journal and identity",
            ));
        }
        Ok((candidate, publication))
    }
    fn version_at(&mut self, snapshot: &str, version: &str) -> Result<Option<json_codec::Value>> {
        let snapshot: Snapshot = self.object("snapshot", snapshot)?;
        validate_snapshot_schema(&snapshot)?;
        crate::metadata_map::lookup(&snapshot.maps["versions"], version, |kind, hash| {
            self.graph(kind, hash)
        })
    }
    fn unchanged(&mut self) -> Result<()> {
        for (path, digest) in &self.anchors {
            let bytes = self
                .root
                .read_bytes_observed(path, MAX_METADATA, &mut |n| self.budget.observe(n))?;
            if ForeignEvidenceDigest::from_digest(blake3::hash(&bytes)) != *digest {
                return Err(invalid("source authority changed during inspection"));
            }
        }
        Ok(())
    }
}

impl ForeignImportPreview {
    /// Verify only this held root. The original seal's physical fields are
    /// retained as source evidence, never rewritten or used to admit a writer.
    /// Missing metadata, unsupported provenance, torn journals and corrupt local
    /// bodies fail closed. Absent bodies retain their graph and are unavailable.
    pub fn inspect(directory: File, path: PathBuf, limits: ForeignImportLimits) -> Result<Self> {
        let mut budget = Budget {
            remaining: limits.metadata_bytes,
        };
        let OpenedSource {
            root,
            local,
            seal_bytes,
            seal,
            disk,
            source_level,
            journal_path,
            config_anchors,
        } = open_source(directory, path, &mut budget)?;
        let journal = root.read_bytes(&journal_path, limits.journal_bytes)?;
        let journal_digest = ForeignEvidenceDigest::from_digest(blake3::hash(&journal));
        let scan =
            scan_frames_observed(
                journal.as_slice(),
                JournalNamespace::Active,
                &mut |_| Ok(()),
            )?;
        if scan.torn_tail {
            return Err(invalid(
                "torn source journal requires its original recovery, not import",
            ));
        }
        let publications = scan
            .records
            .iter()
            .filter(|(record, _)| record.record_type == RecordType::Publication)
            .collect::<Vec<_>>();
        let (last_record, last_frame) = publications
            .last()
            .copied()
            .ok_or_else(|| invalid("source has no independently published history checkpoint"))?;
        let mut evidence = Evidence {
            root: &root,
            disk,
            budget: &mut budget,
            anchors: config_anchors,
        };
        let (candidate, _) = evidence.publication(last_record, last_frame, &seal)?;
        let BoundHistories {
            manifests,
            versions,
        } = load_histories(&mut evidence, &candidate.history_roots, limits.versions)?;
        let origins = original_publications(&mut evidence, &publications, &seal, &versions)?;
        let histories =
            inspect_histories(&mut evidence, manifests, &origins, limits.payload_bytes)?;
        evidence.unchanged()?;
        let authority_anchors = std::mem::take(&mut evidence.anchors);
        drop(evidence);
        let checkpoint = ForeignCheckpoint {
            candidate_hash: last_record.candidate_hash.clone(),
            publication_hash: last_record.object_hash.clone(),
            sequence: candidate.sequence,
            authority_hash: candidate.generation().map(str::to_owned),
            unpublished_tail: scan
                .records
                .last()
                .is_some_and(|(record, _)| record != last_record),
        };
        let digest = preview_digest(
            &root,
            &seal_bytes,
            &checkpoint,
            &histories,
            &authority_anchors,
        )?;
        let seal_digest = ForeignEvidenceDigest::from_digest(blake3::hash(&seal_bytes));
        let preview = Self {
            root,
            seal_bytes,
            journal_path,
            journal_digest,
            authority_anchors,
            limits,
            source: ForeignSourceIdentity {
                registration: DiskRegistrationIdentity {
                    pool_id: seal.pool_id,
                    disk_id: seal.disk_id,
                    slot_id: seal.slot_id,
                    binding_generation: seal.binding_generation,
                },
                seal_digest,
                role_evidence: match &seal.format {
                    DiskSealFormat::ManagedDirectory => DiskRoleEvidence::InitialConfiguration,
                    DiskSealFormat::MembershipGeneration { authority_hash } => {
                        DiskRoleEvidence::MembershipGeneration {
                            authority_hash: authority_hash.clone(),
                        }
                    }
                },
            },
            local,
            source_level,
            checkpoint,
            histories,
            digest,
        };
        preview.revalidate_source()?;
        Ok(preview)
    }
    pub(super) fn revalidate_source(&self) -> Result<()> {
        if matches!(
            self.source.role_evidence,
            DiskRoleEvidence::InitialConfiguration
        ) && self
            .root
            .open_optional_read_regular(
                &self
                    .root
                    .path()
                    .join("metadata/pool-membership/generation.json"),
            )?
            .is_some()
        {
            return Err(invalid("source generation changed after inspection"));
        }
        DiskRoot::from_directory(
            self.root.path().to_owned(),
            self.root.directory(self.root.path())?,
        )?;
        if self
            .root
            .read_bytes(&self.root.path().join("disk.json"), MAX_METADATA)?
            != self.seal_bytes
            || ForeignEvidenceDigest::from_digest(blake3::hash(
                &self
                    .root
                    .read_bytes(&self.journal_path, self.limits.journal_bytes)?,
            )) != self.journal_digest
        {
            return Err(invalid("foreign source checkpoint changed"));
        }
        let mut budget = Budget {
            remaining: self.limits.metadata_bytes,
        };
        for (path, digest) in &self.authority_anchors {
            let bytes = self
                .root
                .read_bytes_observed(path, MAX_METADATA, &mut |count| budget.observe(count))?;
            if ForeignEvidenceDigest::from_digest(blake3::hash(&bytes)) != *digest {
                return Err(invalid("foreign authority anchor changed"));
            }
        }
        Ok(())
    }
}

struct OpenedSource {
    root: DiskRoot,
    local: ForeignLocalBinding,
    seal_bytes: Vec<u8>,
    seal: DiskSeal,
    disk: DiskConfig,
    source_level: u32,
    journal_path: PathBuf,
    config_anchors: BTreeMap<PathBuf, ForeignEvidenceDigest>,
}

fn open_source(directory: File, path: PathBuf, budget: &mut Budget) -> Result<OpenedSource> {
    let path = absolute_path(&path)?;
    let local_metadata = directory.metadata()?;
    let local = ForeignLocalBinding {
        device: local_metadata.dev(),
        inode: local_metadata.ino(),
    };
    let root = DiskRoot::from_directory(path, directory)?;
    let seal_bytes =
        root.read_bytes_observed(&root.path().join("disk.json"), MAX_METADATA, &mut |n| {
            budget.observe(n)
        })?;
    let seal = decode_seal(&seal_bytes)?;
    validate_id(&seal.pool_id, "pool")?;
    validate_id(&seal.disk_id, "disk")?;
    name_policy::validate_slot(&seal.slot_id)?;
    if seal.binding_generation == 0 {
        return Err(invalid("source binding generation is zero"));
    }
    let mut config_anchors = BTreeMap::new();
    let config = activation::read_local_disk_configuration(root.path(), &seal, &mut |path| {
        let bytes = root.read_bytes_observed(path, MAX_METADATA, &mut |n| budget.observe(n))?;
        retain_anchor(&mut config_anchors, path, &bytes)?;
        Ok(bytes)
    })?;
    let recorded = config
        .disks
        .iter()
        .find(|disk| disk.disk_id == seal.disk_id)
        .expect("invariant: local configuration binds this seal");
    if !recorded.metadata_member {
        return Err(Error::new(
            ErrorCode::FormatUnsupported,
            "data-only source lacks a complete published history catalog",
        ));
    }
    let source_level = recorded.level;
    let mut disk = recorded.clone();
    disk.root = root.path().to_owned();
    let marker = root.path().join("metadata/pool-membership/generation.json");
    let journal_path = match &seal.format {
        DiskSealFormat::ManagedDirectory => {
            if root.open_optional_read_regular(&marker)?.is_some() {
                return Err(invalid(
                    "unpromoted generation is not a stable source checkpoint",
                ));
            }
            log_path(&disk)
        }
        DiskSealFormat::MembershipGeneration { authority_hash } => {
            authority_hash
                .parse::<ForeignEvidenceDigest>()
                .map_err(|error| Error::from(std::io::Error::other(error)))?;
            activation::generation_directory(&disk, authority_hash).join("journal.bin")
        }
    };
    Ok(OpenedSource {
        root,
        local,
        seal_bytes,
        seal,
        disk,
        source_level,
        journal_path,
        config_anchors,
    })
}

struct BoundHistories {
    manifests: Vec<(String, HistoryManifest)>,
    versions: BTreeMap<String, FileVersion>,
}
struct VersionOrigin {
    primary: ForeignPrimaryOrigin,
    publication: String,
}
fn load_histories(
    evidence: &mut Evidence<'_>,
    roots: &crate::metadata_map::Reference,
    maximum_versions: usize,
) -> Result<BoundHistories> {
    let (catalog, _) =
        crate::history_package::resolve_catalog(roots, |kind, hash| evidence.graph(kind, hash))?;
    if catalog.len() > maximum_versions {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "foreign history count exceeds budget",
        ));
    }
    let mut manifests = Vec::new();
    let mut versions = BTreeMap::new();
    for (id, hash) in catalog {
        let manifest = crate::history_package::read_bound_foreign_manifest(&hash, |kind, hash| {
            evidence.graph(kind, hash)
        })?;
        if manifest.history.history_tree_id != id {
            return Err(invalid("history catalog identity differs"));
        }
        for (id, version) in &manifest.versions {
            if versions.len() >= maximum_versions {
                return Err(Error::new(
                    ErrorCode::ResourceLimit,
                    "foreign history version count exceeds budget",
                ));
            }
            if versions.insert(id.clone(), version.clone()).is_some() {
                return Err(invalid("foreign history versions overlap domains"));
            }
        }
        manifests.push((hash, manifest));
    }
    Ok(BoundHistories {
        manifests,
        versions,
    })
}

fn original_publications(
    evidence: &mut Evidence<'_>,
    publications: &[&(Record, String)],
    seal: &DiskSeal,
    versions: &BTreeMap<String, FileVersion>,
) -> Result<BTreeMap<String, VersionOrigin>> {
    let mut origins = BTreeMap::new();
    for (record, frame) in publications {
        let (birth, publication) = evidence.publication(record, frame, seal)?;
        // Restored history and re-protection preserve old identities; they
        // cannot supply a missing original primary binding.
        if publication.result.kind != "workspace.commit" {
            continue;
        }
        for version in &birth.versions {
            let id = &version.version.version_id;
            if !versions
                .get(id)
                .is_some_and(|saved| *saved == version.version)
                || origins.contains_key(id)
            {
                continue;
            }
            if evidence.version_at(&birth.snapshot_hash, id)?
                != Some(json_codec::to_value(&version.version)?)
            {
                return Err(invalid(
                    "original version is absent from its published snapshot",
                ));
            }
            if let Some(previous) = &birth.previous_snapshot
                && evidence.version_at(previous, id)?.is_some()
            {
                continue;
            }
            let primary = original_primary(&birth, &publication, version)?;
            origins.insert(
                id.clone(),
                VersionOrigin {
                    primary,
                    publication: record.object_hash.clone(),
                },
            );
        }
    }
    Ok(origins)
}

fn inspect_histories(
    evidence: &mut Evidence<'_>,
    manifests: Vec<(String, HistoryManifest)>,
    origins: &BTreeMap<String, VersionOrigin>,
    payload_limit: u64,
) -> Result<Vec<ForeignHistory>> {
    let mut histories = Vec::new();
    let mut payload_remaining = payload_limit;
    for (manifest_hash, manifest) in manifests {
        let mut primary = None;
        let mut availability = BTreeMap::new();
        let mut origin_publications = BTreeMap::new();
        for (id, version) in &manifest.versions {
            let VersionOrigin { primary: origin, publication } = origins.get(id).ok_or_else(|| Error::new(ErrorCode::FormatUnsupported, "original version publication is unavailable; a current slot cannot substitute for its original primary identity"))?;
            if primary.as_ref().is_some_and(|prior| prior != origin) {
                return Err(Error::new(
                    ErrorCode::FormatUnsupported,
                    "one connected history has several original primary identities and cannot be split without a new graph proof",
                ));
            }
            primary = Some(origin.clone());
            origin_publications.insert(id.clone(), publication.clone());
            let state = availability_at(
                evidence,
                version,
                manifest.content_states[id],
                &mut payload_remaining,
            )?;
            availability.insert(id.clone(), state);
        }
        let origin = primary.ok_or_else(|| {
            Error::new(
                ErrorCode::FormatUnsupported,
                "empty history has no proven original primary binding",
            )
        })?;
        let group_id = object_hash(
            "foreign-history-group",
            &json_codec::json!({
                "pool":origin.pool_id,"disk":origin.disk_id,"binding":origin.binding_generation.to_string(),"history":manifest.history.history_tree_id
            }),
        )?;
        histories.push(ForeignHistory {
            group_id,
            origin,
            manifest_hash,
            manifest,
            availability,
            origin_publications,
        });
    }
    Ok(histories)
}

fn original_primary(
    candidate: &Candidate,
    publication: &Publication,
    evidence: &VersionEvidence,
) -> Result<ForeignPrimaryOrigin> {
    let placement = &evidence.placement;
    let slot = &evidence.version.creation_primary_slot_id;
    let unique = placement.route_slot_ids.iter().collect::<BTreeSet<_>>();
    if placement.primary_slot_id != *slot
        || placement.route_slot_ids.first() != Some(slot)
        || unique.len() != placement.route_slot_ids.len()
        || placement.target_copies as usize != unique.len()
        || unique.iter().any(|slot| {
            !placement
                .binding_generations
                .get(*slot)
                .is_some_and(|generation| *generation > 0)
        })
    {
        return Err(invalid("original primary placement is incomplete"));
    }
    let mut receipts = publication.payload_receipts.iter().filter(|receipt| {
        receipt.version_id == evidence.version.version_id && receipt.slot_id == *slot
    });
    let receipt = receipts
        .next()
        .ok_or_else(|| invalid("original primary receipt is missing"))?;
    if receipts.next().is_some() {
        return Err(invalid("original primary receipt is ambiguous"));
    }
    validate_id(&receipt.disk_id, "disk")?;
    let proof = CommitProof {
        candidate_hash: object_hash("candidate", candidate)?,
        finalize: publication.finalize.clone(),
        version_hash: object_hash("version", &evidence.version)?,
        disk_id: receipt.disk_id.clone(),
        slot_id: slot.clone(),
        route_slot_ids: placement.route_slot_ids.clone(),
    };
    if receipt.proof_hash != object_hash("commit-proof", &proof)? {
        return Err(invalid(
            "original primary receipt does not bind its complete version proof",
        ));
    }
    Ok(ForeignPrimaryOrigin {
        pool_id: candidate.pool_id.clone(),
        disk_id: receipt.disk_id.clone(),
        slot_id: slot.clone(),
        binding_generation: placement.binding_generations[slot],
    })
}

fn availability_at(
    evidence: &mut Evidence<'_>,
    version: &FileVersion,
    disposition: ContentState,
    remaining: &mut u64,
) -> Result<ForeignVersionAvailability> {
    use ForeignVersionAvailability::*;
    match disposition {
        ContentState::Purged => return Ok(RecordedPurged),
        ContentState::Migrated => return Ok(RecordedMigrated),
        ContentState::Retired => {
            return Err(Error::new(
                storage_contract::ErrorCode::HistoryPackageInvalid,
                "retired-node history requires its full native retirement receipts",
            ));
        }
        ContentState::Available => {}
    }
    let directory = version_dir(
        &evidence.disk,
        &version.history_tree_id,
        &version.version_id,
    );
    if !held_directory_present(evidence.root, &directory)? {
        return Ok(TemporarilyUnavailable);
    }
    let local: FileVersion = decode_strict(&evidence.read(&directory.join("version.json"))?)?;
    if local != *version {
        return Err(invalid("local version differs from the bound history"));
    }
    verify_local_publication_at(
        evidence.root,
        &directory,
        version,
        &mut |n| evidence.budget.observe(n),
        None,
    )?;
    let checksums = read_checksum_manifest_at(evidence.root, &directory, version, &mut |count| {
        evidence.budget.observe(count)
    })?;
    let path =
        crate::container_storage::content_path_at(evidence.root, &directory, version, &mut |n| {
            evidence.budget.observe(n)
        })?;
    if evidence.root.optional_metadata(&path)?.is_none() {
        return Ok(TemporarilyUnavailable);
    }
    *remaining = remaining.checked_sub(version.logical_size).ok_or_else(|| {
        Error::new(
            ErrorCode::ResourceLimit,
            "foreign body verification budget exceeded",
        )
    })?;
    let file = evidence.root.open_read_regular(&path)?;
    verify_open_payload_observed(file, checksums, version, &mut |_| Ok(()))?;
    Ok(VerifiedLocal)
}

fn held_directory_present(root: &DiskRoot, path: &Path) -> Result<bool> {
    let relative = path
        .strip_prefix(root.path())
        .map_err(|_| invalid("foreign object escapes its held source"))?;
    let mut current = root.path().to_owned();
    for component in relative.components() {
        current.push(component);
        if root.optional_metadata(&current)?.is_none() {
            return Ok(false);
        }
        root.directory_identity(&current)?;
    }
    Ok(true)
}

fn preview_digest(
    root: &DiskRoot,
    seal: &[u8],
    checkpoint: &ForeignCheckpoint,
    histories: &[ForeignHistory],
    authority_anchors: &BTreeMap<PathBuf, ForeignEvidenceDigest>,
) -> Result<ForeignEvidenceDigest> {
    let anchors = authority_anchors
        .iter()
        .map(|(path, digest)| {
            (
                path.strip_prefix(root.path())
                    .expect("invariant: anchor read through the held root")
                    .to_owned(),
                digest,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let histories = histories.iter().map(|history| {
        let availability = history.availability.iter().map(|(id, state)| (id.clone(), match state { ForeignVersionAvailability::VerifiedLocal => "verified-local", ForeignVersionAvailability::TemporarilyUnavailable => "temporarily-unavailable", ForeignVersionAvailability::RecordedPurged => "recorded-purged", ForeignVersionAvailability::RecordedMigrated => "recorded-migrated" })).collect::<BTreeMap<_, _>>();
        json_codec::json!({"group":history.group_id,"manifest":history.manifest_hash,"origins":history.origin_publications,"availability":availability})
    }).collect::<Vec<_>>();
    Ok(object_hash("foreign-history-preview", &json_codec::json!({"seal":ForeignEvidenceDigest::from_digest(blake3::hash(seal)),"candidate":checkpoint.candidate_hash,"publication":checkpoint.publication_hash,"unpublished_tail":checkpoint.unpublished_tail,"anchors":anchors,"histories":histories}))?.parse().expect("invariant: object hash is a content hash"))
}

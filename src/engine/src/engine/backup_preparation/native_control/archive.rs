//! Original fixed-member journals and every referenced published State/history
//! graph. This archive preserves evidence; it never creates a new disk vote.
use super::*;
use crate::engine::backup_preparation::native_control::databases::scoped::{
    encoding_memo::Transaction,
};
use crate::disk::{DiskSeal, DiskSealFormat};
use crate::history_package::{AdmittedState, bound_history_objects_with};
use crate::journal::{
    AuthenticatedMetadataObject, Candidate, CommitProof, JournalFrameReader, Publication,
    PublicationSeal, RawMetadataRead, Record, RecordType, Snapshot, decode_proof,
    resolve_admitted_snapshot,
    verify_publication_seal,
};
use crate::publication_evidence::{verified_graph_object_at, verified_publication_seal_at};
use crate::persist::layout;
use data_serialization::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};

#[path = "generation_objects.rs"]
pub(super) mod generation_objects;

const FORMAT: &str = "zxs-container-tags-backup-native-control";
const ROOT: &str = "native-control";
const MAX_BYTES: usize = 128 * 1024 * 1024;
const MAX_READS: usize = 32768;
const MAX_PUBLICATIONS: usize = 2048;
const MAX_FRAMES: usize = 32768;
const MAX_JOURNAL: usize = 16 * 1024 * 1024;
const MAX_MANIFEST: usize = 4 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct OriginalMember {
    disk_id: String,
    slot_id: String,
    seal_hash: String,
    journal_hash: String,
    journal_bytes: usize,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Published {
    candidate: String,
    publication: String,
    seal: String,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Manifest {
    format: String,
    config: Config,
    #[serialization(with = "storage_contract::u64_string")]
    through_sequence: u64,
    through_snapshot: String,
    members: Vec<OriginalMember>,
    publications: Vec<Published>,
}

/// Counts refer to the authenticated original prefix, not target votes or a
/// migration decision. It deliberately carries no restore/activation capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupNativeControlReport {
    pub archive_hash: String,
    pub state_hash: String,
    pub snapshot_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub publications: usize,
    pub original_members: usize,
    pub journal_frames: usize,
    pub metadata_read_steps: usize,
    pub verified_metadata_bytes: usize,
}

#[derive(Default)]
struct Budget {
    bytes: usize,
    reads: usize,
}
impl Budget {
    fn account(&mut self, bytes: usize) -> Result<()> {
        self.reads += 1;
        self.bytes = self.bytes.checked_add(bytes).ok_or_else(budget_error)?;
        if self.reads > MAX_READS || self.bytes > MAX_BYTES {
            return Err(budget_error());
        }
        Ok(())
    }
}
fn budget_error() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "complete native prefix exceeds its read/byte budget; no partial archive is complete",
    )
}
fn invalid(detail: &str) -> Error {
    Error::new(ErrorCode::PublicationInvalid, detail)
}
fn hash_valid(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn object_path(root: &Path, kind: &str, hash: &str) -> Result<PathBuf> {
    if !matches!(
        kind,
        "candidate"
            | "publication"
            | "finalize"
            | "backup-original-seal"
            | "backup-publication-seal"
            | "commit-proof"
            | "snapshot"
            | "state-control-auth"
            | "state-page"
            | "metadata-index"
            | "history-manifest"
    ) || !hash_valid(hash)
    {
        return Err(invalid("invalid native archive object address"));
    }
    Ok(root
        .join(ROOT)
        .join("objects")
        .join(kind)
        .join(format!("{hash}.json")))
}
fn manifest_path(root: &Path) -> PathBuf {
    root.join(ROOT).join("manifest.json")
}
fn journal_path(root: &Path, index: usize) -> PathBuf {
    root.join(ROOT)
        .join("journals")
        .join(format!("{index:04}.bin"))
}

trait Source {
    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject>;
    fn journal(&mut self, index: usize, expected: &OriginalMember) -> Result<Vec<u8>>;
    fn budget(&self) -> &Budget;
    fn account(&mut self, bytes: usize) -> Result<()>;
}
fn typed<T: DeserializeOwned + Serialize>(
    source: &mut impl Source,
    kind: &'static str,
    hash: &str,
) -> Result<T> {
    let value = source.object(kind, hash)?.into_value();
    let parsed: T = T::deserialize(&value)?;
    if json_codec::to_value(&parsed)? != value {
        return Err(invalid(
            "native proof projection differs from its authenticated bytes",
        ));
    }
    Ok(parsed)
}

struct Capture<'a> {
    engine: &'a Engine,
    members: Vec<DiskConfig>,
    objects: BTreeMap<(String, String), Vec<u8>>,
    journals: Vec<Vec<u8>>,
    budget: Budget,
    stored_bytes: usize,
}
impl Capture<'_> {
    fn insert(&mut self, kind: &'static str, bytes: Vec<u8>) -> Result<String> {
        let value = storage_contract::strict_json(&bytes)?;
        let hash = storage_contract::object_hash_value(kind, &value)?;
        object_path(Path::new("/"), kind, &hash)?;
        match self.objects.entry((kind.into(), hash.clone())) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                self.stored_bytes = self
                    .stored_bytes
                    .checked_add(bytes.len())
                    .ok_or_else(budget_error)?;
                if self.stored_bytes > MAX_BYTES {
                    return Err(budget_error());
                }
                entry.insert(bytes);
            }
            std::collections::btree_map::Entry::Occupied(entry) if entry.get() == &bytes => {}
            _ => return Err(invalid("native object has conflicting original bytes")),
        }
        Ok(hash)
    }
}
impl Source for Capture<'_> {
    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        object_path(Path::new("/"), kind, hash)?;
        if let Some(bytes) = self.objects.get(&(kind.into(), hash.into())) {
            self.budget.account(bytes.len())?;
            return AuthenticatedMetadataObject::authenticate(
                kind,
                hash,
                storage_contract::strict_json(bytes)?,
            );
        }
        let object =
            verified_graph_object_at(&self.engine.disk_resources, &self.members, kind, hash)?;
        self.budget.account(object.bytes().len())?;
        self.stored_bytes = self
            .stored_bytes
            .checked_add(object.bytes().len())
            .ok_or_else(budget_error)?;
        if self.stored_bytes > MAX_BYTES {
            return Err(budget_error());
        }
        self.objects
            .insert((kind.into(), hash.into()), object.bytes().to_vec());
        Ok(object)
    }
    fn journal(&mut self, index: usize, expected: &OriginalMember) -> Result<Vec<u8>> {
        let bytes = self
            .journals
            .get(index)
            .ok_or_else(|| invalid("original journal missing"))?;
        if bytes.len() != expected.journal_bytes {
            return Err(invalid("original journal length differs"));
        }
        self.budget.account(bytes.len())?;
        Ok(bytes.clone())
    }
    fn budget(&self) -> &Budget {
        &self.budget
    }
    fn account(&mut self, bytes: usize) -> Result<()> {
        self.budget.account(bytes)
    }
}

struct Target<'a, 'memo> {
    root: &'a DiskRoot,
    stop: &'a AtomicBool,
    budget: Budget,
    encodings: Option<&'a mut Transaction<'memo>>,
}
impl Source for Target<'_, '_> {
    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        crate::history_package::preparation_checkpoint(self.stop)?;
        let bytes = self.root.read_bytes(
            &object_path(self.root.path(), kind, hash)?,
            MAX_BYTES - self.budget.bytes,
        )?;
        self.budget.account(bytes.len())?;
        let read = RawMetadataRead::parse(kind, hash, &bytes)?;
        match self.encodings.as_deref_mut() {
            Some(encodings) => encodings.metadata(read),
            None => Ok(read.authenticate(None)?.into_object()),
        }
    }
    fn journal(&mut self, index: usize, expected: &OriginalMember) -> Result<Vec<u8>> {
        crate::history_package::preparation_checkpoint(self.stop)?;
        if expected.journal_bytes > MAX_JOURNAL {
            return Err(budget_error());
        }
        let bytes = self.root.read_bytes(
            &journal_path(self.root.path(), index),
            MAX_JOURNAL.min(MAX_BYTES - self.budget.bytes),
        )?;
        self.budget.account(bytes.len())?;
        Ok(bytes)
    }
    fn budget(&self) -> &Budget {
        &self.budget
    }
    fn account(&mut self, bytes: usize) -> Result<()> {
        self.budget.account(bytes)
    }
}

pub(super) struct PreparedArchive {
    manifest: Vec<u8>,
    objects: BTreeMap<(String, String), Vec<u8>>,
    journals: Vec<Vec<u8>>,
    pub(super) report: BackupNativeControlReport,
}
impl PreparedArchive {
    pub(super) fn freeze(engine: &Engine, job: &Job) -> Result<Self> {
        if engine.config.generation().is_some() || engine.current_namespaces.is_some() {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "native backup archive currently requires the complete fixed genesis membership; generation archives are not silently omitted",
            ));
        }
        let members = engine
            .config
            .disks
            .iter()
            .filter(|disk| disk.metadata_member)
            .cloned()
            .collect::<Vec<_>>();
        if members.is_empty() || members.len() > 32 {
            return Err(budget_error());
        }
        let mut capture = Capture {
            engine,
            members: members.clone(),
            objects: BTreeMap::new(),
            journals: Vec::new(),
            budget: Budget::default(),
            stored_bytes: 0,
        };
        let mut origins = Vec::new();
        let mut journal_bytes = 0usize;
        for disk in &members {
            let root = &engine.disk_resources.get(disk)?.root;
            let seal_bytes = root.read_bytes(
                &disk.root.join(layout::DISK_IDENTITY.as_path()),
                MAX_MANIFEST,
            )?;
            let seal: DiskSeal = decode_proof(&seal_bytes)?;
            let identity = PhysicalBinding::of(root)?;
            if (seal.device, seal.inode) != (identity.device, identity.inode) {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "source seal no longer names its held root",
                ));
            }
            let seal_hash = capture.insert("backup-original-seal", seal_bytes)?;
            let bytes = root.read_bytes(
                &crate::journal::log_path(disk),
                MAX_JOURNAL.min(MAX_BYTES - journal_bytes),
            )?;
            journal_bytes = journal_bytes
                .checked_add(bytes.len())
                .ok_or_else(budget_error)?;
            let origin = OriginalMember {
                disk_id: disk.disk_id.clone(),
                slot_id: disk.slot_id.clone(),
                seal_hash,
                journal_hash: blake3::hash(&bytes).to_hex().to_string(),
                journal_bytes: bytes.len(),
            };
            origins.push(origin);
            capture.stored_bytes = capture
                .stored_bytes
                .checked_add(bytes.len())
                .ok_or_else(budget_error)?;
            if capture.stored_bytes > MAX_BYTES {
                return Err(budget_error());
            }
            capture.journals.push(bytes);
        }
        let mut refs = BTreeMap::<u64, (String, String)>::new();
        let mut frames = 0usize;
        // The writer cache retains only its current slot. Discover the complete
        // object set from the same original journal bytes archived above; the
        // full verifier below still authenticates every member's receipts.
        for bytes in &capture.journals {
            let mut reader = JournalFrameReader::new(bytes.as_slice());
            while let Some((record, _)) =
                reader.next_record_observed(&mut |count| capture.budget.account(count))?
            {
                frames = frames.checked_add(1).ok_or_else(budget_error)?;
                if frames > MAX_FRAMES {
                    return Err(budget_error());
                }
                if record.record_type != RecordType::Publication {
                    continue;
                }
                if record.sequence == 0 || record.sequence > job.request.expected_sequence {
                    return Err(invalid("native prefix is outside the frozen task"));
                }
                let pair = (record.candidate_hash.clone(), record.object_hash.clone());
                match refs.insert(record.sequence, pair.clone()) {
                    Some(old) if old != pair => return Err(invalid("native publication fork")),
                    _ => {}
                }
                if refs.len() > MAX_PUBLICATIONS {
                    return Err(budget_error());
                }
            }
            if reader.torn_tail || reader.valid_bytes != bytes.len() as u64 {
                return Err(invalid("original journal has an incomplete frame"));
            }
        }
        let mut publications = Vec::with_capacity(refs.len());
        for (candidate_hash, publication_hash) in refs.values() {
            let candidate: Candidate = typed(&mut capture, "candidate", candidate_hash)?;
            let publication: Publication = typed(&mut capture, "publication", publication_hash)?;
            let seal = verified_publication_seal_at(
                &engine.disk_resources,
                &members,
                &candidate,
                &publication,
            )?;
            let seal_hash = capture.insert("backup-publication-seal", canonical_json(&seal)?)?;
            // CommitProof has a closed deterministic representation. Reconstruct
            // only the bytes whose hash the original publication already binds.
            // No new receipt, vote, disk identity or payload permission is made.
            for evidence in &candidate.versions {
                for slot in &evidence.placement.route_slot_ids {
                    let disk = members_config_disk(engine.config(), slot)?;
                    let proof = CommitProof {
                        candidate_hash: candidate_hash.clone(),
                        finalize: publication.finalize.clone(),
                        version_hash: object_hash("version", &evidence.version)?,
                        disk_id: disk.disk_id.clone(),
                        slot_id: slot.clone(),
                        route_slot_ids: evidence.placement.route_slot_ids.clone(),
                    };
                    capture.insert("commit-proof", canonical_json(&proof)?)?;
                }
            }
            publications.push(Published {
                candidate: candidate_hash.clone(),
                publication: publication_hash.clone(),
                seal: seal_hash,
            });
        }
        let manifest = Manifest {
            format: FORMAT.into(),
            config: engine.config().clone(),
            through_sequence: job.request.expected_sequence,
            through_snapshot: job.snapshot.clone(),
            members: origins,
            publications,
        };
        let bytes = canonical_json(&manifest)?;
        if bytes.len() > MAX_MANIFEST {
            return Err(budget_error());
        }
        // Apply the same target verifier to captured bytes. This is runtime
        // admission, not an engineering test or a substitute for final cutover.
        capture.budget = Budget::default();
        let (report, state) = verify(&manifest, &bytes, job, &mut capture)?;
        if canonical_json(state.state())? != canonical_json(engine.state())? {
            return Err(invalid(
                "native prefix differs from the admitted live State",
            ));
        }
        Ok(Self {
            manifest: bytes,
            objects: capture.objects,
            journals: capture.journals,
            report,
        })
    }

    pub(super) fn remaining_bytes(&self, root: &DiskRoot) -> Result<u64> {
        let mut bytes = 0u64;
        self.visit(root.path(), |path, content| {
            if !crate::history_package::preparation_path_exists(root, &path)? {
                bytes = bytes
                    .checked_add(content.len() as u64)
                    .ok_or_else(budget_error)?;
            }
            Ok(())
        })?;
        Ok(bytes)
    }
    fn visit(
        &self,
        root: &Path,
        mut visit: impl FnMut(PathBuf, &[u8]) -> Result<()>,
    ) -> Result<()> {
        for ((kind, hash), bytes) in &self.objects {
            visit(object_path(root, kind, hash)?, bytes)?;
        }
        for (index, bytes) in self.journals.iter().enumerate() {
            visit(journal_path(root, index), bytes)?;
        }
        visit(manifest_path(root), &self.manifest)
    }
    pub(super) fn copy(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        self.visit(root.path(), |path, bytes| {
            crate::history_package::preparation_checkpoint(stop)?;
            crate::history_package::install_preparation_bytes(root, &path, bytes)
        })
    }
}

fn members_config_disk<'a>(config: &'a Config, slot: &str) -> Result<&'a DiskConfig> {
    config
        .disks
        .iter()
        .find(|disk| disk.slot_id == slot)
        .ok_or_else(|| invalid("native payload names no original disk"))
}

type JournalSlots = BTreeMap<u64, Vec<(Record, String)>>;
fn original_journals(
    manifest: &Manifest,
    source: &mut impl Source,
) -> Result<(Vec<JournalSlots>, usize)> {
    let members = manifest
        .config
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
        .collect::<Vec<_>>();
    if members.len() != manifest.members.len() || members.is_empty() || members.len() > 32 {
        return Err(invalid("native archive omits original fixed members"));
    }
    let mut output = Vec::new();
    let mut total_frames = 0usize;
    for (index, (disk, member)) in members.iter().zip(&manifest.members).enumerate() {
        let seal: DiskSeal = typed(source, "backup-original-seal", &member.seal_hash)?;
        if member.disk_id != disk.disk_id
            || member.slot_id != disk.slot_id
            || seal.disk_id != disk.disk_id
            || seal.slot_id != disk.slot_id
            || seal.pool_id != manifest.config.pool_id
            || seal.binding_generation != disk.binding_generation
            || seal.format != DiskSealFormat::ManagedDirectory
            || seal.simulation != (manifest.config.mode == Mode::Development)
        {
            return Err(invalid(
                "archived member identity differs from the original configuration",
            ));
        }
        let bytes = source.journal(index, member)?;
        if bytes.len() != member.journal_bytes
            || blake3::hash(&bytes).to_hex().as_str() != member.journal_hash
        {
            return Err(invalid("original journal bytes differ"));
        }
        let mut reader = JournalFrameReader::new(bytes.as_slice());
        let mut slots: JournalSlots = BTreeMap::new();
        let mut last_publication = None;
        while let Some((record, hash)) =
            reader.next_record_observed(&mut |count| source.account(count))?
        {
            total_frames = total_frames.checked_add(1).ok_or_else(budget_error)?;
            if total_frames > MAX_FRAMES {
                return Err(budget_error());
            }
            if record.sequence == 0
                || record.sequence > manifest.through_sequence
                || !matches!(
                    record.record_type,
                    RecordType::Prepare
                        | RecordType::Accept
                        | RecordType::Finalize
                        | RecordType::Publication
                )
            {
                return Err(invalid(
                    "native prefix contains unsupported recovery or membership decisions",
                ));
            }
            last_publication =
                (record.record_type == RecordType::Publication).then_some(record.sequence);
            slots
                .entry(record.sequence)
                .or_default()
                .push((record, hash));
        }
        if reader.torn_tail
            || reader.valid_bytes != bytes.len() as u64
            || last_publication != Some(manifest.through_sequence)
        {
            return Err(invalid(
                "original journal is torn, pending, or lacks the frozen head",
            ));
        }
        if slots.len() != manifest.publications.len() {
            return Err(invalid("original journal has an omitted/unpublished slot"));
        }
        output.push(slots);
    }
    Ok((output, total_frames))
}

fn verify(
    manifest: &Manifest,
    bytes: &[u8],
    job: &Job,
    source: &mut impl Source,
) -> Result<(BackupNativeControlReport, AdmittedState)> {
    verify_observed(manifest, bytes, job, source, &mut |_, _, _, _| Ok(()))
}

fn verify_observed(
    manifest: &Manifest,
    bytes: &[u8],
    job: &Job,
    source: &mut impl Source,
    observe: &mut impl FnMut(
        &crate::ValidatedConfig, &Candidate, &str, &AdmittedState,
    ) -> Result<()>,
) -> Result<(BackupNativeControlReport, AdmittedState)> {
    if manifest.format != FORMAT
        || manifest.publications.is_empty()
        || manifest.publications.len() > MAX_PUBLICATIONS
        || manifest.through_sequence != job.request.expected_sequence
        || manifest.through_snapshot != job.snapshot
        || manifest.config.pool_id != job.pool_id
        || object_hash("backup-preparation-config", &manifest.config)? != job.configuration_hash
    {
        return Err(invalid(
            "native prefix does not bind the complete frozen task",
        ));
    }
    let config = crate::ValidatedConfig::try_from(manifest.config.clone())?;
    source.account(bytes.len())?;
    let (journals, journal_frames) = original_journals(manifest, source)?;
    let mut previous: Option<AdmittedState> = None;
    let mut snapshot: Option<String> = None;
    for (index, published) in manifest.publications.iter().enumerate() {
        let candidate: Candidate = typed(source, "candidate", &published.candidate)?;
        config.verify_original_candidate_membership(&candidate)?;
        if candidate.generation().is_some()
            || candidate.registry().is_some()
            || candidate.sequence != index as u64 + 1
            || candidate.pool_id != job.pool_id
            || candidate.previous_snapshot != snapshot
        {
            return Err(invalid(
                "native prefix does not extend the exact genesis publication chain",
            ));
        }
        let publication: Publication = typed(source, "publication", &published.publication)?;
        let seal: PublicationSeal = typed(source, "backup-publication-seal", &published.seal)?;
        if publication.recovery.is_some() {
            return Err(invalid(
                "degraded native publication is not supported by this control archive",
            ));
        }
        verify_publication_seal(&candidate, &publication, &seal)?;
        let finalize_hash = verify_journal_receipts(
            &candidate,
            &publication,
            &seal,
            published,
            &manifest.members,
            &journals,
        )?;
        let finalize: crate::journal::Finalize =
            typed(source, "finalize", finalize_hash.as_str())?;
        if finalize != publication.finalize {
            return Err(invalid("original FINALIZE differs"));
        }
        let descriptor: Snapshot = typed(source, "snapshot", &candidate.snapshot_hash)?;
        let admitted =
            resolve_admitted_snapshot(descriptor, |kind, hash| source.object(kind, hash))?;
        let state = admitted.state();
        if state.sequence != candidate.sequence
            || state.operations.get(&candidate.operation_id) != Some(&publication.result)
            || !state
                .operation_principals
                .contains_key(&candidate.operation_id)
        {
            return Err(invalid(
                "native publication omits or changes its exact original receipt",
            ));
        }
        verify_receipt_prefix(
            previous.as_ref().map(AdmittedState::state),
            state,
            &candidate,
        )?;
        bound_history_objects_with(&candidate, Some(&admitted), |kind, hash| {
            source.object(kind, hash)
        })?;
        verify_payloads(&manifest.config, &candidate, &publication, source)?;
        // The observer can build bounded in-memory derived material. It must
        // not install/authorize anything until this entire prefix returns.
        observe(&config, &candidate, &published.candidate, &admitted)?;
        snapshot = Some(candidate.snapshot_hash.clone());
        previous = Some(admitted);
    }
    let state = previous.expect("invariant: admitted nonempty native prefix");
    if state.state().sequence != manifest.through_sequence
        || snapshot.as_deref() != Some(manifest.through_snapshot.as_str())
    {
        return Err(invalid("native archive stops before the frozen boundary"));
    }
    let report = BackupNativeControlReport {
        archive_hash: object_hash(FORMAT, manifest)?,
        state_hash: object_hash("backup-prepared-state", state.state())?,
        snapshot_hash: manifest.through_snapshot.clone(),
        sequence: manifest.through_sequence,
        publications: manifest.publications.len(),
        original_members: manifest.members.len(),
        journal_frames,
        metadata_read_steps: source.budget().reads,
        verified_metadata_bytes: source.budget().bytes,
    };
    Ok((report, state))
}

fn verify_receipt_prefix(
    previous: Option<&State>,
    next: &State,
    candidate: &Candidate,
) -> Result<()> {
    let actor = next
        .operation_principals
        .get(&candidate.operation_id)
        .ok_or_else(|| invalid("native operation has no original principal"))?;
    Engine::require_actor(actor)?;
    let old_count = previous.map_or(0, |state| state.operations.len());
    if next.operations.len() != old_count + 1
        || next.operation_principals.len() != next.operations.len()
        || previous.is_some_and(|state| {
            state
                .operations
                .iter()
                .any(|(key, value)| next.operations.get(key) != Some(value))
                || state
                    .operation_principals
                    .iter()
                    .any(|(key, value)| next.operation_principals.get(key) != Some(value))
                || !next.events.starts_with(&state.events)
                || state.operations.contains_key(&candidate.operation_id)
        })
    {
        return Err(invalid(
            "native prefix changed or dropped an original permanent receipt",
        ));
    }
    let start = previous.map_or(0, |state| state.events.len());
    if next.events.len() != start + 1
        || next.events[start].operation_id != candidate.operation_id
        || next.events[start].sequence != candidate.sequence
    {
        return Err(invalid(
            "native prefix event does not bind the original operation",
        ));
    }
    Ok(())
}

// Constructed only from this exact FINALIZE value by the canonical hash authority.
// Carrying the digest does not admit the independent target object or its bytes.
struct FinalizeDigest(String);

impl FinalizeDigest {
    fn compute(finalize: &crate::journal::Finalize) -> Result<Self> {
        Ok(Self(object_hash("finalize", finalize)?))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

fn verify_journal_receipts(
    candidate: &Candidate,
    publication: &Publication,
    seal: &PublicationSeal,
    published: &Published,
    members: &[OriginalMember],
    journals: &[JournalSlots],
) -> Result<FinalizeDigest> {
    let finalize_hash = FinalizeDigest::compute(&publication.finalize)?;
    for (member, slots) in members.iter().zip(journals) {
        let records = slots
            .get(&candidate.sequence)
            .ok_or_else(|| invalid("original member omits a publication slot"))?;
        for (record, _) in records {
            let expected = match record.record_type {
                RecordType::Prepare => candidate.snapshot_hash.as_str(),
                RecordType::Accept => published.candidate.as_str(),
                RecordType::Finalize => finalize_hash.as_str(),
                RecordType::Publication => published.publication.as_str(),
                _ => return Err(invalid("unsupported original journal record")),
            };
            if record.candidate_hash != published.candidate || record.object_hash != expected {
                return Err(invalid("original journal references another decision"));
            }
        }
        for receipt in publication
            .finalize
            .accepts
            .iter()
            .chain(&publication.finalize_receipts)
            .chain(&seal.receipts)
            .filter(|receipt| receipt.disk_id == member.disk_id)
        {
            if receipt.slot_id != member.slot_id
                || !records
                    .iter()
                    .any(|(record, hash)| record == &receipt.record && hash == &receipt.frame_hash)
            {
                return Err(invalid(
                    "original receipt is absent from its owner's journal",
                ));
            }
        }
        if records
            .iter()
            .filter(|(record, _)| record.record_type == RecordType::Publication)
            .count()
            != 1
        {
            return Err(invalid("original journal repeats its publication slot"));
        }
    }
    Ok(finalize_hash)
}

fn verify_payloads(
    config: &Config,
    candidate: &Candidate,
    publication: &Publication,
    source: &mut impl Source,
) -> Result<()> {
    let mut expected = BTreeSet::new();
    for evidence in &candidate.versions {
        let mut route = BTreeSet::new();
        if evidence.placement.version_id != evidence.version.version_id
            || evidence.placement.route_slot_ids.is_empty()
        {
            return Err(invalid("native payload has no complete route"));
        }
        for slot in &evidence.placement.route_slot_ids {
            if !route.insert(slot) {
                return Err(invalid("native payload repeats a route slot"));
            }
            let disk = members_config_disk(config, slot)?;
            let key = (
                evidence.version.version_id.as_str(),
                disk.disk_id.as_str(),
                slot.as_str(),
            );
            if !expected.insert(key) {
                return Err(invalid("native candidate repeats payload evidence"));
            }
            let receipt = publication
                .payload_receipts
                .iter()
                .find(|r| r.version_id == key.0 && r.disk_id == key.1 && r.slot_id == key.2)
                .ok_or_else(|| invalid("original payload receipt missing"))?;
            let proof: CommitProof = typed(source, "commit-proof", &receipt.proof_hash)?;
            if proof.candidate_hash != publication.candidate_hash
                || proof.disk_id != disk.disk_id
                || proof.slot_id != *slot
                || proof.version_hash != object_hash("version", &evidence.version)?
                || proof.route_slot_ids != evidence.placement.route_slot_ids
                || proof.finalize != publication.finalize
            {
                return Err(invalid(
                    "original payload proof differs from certified evidence",
                ));
            }
        }
    }
    let actual = publication
        .payload_receipts
        .iter()
        .map(|r| {
            (
                r.version_id.as_str(),
                r.disk_id.as_str(),
                r.slot_id.as_str(),
            )
        })
        .collect::<BTreeSet<_>>();
    if actual != expected || actual.len() != publication.payload_receipts.len() {
        return Err(invalid(
            "publication has extra or duplicate payload receipts",
        ));
    }
    Ok(())
}

pub(super) fn read(
    root: &DiskRoot,
    job: &Job,
    expected: &str,
    stop: &AtomicBool,
) -> Result<BackupNativeControlReport> {
    let (report, _) = read_observed(root, job, expected, stop, &mut |_, _, _, _| Ok(()))?;
    Ok(report)
}

pub(super) fn read_observed(
    root: &DiskRoot,
    job: &Job,
    expected: &str,
    stop: &AtomicBool,
    observe: &mut impl FnMut(
        &crate::ValidatedConfig, &Candidate, &str, &AdmittedState,
    ) -> Result<()>,
) -> Result<(BackupNativeControlReport, AdmittedState)> {
    read_observed_at(root, job, ExpectedArchiveDigest(expected), stop, observe, None)
}

// An expectation, not an admitted archive or a substitute for manifest readback.
struct ExpectedArchiveDigest<'a>(&'a str);

pub(super) fn read_with_encodings(
    root: &DiskRoot,
    job: &Job,
    task: &NativeTask,
    stop: &AtomicBool,
    encodings: &mut Transaction<'_>,
) -> Result<BackupNativeControlReport> {
    let (report, _) = read_observed_with_encodings(
        root, job, task, stop, &mut |_, _, _, _| Ok(()), encodings,
    )?;
    Ok(report)
}

pub(super) fn read_observed_with_encodings(
    root: &DiskRoot,
    job: &Job,
    task: &NativeTask,
    stop: &AtomicBool,
    observe: &mut impl FnMut(
        &crate::ValidatedConfig, &Candidate, &str, &AdmittedState,
    ) -> Result<()>,
    encodings: &mut Transaction<'_>,
) -> Result<(BackupNativeControlReport, AdmittedState)> {
    read_observed_at(
        root,
        job,
        ExpectedArchiveDigest(&task.archive_hash),
        stop,
        observe,
        Some(encodings),
    )
}

fn read_observed_at(
    root: &DiskRoot,
    job: &Job,
    expected: ExpectedArchiveDigest<'_>,
    stop: &AtomicBool,
    observe: &mut impl FnMut(
        &crate::ValidatedConfig, &Candidate, &str, &AdmittedState,
    ) -> Result<()>,
    encodings: Option<&mut Transaction<'_>>,
) -> Result<(BackupNativeControlReport, AdmittedState)> {
    let bytes = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST)?;
    let manifest: Manifest = decode_proof(&bytes)?;
    if object_hash(FORMAT, &manifest)? != expected.0 {
        return Err(invalid("native archive differs from the immutable task"));
    }
    let mut source = Target {
        root,
        stop,
        budget: Budget::default(),
        encodings,
    };
    verify_observed(&manifest, &bytes, job, &mut source, observe)
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/backup_native_format_tests.rs"]
mod format_tests;

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/backup_native_decode_tests.rs"]
mod decode_tests;

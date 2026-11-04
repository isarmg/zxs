//! Read-only control-evidence closure inspection, not an installed archive or
//! target-member readiness. Historical votes always keep their original owner.
use super::pending::activation::{AUTHORITY_KIND, PortableAuthority};
use super::*;
use crate::publication_evidence::{verified_graph_object_at, verified_publication_seal_at};
use std::{
    collections::BTreeSet,
    io::{BufReader, Read},
};

/// Successful inspection covers old journal/control metadata only. It does not
/// cover version-local proofs, payloads, tombstones or retained workspace bytes.
/// No new disk is admitted and no authority transfers on this result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipArchiveInspection {
    pub transition_hash: String,
    pub original_members: usize,
    pub published_candidates: usize,
    pub journal_frames: u64,
    pub journal_bytes: u64,
    pub immutable_metadata_objects: usize,
    pub immutable_metadata_bytes: u64,
    pub journals: Vec<MembershipJournalInspection>,
}

/// Original identity and exact read bytes, never a receipt for a target disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct MembershipJournalInspection {
    pub disk_id: String,
    pub slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub binding_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub bytes: u64,
    pub tail_frame_hash: String,
    pub content_hash: String,
}

/// Only exact hashes in the authenticated original publication chain resolve.
/// A sequence comparison alone never chooses a historical membership.
struct VerifiedFrozenPrefix<'a> {
    epoch: &'a FrozenEpoch<'a>,
    publications: &'a BTreeMap<u64, String>,
}
impl VerifiedFrozenPrefix<'_> {
    fn verify_candidate(&self, candidate: &Candidate) -> Result<()> {
        let hash = object_hash("candidate", candidate)?;
        if self
            .publications
            .get(&candidate.sequence)
            .map(String::as_str)
            != Some(hash.as_str())
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "candidate is outside the exact frozen publication prefix",
            ));
        }
        self.epoch.verify_candidate(candidate)
    }
}

struct FrozenEpoch<'a> {
    config: &'a ValidatedConfig,
    genesis: ValidatedConfig,
    authority_hash: Option<String>,
    previous_snapshot: Option<String>,
    first_sequence: u64,
    activation: Option<(PortableAuthority, std::sync::Arc<State>)>,
}
impl<'a> FrozenEpoch<'a> {
    fn read(intent: &'a TransitionIntent, source: &mut impl ControlSource) -> Result<Self> {
        let config = intent.origin.config();
        match intent.origin.predecessor() {
            None => Ok(Self {
                config,
                genesis: config.clone(),
                authority_hash: None,
                previous_snapshot: None,
                first_sequence: 0,
                activation: None,
            }),
            Some(hash) => {
                let predecessor = PortableAuthority::load(hash, |kind, hash| {
                    source.predecessor_object(kind, hash)
                })?;
                let previous = predecessor.intent()?;
                if !pending::activation::authority_config_matches(config, &previous.target)
                    || intent
                        .boundary
                        .as_ref()
                        .is_none_or(|boundary| boundary.sequence <= previous.fence_sequence)
                {
                    return Err(Error::new(
                        ErrorCode::LogFork,
                        "frozen successor has no completed predecessor",
                    ));
                }
                let archive = predecessor
                    .archives()?
                    .pop()
                    .expect("invariant: predecessor archive")
                    .0;
                let base = persistent::frozen_state_with(&archive, &previous, |kind, hash| {
                    source.predecessor_object(kind, hash)
                })?;
                Ok(Self {
                    config,
                    genesis: predecessor.genesis_config()?,
                    authority_hash: Some(hash.to_owned()),
                    previous_snapshot: previous.boundary.map(|boundary| boundary.snapshot_hash),
                    first_sequence: previous.fence_sequence.checked_add(1).ok_or_else(|| {
                        Error::new(ErrorCode::ResourceLimit, "frozen epoch sequence exhausted")
                    })?,
                    activation: Some((predecessor, std::sync::Arc::new(base))),
                })
            }
        }
    }
    fn verify_candidate(&self, candidate: &Candidate) -> Result<()> {
        if candidate.generation() != self.authority_hash.as_deref()
            || candidate.sequence < self.first_sequence
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "candidate belongs to another frozen epoch",
            ));
        }
        self.config.verify_original_candidate_membership(candidate)
    }
    fn verify_activation(&self, candidate: &Candidate) -> Result<()> {
        if candidate.sequence == self.first_sequence
            && let Some((portable, base)) = &self.activation
        {
            pending::activation::verify_frozen_activation(
                portable,
                std::sync::Arc::clone(base),
                candidate,
            )?;
        }
        Ok(())
    }
}

/// The two consumers share semantic admission; archive reads have no fallback.
trait ControlSource {
    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject>;
    fn predecessor_object(
        &mut self,
        kind: &'static str,
        hash: &str,
    ) -> Result<AuthenticatedMetadataObject> {
        self.object(kind, hash)
    }
    fn seal(&mut self, candidate: &Candidate, publication: &Publication)
    -> Result<PublicationSeal>;
    fn journal(&self, disk: &DiskConfig) -> Result<ReadFile>;
    fn observe(&mut self, bytes: usize) -> Result<()>;
}
struct LiveSource<'a> {
    engine: &'a Engine,
    members: Vec<DiskConfig>,
    destination: Option<&'a persistent::ArchiveStore>,
    seals: BTreeMap<String, String>,
    copied: BTreeSet<(&'static str, String)>,
}
impl ControlSource for LiveSource<'_> {
    fn observe(&mut self, _bytes: usize) -> Result<()> {
        Ok(())
    }

    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        let object = if kind == AUTHORITY_KIND {
            let authority = self.engine.config.generation().ok_or_else(|| {
                Error::new(
                    ErrorCode::ObjectCorrupt,
                    "Genesis has no predecessor authority",
                )
            })?;
            let object = authority
                .portable
                .node_objects()?
                .into_iter()
                .find(|object| object.hash == hash)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::ObjectCorrupt,
                        "authority node is outside admitted lineage",
                    )
                })?;
            AuthenticatedMetadataObject::authenticate(kind, hash, decode_strict(object.bytes())?)?
        } else {
            verified_graph_object_at(&self.engine.disk_resources, &self.members, kind, hash)?
        };
        if let Some(destination) = self.destination
            && self
                .copied
                .insert((object.kind(), object.hash().to_owned()))
        {
            destination.object_bytes(object.kind(), object.hash(), object.bytes())?;
        }
        Ok(object)
    }
    fn predecessor_object(
        &mut self,
        kind: &'static str,
        hash: &str,
    ) -> Result<AuthenticatedMetadataObject> {
        if kind == AUTHORITY_KIND {
            return self.object(kind, hash);
        }
        let object = persistent::predecessor_object(self.engine, kind, hash)?;
        if let Some(destination) = self.destination
            && self
                .copied
                .insert((object.kind(), object.hash().to_owned()))
        {
            destination.object_bytes(object.kind(), object.hash(), object.bytes())?;
        }
        Ok(object)
    }
    fn seal(
        &mut self,
        candidate: &Candidate,
        publication: &Publication,
    ) -> Result<PublicationSeal> {
        let seal = verified_publication_seal_at(
            &self.engine.disk_resources,
            &self.members,
            candidate,
            publication,
        )?;
        if let Some(destination) = self.destination
            && !self.seals.contains_key(&seal.candidate_hash)
        {
            let prepared = PreparedMetadataObject::new(persistent::SEAL_KIND, &seal, MAX_METADATA)?;
            destination.object_bytes(prepared.kind(), &prepared.hash, prepared.bytes())?;
            self.seals
                .insert(seal.candidate_hash.clone(), prepared.hash);
        }
        Ok(seal)
    }
    fn journal(&self, disk: &DiskConfig) -> Result<ReadFile> {
        self.engine
            .disk_resources
            .get(disk)?
            .root
            .open_read_regular(&pending::activation::log_path(&self.engine.config, disk))
    }
}
struct Inspection<'a, S> {
    epoch: &'a FrozenEpoch<'a>,
    source: &'a mut S,
    members: Vec<DiskConfig>,
    objects: BTreeSet<(&'static str, String)>,
    candidates: BTreeSet<String>,
    metadata_bytes: u64,
}
impl<S: ControlSource> Inspection<'_, S> {
    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        let object = self.source.object(kind, hash)?;
        self.remember(&object)?;
        Ok(object)
    }
    fn remember(&mut self, object: &AuthenticatedMetadataObject) -> Result<()> {
        if self
            .objects
            .insert((object.kind(), object.hash().to_owned()))
        {
            self.metadata_bytes = self
                .metadata_bytes
                .checked_add(object.bytes().len() as u64)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::ResourceLimit,
                        "archive metadata byte count overflow",
                    )
                })?;
        }
        Ok(())
    }
    fn candidate(&mut self, hash: &str) -> Result<Candidate> {
        let object = self.object("candidate", hash)?;
        let candidate: Candidate = json_codec::from_value(object.into_value())?;
        self.epoch.verify_candidate(&candidate)?;
        if self.candidates.insert(hash.to_owned()) {
            self.source.observe(0)?;
            self.epoch.verify_activation(&candidate)?;
            self.source.observe(0)?;
            let snapshot: Snapshot = json_codec::from_value(
                self.object("snapshot", &candidate.snapshot_hash)?
                    .into_value(),
            )?;
            if snapshot.sequence != candidate.sequence {
                return Err(Error::new(
                    ErrorCode::ObjectCorrupt,
                    "archive snapshot sequence differs from candidate",
                ));
            }
            let admitted =
                resolve_admitted_snapshot(snapshot, |kind, hash| self.object(kind, hash))?;
            crate::history_package::bound_history_objects_with(
                &candidate,
                Some(&admitted),
                |kind, hash| self.object(kind, hash),
            )?;
        }
        Ok(candidate)
    }
    fn decision_reference(&mut self, record: &Record) -> Result<()> {
        let candidate = self.candidate(&record.candidate_hash)?;
        if candidate.sequence != record.sequence {
            return Err(Error::new(
                ErrorCode::LogFork,
                "archive decision sequence differs",
            ));
        }
        let valid = match record.record_type {
            RecordType::Prepare => record.object_hash == candidate.snapshot_hash,
            RecordType::Accept => record.object_hash == record.candidate_hash,
            RecordType::Finalize => {
                let finalize: Finalize = json_codec::from_value(
                    self.object("finalize", &record.object_hash)?.into_value(),
                )?;
                let mut seen = BTreeSet::new();
                for accept in &finalize.accepts {
                    if !seen.insert(accept.disk_id.as_str())
                        || !self.members.iter().any(|member| {
                            member.disk_id == accept.disk_id && member.slot_id == accept.slot_id
                        })
                        || !receipt_binds(
                            accept,
                            RecordType::Accept,
                            candidate.sequence,
                            &record.candidate_hash,
                            &record.candidate_hash,
                        )?
                    {
                        return Err(Error::new(
                            ErrorCode::PublicationInvalid,
                            "archived finalize contains invalid original ACCEPT receipts",
                        ));
                    }
                }
                finalize.candidate_hash == record.candidate_hash
                    && if finalize.recovery_id.is_none() {
                        seen.len() == self.members.len()
                    } else {
                        seen.len() >= self.epoch.config.metadata_quorum()
                    }
            }
            RecordType::Reprotect => {
                let plan: crate::RecoveryPlan = json_codec::from_value(
                    self.object("recovery-plan", &record.object_hash)?
                        .into_value(),
                )?;
                plan.validate_for_candidate(&candidate)?;
                plan.candidate_hash == record.candidate_hash
            }
            RecordType::MembershipControlBootstrap | RecordType::MembershipJointAccept => {
                return Err(Error::new(
                    ErrorCode::LogCorrupt,
                    "pending bootstrap is not original control evidence",
                ));
            }
            RecordType::Publication | RecordType::MembershipFence => unreachable!(
                "invariant: decision reference dispatch excludes publication and membership fence"
            ),
        };
        if !valid {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "archive decision object does not bind its candidate",
            ));
        }
        Ok(())
    }
    fn publication(
        &mut self,
        disk: &DiskConfig,
        record: &Record,
        frame_hash: &str,
    ) -> Result<Candidate> {
        let candidate = self.candidate(&record.candidate_hash)?;
        if candidate.sequence != record.sequence {
            return Err(Error::new(
                ErrorCode::LogFork,
                "archived publication sequence differs",
            ));
        }
        let publication: Publication = json_codec::from_value(
            self.object("publication", &record.object_hash)?
                .into_value(),
        )?;
        let seal = self.source.seal(&candidate, &publication)?;
        let local = seal
            .receipts
            .iter()
            .find(|receipt| receipt.disk_id == disk.disk_id && receipt.slot_id == disk.slot_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::PublicationInvalid,
                    "archive publication omits its original member",
                )
            })?;
        if local.frame_hash != frame_hash {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "archive publication receipt differs from original journal",
            ));
        }
        // The independently retained FINALIZE remains part of recovery closure.
        let _ = self.object("finalize", &object_hash("finalize", &publication.finalize)?)?;
        Ok(candidate)
    }
}

struct HashingReader<'a, R> {
    reader: R,
    digest: &'a mut blake3::Hasher,
}
impl<R: Read> Read for HashingReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let count = self.reader.read(bytes)?;
        self.digest.update(&bytes[..count]);
        Ok(count)
    }
}

impl Engine {
    /// Inspect every original member's exact journal and all referenced candidate
    /// snapshot/history graphs, without repairing or writing any source bytes.
    /// This is a preflight for a future archive, not a completed copy or bootstrap.
    pub fn inspect_membership_metadata_archive(&self) -> Result<MembershipArchiveInspection> {
        let fence = self.membership_fence.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "archive inspection requires a fixed membership handoff",
            )
        })?;
        let members = self.members();
        self.validate_fence_members(&members)?;
        if fence.receipts.len() != members.len()
            || self.journals.values().any(JournalWriter::needs_recovery)
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "archive inspection requires every original member's complete fence",
            ));
        }
        let mut source = LiveSource {
            engine: self,
            members: self.members(),
            destination: None,
            seals: BTreeMap::new(),
            copied: BTreeSet::new(),
        };
        inspect_control(&self.config, fence, &mut source)
    }
}

fn inspect_control(
    config: &ValidatedConfig,
    fence: &AdmittedFence,
    source: &mut impl ControlSource,
) -> Result<MembershipArchiveInspection> {
    pending::reservation::verify_intent_roots(&fence.intent)?;
    let epoch = FrozenEpoch::read(&fence.intent, source)?;
    let mut members = config.metadata_members().cloned().collect::<Vec<_>>();
    members.sort_by(|left, right| left.disk_id.cmp(&right.disk_id));
    let mut inspection = Inspection {
        epoch: &epoch,
        source,
        members: members.clone(),
        objects: BTreeSet::new(),
        candidates: BTreeSet::new(),
        metadata_bytes: 0,
    };
    let mut prefix: Option<BTreeMap<u64, String>> = None;
    let mut journals = Vec::new();
    let mut journal_frames = 0u64;
    let mut journal_bytes = 0u64;
    for disk in &members {
        let mut digest = blake3::Hasher::new();
        let source = inspection.source.journal(disk)?;
        let mut reader = JournalFrameReader::new(HashingReader {
            reader: BufReader::new(source),
            digest: &mut digest,
        });
        let mut publications = BTreeMap::new();
        let mut previous_snapshot = epoch.previous_snapshot.clone();
        let mut last = None;
        loop {
            inspection.source.observe(0)?;
            let Some((record, frame_hash)) =
                reader.next_record_observed(&mut |bytes| inspection.source.observe(bytes))?
            else {
                break;
            };
            if last.as_ref().is_some_and(|(record, _): &(Record, String)| {
                record.record_type == RecordType::MembershipFence
            }) {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "records follow the archived membership fence",
                ));
            }
            validate_frozen_record(&fence.intent, &record)?;
            journal_frames = journal_frames.checked_add(1).ok_or_else(|| {
                Error::new(ErrorCode::ResourceLimit, "archive frame count overflow")
            })?;
            match record.record_type {
                RecordType::Prepare
                | RecordType::Accept
                | RecordType::Finalize
                | RecordType::Reprotect => {
                    inspection.decision_reference(&record)?;
                }
                RecordType::Publication => {
                    let candidate = inspection.publication(disk, &record, &frame_hash)?;
                    if candidate.previous_snapshot != previous_snapshot
                        || publications
                            .insert(candidate.sequence, record.candidate_hash.clone())
                            .is_some()
                    {
                        return Err(Error::new(
                            ErrorCode::LogFork,
                            "archive publication chain differs",
                        ));
                    }
                    if let Some(prefix) = &prefix {
                        VerifiedFrozenPrefix {
                            epoch: &epoch,
                            publications: prefix,
                        }
                        .verify_candidate(&candidate)?;
                    }
                    previous_snapshot = Some(candidate.snapshot_hash.clone());
                }
                RecordType::MembershipControlBootstrap | RecordType::MembershipJointAccept => {
                    return Err(Error::new(
                        ErrorCode::LogCorrupt,
                        "pending bootstrap is not an original journal record",
                    ));
                }
                RecordType::MembershipFence => {
                    if record.object_hash != fence.hash
                        || record.candidate_hash != fence.hash
                        || record.sequence != fence.intent.fence_sequence
                    {
                        return Err(Error::new(
                            ErrorCode::LogFork,
                            "archive contains a different membership fence",
                        ));
                    }
                    inspection.object(INTENT_KIND, &record.object_hash)?;
                }
            }
            last = Some((record, frame_hash));
        }
        if reader.torn_tail {
            return Err(Error::new(
                ErrorCode::LogCorrupt,
                "archive journal has a torn tail",
            ));
        }
        journal_bytes = journal_bytes
            .checked_add(reader.valid_bytes)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "archive journal byte count overflow",
                )
            })?;
        let bytes = reader.valid_bytes;
        drop(reader);
        let content_hash = digest.finalize().to_hex().to_string();
        let expected = &fence.receipts[&disk.disk_id];
        if !receipt_binds(
            expected,
            RecordType::MembershipFence,
            fence.intent.fence_sequence,
            &fence.hash,
            &fence.hash,
        )? || expected.disk_id != disk.disk_id
            || expected.slot_id != disk.slot_id
            || last.as_ref().map(|(_, hash)| hash.as_str()) != Some(expected.frame_hash.as_str())
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "archive journal differs from its admitted original fence",
            ));
        }
        journals.push(MembershipJournalInspection {
            disk_id: disk.disk_id.clone(),
            slot_id: disk.slot_id.clone(),
            binding_generation: disk.binding_generation,
            bytes,
            tail_frame_hash: expected.frame_hash.clone(),
            content_hash,
        });
        let expected_boundary = fence.intent.boundary.as_ref();
        if (epoch.activation.is_some() && !publications.contains_key(&epoch.first_sequence))
            || publications
                .last_key_value()
                .map(|(sequence, hash)| (*sequence, hash.as_str()))
                != expected_boundary.map(|base| (base.sequence, base.candidate_hash.as_str()))
            || previous_snapshot.as_deref()
                != expected_boundary.map(|base| base.snapshot_hash.as_str())
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "archive differs from the exact frozen prefix",
            ));
        }
        match &prefix {
            Some(prefix) if prefix != &publications => {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "original members disagree about the archived publication prefix",
                ));
            }
            Some(_) => {}
            None => prefix = Some(publications),
        }
    }
    Ok(MembershipArchiveInspection {
        transition_hash: fence.hash.clone(),
        original_members: members.len(),
        published_candidates: prefix
            .expect("invariant: original membership is nonempty")
            .len(),
        journal_frames,
        journal_bytes,
        immutable_metadata_objects: inspection.objects.len(),
        immutable_metadata_bytes: inspection.metadata_bytes,
        journals,
    })
}

fn validate_frozen_record(intent: &TransitionIntent, record: &Record) -> Result<()> {
    if record.record_type == RecordType::MembershipFence {
        return Ok(());
    }
    let sequence = intent.boundary.as_ref().map_or(0, |base| base.sequence);
    if record.sequence >= intent.fence_sequence
        || (record.sequence > sequence && record.record_type != RecordType::Prepare)
        || (record.sequence == sequence
            && intent
                .boundary
                .as_ref()
                .is_some_and(|base| record.candidate_hash != base.candidate_hash))
    {
        return Err(Error::new(
            ErrorCode::LogFork,
            "archive decision conflicts with frozen prefix",
        ));
    }
    Ok(())
}

pub(super) mod persistent;
pub(super) use persistent::PendingControlArchive;
pub use persistent::{MembershipControlArchive, verify_membership_control_archive};

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/membership_archive_tests.rs"]
mod tests;

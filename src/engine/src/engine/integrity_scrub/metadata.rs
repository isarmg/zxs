//! Per-member hash/shape/closure verification. No replica fallback or repair.
use super::*;
use crate::history_package::decode_history_root;
use crate::metadata_map::{PageShape, Reference, ReferenceKind, decode_index};
use std::collections::{BTreeMap, BTreeSet};
use std::io::BufReader;

// Cache only fixed-size addresses and bounded structural summaries, never State,
// complete JSON objects or one descriptor per historical logical record. When
// this bound is exhausted the result is explicitly incomplete, not healthy.
const MAX_VERIFIED_OBJECTS: usize = 262_144;
const MAX_JOURNAL_FRAMES: usize = 4_194_304;

/// Missing local proof is an availability failure only when an admitted,
/// fully verified degraded publication explicitly omitted this required copy.
#[derive(Default)]
pub(super) struct AcknowledgedUnavailable {
    versions: BTreeMap<String, BTreeSet<String>>,
    copies: usize,
}

impl AcknowledgedUnavailable {
    pub(super) fn contains(&self, version: &str, slot: &str) -> bool {
        self.versions
            .get(version)
            .is_some_and(|slots| slots.contains(slot))
    }

    fn record(&mut self, version: &str, slot: &str, unavailable: bool) -> Result<()> {
        if unavailable {
            if self.contains(version, slot) {
                return Ok(());
            }
            if self.copies == MAX_VERIFIED_OBJECTS {
                return Err(Error::new(
                    ErrorCode::ResourceLimit,
                    "integrity scrub acknowledged-loss budget exhausted",
                ));
            }
            self.versions
                .entry(version.to_owned())
                .or_default()
                .insert(slot.to_owned());
            self.copies += 1;
        } else if let Some(slots) = self.versions.get_mut(version) {
            if slots.remove(slot) {
                self.copies -= 1;
            }
            if slots.is_empty() {
                self.versions.remove(version);
            }
        }
        Ok(())
    }
}

enum Shape {
    Page(PageShape),
    Index(String),
    History(String),
    Snapshot(u64),
}

struct MemberScan<'a> {
    engine: &'a Engine,
    disk: &'a DiskConfig,
    cancellation: &'a IntegrityScrubCancellation,
    report: &'a mut IntegrityScrubReport,
    verified: BTreeMap<(&'static str, String), Shape>,
    loaded_objects: usize,
    unavailable: AcknowledgedUnavailable,
}

pub(super) fn verify_member(
    engine: &Engine,
    disk: &DiskConfig,
    cancellation: &IntegrityScrubCancellation,
    report: &mut IntegrityScrubReport,
) -> Result<AcknowledgedUnavailable> {
    verify_saved_membership(engine, disk)?;
    MemberScan {
        engine,
        disk,
        cancellation,
        report,
        verified: BTreeMap::new(),
        loaded_objects: 0,
        unavailable: AcknowledgedUnavailable::default(),
    }
    .journal()
}

impl MemberScan<'_> {
    fn journal(&mut self) -> Result<AcknowledgedUnavailable> {
        // Hash-linked receipts are not signatures. Bind the observed chain to
        // this Engine's admitted writer tail, not a newly self-consistent disk.
        let writer = self.engine.journals.get(&self.disk.disk_id).ok_or_else(|| {
            Error::new(ErrorCode::RecoveryRequired, "metadata member has no admitted writer anchor; recover it before certifying its retained closure")
        })?;
        let expected_tail = writer.records().last().map(|(_, hash)| hash.clone());
        let mut reader =
            JournalFrameReader::new(BufReader::new(open_regular(&log_path(self.disk))?));
        let mut previous_snapshot = None;
        let mut published_sequence = 0;
        let mut last_frame = None;
        let mut frames = 0;
        loop {
            self.cancellation.checkpoint()?;
            let Some((record, frame_hash)) = reader.next_record()? else {
                break;
            };
            if frames == MAX_JOURNAL_FRAMES {
                return Err(Error::new(
                    ErrorCode::ResourceLimit,
                    "integrity scrub journal frame budget exhausted",
                ));
            }
            frames += 1;
            self.report.journal_frames_verified += 1;
            if record.sequence > self.engine.state.sequence {
                self.report.unpublished_journal_records += 1;
            } else if record.record_type == RecordType::Publication {
                if record.sequence <= published_sequence {
                    return Err(Error::new(
                        ErrorCode::LogFork,
                        "published sequences are not increasing",
                    ));
                }
                let candidate = self.publication(&record, &frame_hash)?;
                if candidate.previous_snapshot != previous_snapshot {
                    return Err(Error::new(
                        ErrorCode::LogFork,
                        "published candidate does not extend the retained snapshot chain",
                    ));
                }
                previous_snapshot = Some(candidate.snapshot_hash);
                published_sequence = record.sequence;
            }
            last_frame = Some(frame_hash);
        }
        if reader.torn_tail {
            return Err(Error::new(
                ErrorCode::LogCorrupt,
                "journal contains a torn tail",
            ));
        }
        if previous_snapshot != self.engine.snapshot
            || published_sequence != self.engine.state.sequence
        {
            return Err(Error::new(
                ErrorCode::LogCorrupt,
                "journal publication prefix differs from the admitted snapshot",
            ));
        }
        if last_frame != expected_tail {
            return Err(Error::new(
                ErrorCode::LogCorrupt,
                "journal tail differs from the admitted writer",
            ));
        }
        Ok(std::mem::take(&mut self.unavailable))
    }

    fn publication(&mut self, record: &Record, frame_hash: &str) -> Result<Candidate> {
        let candidate: Candidate = self.object("candidate", &record.candidate_hash)?;
        self.engine.verify_candidate_membership(&candidate)?;
        if candidate.pool_id != self.engine.config.pool_id || candidate.sequence != record.sequence
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "candidate pool or sequence differs from its journal",
            ));
        }
        let publication: Publication = self.object("publication", &record.object_hash)?;
        let seal: PublicationSeal =
            read_json(&publication_seal_path(self.disk, &record.candidate_hash))?;
        verify_publication_seal(&candidate, &publication, &seal)?;
        let local = seal
            .receipts
            .iter()
            .find(|receipt| {
                receipt.disk_id == self.disk.disk_id && receipt.slot_id == self.disk.slot_id
            })
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::PublicationInvalid,
                    "publication seal omits this member binding",
                )
            })?;
        if local.frame_hash != frame_hash {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "publication receipt differs from the actual journal frame",
            ));
        }
        // FINALIZE is a separately retained recovery object, even though the
        // publication also embeds its authenticated logical value.
        let finalize_hash = object_hash("finalize", &publication.finalize)?;
        let _: Finalize = self.object("finalize", &finalize_hash)?;
        self.snapshot(&candidate.snapshot_hash, candidate.sequence)?;
        for (id, hash) in &candidate.history_manifest_hashes {
            self.history(hash, id)?;
        }
        if candidate.sequence == self.engine.state.sequence
            && candidate
                .history_manifest_hashes
                .keys()
                .ne(self.engine.state.histories.keys())
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "current candidate history coverage differs from admitted State",
            ));
        }
        self.record_payload_expectations(&candidate, &publication)?;
        self.report.metadata_publications_verified += 1;
        Ok(candidate)
    }

    fn record_payload_expectations(
        &mut self,
        candidate: &Candidate,
        publication: &Publication,
    ) -> Result<()> {
        for evidence in &candidate.versions {
            let id = &evidence.version.version_id;
            // Only the exact currently admitted descriptor/route can consume a
            // historical exception. A later complete publication clears it.
            if self.engine.state.versions.get(id) != Some(&evidence.version) {
                continue;
            }
            let placement = &self.engine.state.placements[id];
            for slot in &evidence.placement.route_slot_ids {
                if !placement.route_slot_ids.contains(slot) {
                    continue;
                }
                let unavailable = publication.recovery.is_some()
                    && !publication
                        .payload_receipts
                        .iter()
                        .any(|receipt| receipt.version_id == *id && receipt.slot_id == *slot);
                self.unavailable.record(id, slot, unavailable)?;
            }
        }
        Ok(())
    }

    fn object<T: data_serialization::de::DeserializeOwned + data_serialization::Serialize>(
        &mut self,
        kind: &'static str,
        hash: &str,
    ) -> Result<T> {
        self.admit_object()?;
        let value = load_object(self.disk, kind, hash).map_err(|error| {
            error.context(
                ErrorCode::ObjectCorrupt,
                format!("{kind}/{hash} failed integrity scrub"),
            )
        })?;
        self.report.metadata_objects_verified += 1;
        Ok(value)
    }

    fn graph_object(
        &mut self,
        kind: &'static str,
        hash: &str,
    ) -> Result<AuthenticatedMetadataObject> {
        self.admit_object()?;
        let object = load_authenticated_object(self.disk, kind, hash).map_err(|error| {
            error.context(
                ErrorCode::ObjectCorrupt,
                format!("{kind}/{hash} failed integrity scrub"),
            )
        })?;
        self.report.metadata_objects_verified += 1;
        Ok(object)
    }

    fn admit_object(&mut self) -> Result<()> {
        self.cancellation.checkpoint()?;
        if self.loaded_objects == MAX_VERIFIED_OBJECTS {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "integrity scrub unique-object budget exhausted",
            ));
        }
        self.loaded_objects += 1;
        Ok(())
    }

    fn snapshot(&mut self, hash: &str, expected_sequence: u64) -> Result<()> {
        let address = ("snapshot", hash.to_owned());
        if let Some(Shape::Snapshot(sequence)) = self.verified.get(&address) {
            return same_sequence(*sequence, expected_sequence);
        }
        let snapshot: Snapshot = self.object("snapshot", hash)?;
        validate_snapshot_schema(&snapshot)?;
        same_sequence(snapshot.sequence, expected_sequence)?;
        for hashes in snapshot.pages.values() {
            for hash in hashes {
                self.page(hash)?;
            }
        }
        self.verified
            .insert(address, Shape::Snapshot(snapshot.sequence));
        Ok(())
    }

    fn page(&mut self, hash: &str) -> Result<&PageShape> {
        let address = ("state-page", hash.to_owned());
        if !self.verified.contains_key(&address) {
            let object = self.graph_object("state-page", hash)?;
            let summary = PageShape::read(&object)?;
            self.verified.insert(address.clone(), Shape::Page(summary));
        }
        let Some(Shape::Page(summary)) = self.verified.get(&address) else {
            unreachable!("invariant: state-page addresses contain page summaries");
        };
        Ok(summary)
    }

    fn history(&mut self, hash: &str, expected_id: &str) -> Result<()> {
        let address = ("history-manifest", hash.to_owned());
        if let Some(Shape::History(id)) = self.verified.get(&address) {
            return same_history(id, expected_id);
        }
        let root = decode_history_root(&self.graph_object("history-manifest", hash)?)?;
        validate_id(&root.history.history_tree_id, "history")?;
        validate_id(&root.repository.repository_id, "repo")?;
        same_history(&root.history.history_tree_id, expected_id)?;
        if root.history.repository_id != root.repository.repository_id
            || root.repository.history_tree_id != root.history.history_tree_id
        {
            return Err(Error::new(
                ErrorCode::HistoryPackageInvalid,
                "history root repository binding differs",
            ));
        }
        for reference in root.maps.values() {
            self.history_node(reference, "")?;
        }
        self.verified
            .insert(address, Shape::History(root.history.history_tree_id));
        Ok(())
    }

    fn history_node(&mut self, reference: &Reference, prefix: &str) -> Result<()> {
        self.cancellation.checkpoint()?;
        // References are parsed and validated by the one history graph decoder.
        match reference.kind() {
            ReferenceKind::Page => self.page(reference.hash())?.validate_at(prefix),
            ReferenceKind::Index => {
                let address = ("history-index", reference.hash().to_owned());
                if let Some(Shape::Index(found)) = self.verified.get(&address) {
                    return same_prefix(found, prefix);
                }
                let index = decode_index(&self.graph_object("history-index", reference.hash())?)?;
                same_prefix(&index.prefix, prefix)?;
                for (digit, child) in &index.children {
                    self.history_node(child, &format!("{prefix}{digit}"))?;
                }
                self.verified.insert(address, Shape::Index(index.prefix));
                Ok(())
            }
        }
    }
}

fn same_sequence(actual: u64, expected: u64) -> Result<()> {
    if actual != expected {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "snapshot sequence differs from candidate",
        ));
    }
    Ok(())
}

fn same_history(actual: &str, expected: &str) -> Result<()> {
    if actual != expected {
        return Err(Error::new(
            ErrorCode::HistoryPackageInvalid,
            "history root identity differs from candidate",
        ));
    }
    Ok(())
}

fn same_prefix(actual: &str, expected: &str) -> Result<()> {
    if actual != expected {
        return Err(Error::new(
            ErrorCode::HistoryPackageInvalid,
            "history graph radix prefix differs",
        ));
    }
    Ok(())
}

fn verify_saved_membership(engine: &Engine, disk: &DiskConfig) -> Result<()> {
    let saved = ValidatedConfig::try_from(read_json::<Config>(
        &disk.root.join("metadata/pool-membership/config.json"),
    )?)?;
    fn identity(member: &DiskConfig) -> (&str, &str, u64) {
        (&member.disk_id, &member.slot_id, member.binding_generation)
    }
    let expected: std::collections::BTreeSet<_> =
        engine.config.metadata_members().map(identity).collect();
    let actual: std::collections::BTreeSet<_> = saved.metadata_members().map(identity).collect();
    if saved.pool_id != engine.config.pool_id || actual != expected {
        return Err(Error::new(
            ErrorCode::PublicationInvalid,
            "saved metadata membership differs from admitted configuration",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/integrity_scrub/metadata_tests.rs"]
mod tests;

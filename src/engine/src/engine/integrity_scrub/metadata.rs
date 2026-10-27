//! Per-member hash/shape/closure verification. No replica fallback or repair.
use super::*;
use crate::history_package::decode_history_root;
use crate::metadata_map::{MetadataIndex, PageShape, Reference, ReferenceKind, decode_index};
use std::collections::{BTreeMap, BTreeSet};

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
    Page {
        summary: PageShape,
        catalog: Option<Vec<(String, String)>>,
    },
    Index {
        node: MetadataIndex,
        complete: bool,
    },
    ControlAuth,
    History(String),
    Snapshot(u64),
}

struct MemberScan<'a, O, P> {
    plan: &'a IntegrityScrubPlan,
    disk: &'a DiskConfig,
    observe: &'a mut O,
    progress: &'a mut P,
    report: &'a mut IntegrityScrubReport,
    verified: BTreeMap<(&'static str, String), Shape>,
    loaded_objects: usize,
    catalog_bindings: usize,
    unavailable: AcknowledgedUnavailable,
}

pub(super) fn verify_member(
    plan: &IntegrityScrubPlan,
    disk: &DiskConfig,
    observe: &mut impl FnMut(usize) -> Result<()>,
    progress: &mut impl FnMut(&IntegrityScrubReport),
    report: &mut IntegrityScrubReport,
) -> Result<(AcknowledgedUnavailable, bool)> {
    verify_saved_membership(plan, disk, observe)?;
    MemberScan {
        plan,
        disk,
        observe,
        progress,
        report,
        verified: BTreeMap::new(),
        loaded_objects: 0,
        catalog_bindings: 0,
        unavailable: AcknowledgedUnavailable::default(),
    }
    .journal()
}

impl<O: FnMut(usize) -> Result<()>, P: FnMut(&IntegrityScrubReport)> MemberScan<'_, O, P> {
    fn journal(&mut self) -> Result<(AcknowledgedUnavailable, bool)> {
        // Hash-linked receipts are not signatures. Bind the observed chain to
        // this Engine's admitted writer tail, not a newly self-consistent disk.
        let expected_tail = self.plan.tails.get(&self.disk.disk_id).ok_or_else(|| {
            Error::new(ErrorCode::RecoveryRequired, "metadata member has no admitted writer anchor; recover it before certifying its retained closure")
        })?;
        // No BufReader read-ahead: only the explicit one-byte probe may read beyond the captured frontier.
        let mut reader = JournalFrameReader::new(open_regular(&log_path(self.disk))?);
        let mut previous_snapshot = None;
        let mut published_sequence = 0;
        let mut last_frame = None;
        let mut frames = 0;
        loop {
            (self.observe)(0)?;
            if !self.plan.verify_eof && &last_frame == expected_tail {
                break;
            }
            let Some((record, frame_hash)) = reader.next_record_observed(self.observe)? else {
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
            if record.sequence > self.plan.state.sequence {
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
            (self.progress)(self.report);
        }
        if reader.torn_tail {
            return Err(Error::new(
                ErrorCode::LogCorrupt,
                "journal contains a torn tail",
            ));
        }
        if previous_snapshot != self.plan.snapshot || published_sequence != self.plan.state.sequence
        {
            return Err(Error::new(
                ErrorCode::LogCorrupt,
                "journal publication prefix differs from the admitted snapshot",
            ));
        }
        if &last_frame != expected_tail {
            return Err(Error::new(
                ErrorCode::LogCorrupt,
                "journal tail differs from the admitted writer",
            ));
        }
        let extra = !self.plan.verify_eof && reader.probe_extra_observed(self.observe)?;
        Ok((std::mem::take(&mut self.unavailable), extra))
    }

    fn publication(&mut self, record: &Record, frame_hash: &str) -> Result<Candidate> {
        let candidate: Candidate = self.object("candidate", &record.candidate_hash)?;
        self.plan.config.verify_candidate_membership(&candidate)?;
        if candidate.pool_id != self.plan.config.pool_id || candidate.sequence != record.sequence {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "candidate pool or sequence differs from its journal",
            ));
        }
        let publication: Publication = self.object("publication", &record.object_hash)?;
        let seal: PublicationSeal = read_json_observed(
            &publication_seal_path(self.disk, &record.candidate_hash),
            self.observe,
        )?;
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
        candidate.history_roots.validate()?;
        let current = candidate.sequence == self.plan.state.sequence;
        let histories = self.catalog_node(&candidate.history_roots, "", current)?;
        if current && histories != self.plan.state.histories.len() {
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
            if self.plan.state.versions.get(id) != Some(&evidence.version) {
                continue;
            }
            let placement = &self.plan.state.placements[id];
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
        let value = load_object_observed(self.disk, kind, hash, self.observe).map_err(|error| {
            if matches!(
                error.code(),
                ErrorCode::OperationAborted | ErrorCode::ResourceLimit
            ) {
                error
            } else {
                error.context(
                    ErrorCode::ObjectCorrupt,
                    format!("{kind}/{hash} failed integrity scrub"),
                )
            }
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
        let object = load_authenticated_object_observed(self.disk, kind, hash, self.observe)
            .map_err(|error| {
                if matches!(
                    error.code(),
                    ErrorCode::OperationAborted | ErrorCode::ResourceLimit
                ) {
                    error
                } else {
                    error.context(
                        ErrorCode::ObjectCorrupt,
                        format!("{kind}/{hash} failed integrity scrub"),
                    )
                }
            })?;
        self.report.metadata_objects_verified += 1;
        Ok(object)
    }

    fn admit_object(&mut self) -> Result<()> {
        (self.observe)(0)?;
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
        for reference in snapshot
            .maps
            .values()
            .chain(std::iter::once(&snapshot.events))
        {
            self.history_node(reference, "")?;
        }
        let auth = (
            "state-control-auth",
            snapshot.control_auth.hash().to_owned(),
        );
        if !self.verified.contains_key(&auth) {
            self.graph_object("state-control-auth", snapshot.control_auth.hash())?;
            self.verified.insert(auth, Shape::ControlAuth);
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
            // A page can be referenced in multiple roles (including the shared
            // empty leaf). Classify possible catalog bindings during its one
            // physical read; a non-catalog business map is not a parse failure.
            let catalog = object
                .value()
                .as_object()
                .expect("invariant: PageShape admits a map")
                .iter()
                .map(|(id, value)| {
                    Ok((
                        id.clone(),
                        crate::history_package::catalog_entry(id, value)?.to_owned(),
                    ))
                })
                .collect::<Result<Vec<_>>>()
                .ok();
            if let Some(entries) = &catalog {
                if entries.len() > MAX_VERIFIED_OBJECTS - self.catalog_bindings {
                    return Err(Error::new(
                        ErrorCode::ResourceLimit,
                        "integrity scrub catalog binding budget exhausted",
                    ));
                }
                self.catalog_bindings += entries.len();
            }
            self.verified
                .insert(address.clone(), Shape::Page { summary, catalog });
        }
        let Some(Shape::Page { summary, .. }) = self.verified.get(&address) else {
            unreachable!("invariant: state-page addresses contain page summaries")
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

    fn index(&mut self, reference: &Reference, prefix: &str) -> Result<(MetadataIndex, bool)> {
        let address = ("metadata-index", reference.hash().to_owned());
        if let Some(Shape::Index { node, complete }) = self.verified.get(&address) {
            same_prefix(&node.prefix, prefix)?;
            return Ok((node.clone(), *complete));
        }
        let node = decode_index(&self.graph_object("metadata-index", reference.hash())?)?;
        same_prefix(&node.prefix, prefix)?;
        self.verified.insert(
            address,
            Shape::Index {
                node: node.clone(),
                complete: false,
            },
        );
        Ok((node, false))
    }
    fn complete_index(&mut self, hash: &str) {
        let Some(Shape::Index { complete, .. }) =
            self.verified.get_mut(&("metadata-index", hash.to_owned()))
        else {
            unreachable!("invariant: traversed index has a decoded cache entry")
        };
        *complete = true;
    }
    fn history_node(&mut self, reference: &Reference, prefix: &str) -> Result<()> {
        (self.observe)(0)?;
        match reference.kind() {
            ReferenceKind::Page => self.page(reference.hash())?.validate_at(prefix),
            ReferenceKind::Index => {
                let (index, complete) = self.index(reference, prefix)?;
                if !complete {
                    for (digit, child) in index.children {
                        self.history_node(&child, &format!("{prefix}{digit}"))?;
                    }
                    self.complete_index(reference.hash());
                }
                Ok(())
            }
        }
    }
    fn catalog_node(
        &mut self,
        reference: &Reference,
        prefix: &str,
        current: bool,
    ) -> Result<usize> {
        (self.observe)(0)?;
        match reference.kind() {
            ReferenceKind::Page => {
                let address = ("state-page", reference.hash().to_owned());
                self.page(reference.hash())?.validate_at(prefix)?;
                let Some(Shape::Page { catalog, .. }) = self.verified.get(&address) else {
                    unreachable!("invariant: catalog page was checked")
                };
                let entries = catalog.as_ref().ok_or_else(|| {
                    Error::new(
                        ErrorCode::ObjectCorrupt,
                        "referenced state-page is not a valid history catalog",
                    )
                })?;
                // At most 128 typed bindings; no complete manifest/State cache.
                let entries = entries.clone();
                for (id, hash) in &entries {
                    if current && !self.plan.state.histories.contains_key(id) {
                        return Err(Error::new(
                            ErrorCode::ObjectCorrupt,
                            "current catalog contains an unadmitted history",
                        ));
                    }
                    self.history(hash, id)?;
                }
                Ok(entries.len())
            }
            ReferenceKind::Index => {
                let (index, _) = self.index(reference, prefix)?;
                let mut count = 0usize;
                for (digit, child) in index.children {
                    count = count
                        .checked_add(self.catalog_node(
                            &child,
                            &format!("{prefix}{digit}"),
                            current,
                        )?)
                        .ok_or_else(|| {
                            Error::new(ErrorCode::ResourceLimit, "history catalog count overflow")
                        })?;
                }
                self.complete_index(reference.hash());
                Ok(count)
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

fn verify_saved_membership(
    plan: &IntegrityScrubPlan,
    disk: &DiskConfig,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    let saved = ValidatedConfig::try_from(read_json_observed::<Config>(
        &disk.root.join("metadata/pool-membership/config.json"),
        observe,
    )?)?;
    plan.config.verify_saved_metadata_membership(&saved)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/integrity_scrub/metadata_tests.rs"]
mod tests;

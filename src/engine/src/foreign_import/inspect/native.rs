//! Stream the complete original fixed-genesis published State prefix from one
//! held source. This is local source evidence, not a new pool admission.
use super::*;
use crate::history_package::{AdmittedState, bound_history_objects_with};

const MAX_PUBLICATIONS: usize = 2048;

impl ForeignImportPreview {
    pub(crate) fn visit_original_states(
        &self,
        mut visit: impl FnMut(&ValidatedConfig, &Candidate, &str, &AdmittedState) -> Result<()>,
    ) -> Result<(ForeignOriginalStateReport, AdmittedState)> {
        self.revalidate_source()?;
        if self.checkpoint.authority_hash.is_some() || self.checkpoint.unpublished_tail {
            return Err(invalid(
                "foreign database preparation requires a stable complete fixed-genesis prefix",
            ));
        }
        let mut budget = Budget {
            remaining: self.limits.metadata_bytes,
        };
        let opened = open_source(
            self.root.directory(self.root.path())?,
            self.root.path().into(),
            &mut budget,
        )?;
        if opened.config.generation().is_some() || opened.config.disks.len() > 32 {
            return Err(invalid(
                "foreign original scopes require at most 32 fixed original disk bindings",
            ));
        }
        let bytes = opened
            .root
            .read_bytes(&opened.journal_path, self.limits.journal_bytes)?;
        let scan =
            scan_frames_observed(bytes.as_slice(), JournalNamespace::Active, &mut |_| Ok(()))?;
        if scan.torn_tail
            || ForeignEvidenceDigest::from_digest(blake3::hash(&bytes)) != self.journal_digest
        {
            return Err(invalid("original source journal changed or is torn"));
        }
        let mut evidence = Evidence {
            root: &opened.root,
            disk: opened.disk,
            budget: &mut budget,
            anchors: opened.config_anchors,
        };
        let mut previous: Option<AdmittedState> = None;
        let mut snapshot = None;
        let mut prefix = Vec::new();
        let mut through = None;
        for (record, frame) in &scan.records {
            if !matches!(
                record.record_type,
                RecordType::Prepare
                    | RecordType::Accept
                    | RecordType::Finalize
                    | RecordType::Publication
            ) {
                return Err(invalid(
                    "source contains unsupported membership/reprotection history; original evidence was preserved",
                ));
            }
            if record.record_type != RecordType::Publication {
                continue;
            }
            if prefix.len() == MAX_PUBLICATIONS {
                return Err(Error::new(
                    ErrorCode::ResourceLimit,
                    "foreign native prefix exceeds 2048 publications",
                ));
            }
            let (candidate, publication) = evidence.publication(record, frame, &opened.seal)?;
            opened
                .config
                .verify_original_candidate_membership(&candidate)?;
            if candidate.generation().is_some()
                || candidate.registry().is_some()
                || publication.recovery.is_some()
                || candidate.sequence != prefix.len() as u64 + 1
                || candidate.previous_snapshot != snapshot
            {
                return Err(invalid(
                    "foreign original State does not extend the exact fixed-genesis publication chain",
                ));
            }
            local_receipts(
                &scan.records,
                &opened.seal,
                &candidate,
                &publication,
                record,
            )?;
            let finalize: crate::journal::Finalize =
                evidence.object("finalize", &object_hash("finalize", &publication.finalize)?)?;
            if canonical_json(&finalize)? != canonical_json(&publication.finalize)? {
                return Err(invalid(
                    "original FINALIZE object differs from publication evidence",
                ));
            }
            let descriptor: Snapshot = evidence.object("snapshot", &candidate.snapshot_hash)?;
            let admitted =
                resolve_admitted_snapshot(descriptor, |kind, hash| evidence.graph(kind, hash))?;
            let state = admitted.state();
            if state.sequence != candidate.sequence
                || state.operations.get(&candidate.operation_id) != Some(&publication.result)
            {
                return Err(invalid(
                    "original State and publication have different permanent receipts",
                ));
            }
            retained_prefix(
                previous.as_ref().map(AdmittedState::state),
                state,
                &candidate,
            )?;
            bound_history_objects_with(&candidate, Some(&admitted), |kind, hash| {
                evidence.graph(kind, hash)
            })?;
            // The observer may derive bounded in-memory images, never install
            // them before the complete prefix and unchanged anchors return.
            visit(
                &opened.config,
                &candidate,
                &record.candidate_hash,
                &admitted,
            )?;
            snapshot = Some(candidate.snapshot_hash.clone());
            prefix.push((record.candidate_hash.clone(), record.object_hash.clone()));
            through = Some((record.candidate_hash.clone(), record.object_hash.clone()));
            previous = Some(admitted);
        }
        let state = previous
            .ok_or_else(|| invalid("foreign source has no complete native State prefix"))?;
        if state.state().sequence != self.checkpoint.sequence
            || through.as_ref().map(|item| (&item.0, &item.1))
                != Some((
                    &self.checkpoint.candidate_hash,
                    &self.checkpoint.publication_hash,
                ))
        {
            return Err(invalid(
                "foreign original State prefix differs from the frozen source checkpoint",
            ));
        }
        evidence.unchanged()?;
        self.revalidate_source()?;
        let report = ForeignOriginalStateReport {
            pool_id: opened.config.pool_id.clone(),
            configuration_hash: object_hash("foreign-original-configuration", &opened.config)?,
            registrations: opened
                .config
                .disks
                .iter()
                .map(|disk| DiskRegistrationIdentity {
                    pool_id: opened.config.pool_id.clone(),
                    disk_id: disk.disk_id.clone(),
                    slot_id: disk.slot_id.clone(),
                    binding_generation: disk.binding_generation,
                })
                .collect(),
            sequence: state.state().sequence,
            snapshot_hash: snapshot.expect("invariant: admitted nonempty source prefix"),
            state_hash: object_hash("foreign-original-state", state.state())?,
            prefix_hash: object_hash("foreign-original-published-prefix", &prefix)?,
            publications: prefix.len(),
            operations: state.state().operations.len(),
            principals: state.state().operation_principals.len(),
            events: state.state().events.len(),
        };
        Ok((report, state))
    }
}

fn retained_prefix(previous: Option<&State>, next: &State, candidate: &Candidate) -> Result<()> {
    let actor = next
        .operation_principals
        .get(&candidate.operation_id)
        .ok_or_else(|| invalid("foreign native operation lacks its original principal"))?;
    Engine::require_actor(actor)?;
    let old_count = previous.map_or(0, |state| state.operations.len());
    if next.operations.len() != old_count + 1
        || next.operation_principals.len() != next.operations.len()
        || previous.is_some_and(|state| {
            state
                .operations
                .iter()
                .any(|(id, value)| next.operations.get(id) != Some(value))
                || state
                    .operation_principals
                    .iter()
                    .any(|(id, value)| next.operation_principals.get(id) != Some(value))
                || !next.events.starts_with(&state.events)
                || state.operations.contains_key(&candidate.operation_id)
        })
    {
        return Err(invalid(
            "foreign native prefix dropped or rewrote permanent operations/principals/events",
        ));
    }
    let start = previous.map_or(0, |state| state.events.len());
    if next.events.len() != start + 1
        || next.events[start].operation_id != candidate.operation_id
        || next.events[start].sequence != candidate.sequence
    {
        return Err(invalid(
            "foreign native event is not the original publication event",
        ));
    }
    Ok(())
}
fn local_receipts(
    records: &[(Record, String)],
    seal: &DiskSeal,
    candidate: &Candidate,
    publication: &Publication,
    published: &Record,
) -> Result<()> {
    let finalize = object_hash("finalize", &publication.finalize)?;
    // The frame reader already proves monotonically ordered sequences.
    let start = records.partition_point(|(record, _)| record.sequence < candidate.sequence);
    let end = records.partition_point(|(record, _)| record.sequence <= candidate.sequence);
    let local = &records[start..end];
    for kind in [
        RecordType::Prepare,
        RecordType::Accept,
        RecordType::Finalize,
        RecordType::Publication,
    ] {
        if !local.iter().any(|(record, _)| record.record_type == kind) {
            return Err(invalid("original local journal omits a publication phase"));
        }
    }
    for (record, _) in local {
        let expected = match record.record_type {
            RecordType::Prepare => &candidate.snapshot_hash,
            RecordType::Accept => &published.candidate_hash,
            RecordType::Finalize => &finalize,
            RecordType::Publication => &published.object_hash,
            _ => return Err(invalid("unsupported original journal record")),
        };
        if record.candidate_hash != published.candidate_hash || &record.object_hash != expected {
            return Err(invalid(
                "local original journal binds conflicting decisions",
            ));
        }
    }
    for receipt in publication
        .finalize
        .accepts
        .iter()
        .chain(&publication.finalize_receipts)
        .filter(|receipt| receipt.disk_id == seal.disk_id)
    {
        if receipt.slot_id != seal.slot_id
            || !local
                .iter()
                .any(|(record, hash)| record == &receipt.record && hash == &receipt.frame_hash)
        {
            return Err(invalid(
                "original local receipt is missing from its source journal",
            ));
        }
    }
    Ok(())
}

//! Stream authenticated snapshots. Keep only adjacent local projections, never
//! an all-history State cache. Incomplete/unsupported provenance is not a prefix.
use super::*;
use crate::{
    journal::{Candidate, Publication, RecordType},
    publication_evidence::*,
};

struct Source<'a> {
    candidate: &'a str,
    publication: &'a str,
}

fn sources(engine: &Engine, through: u64) -> Result<BTreeMap<u64, Source<'_>>> {
    let mut sources: BTreeMap<u64, Source<'_>> = BTreeMap::new();
    for writer in engine.journals.values() {
        for (record, _) in writer.records() {
            if record.record_type != RecordType::Publication || record.sequence > through {
                continue;
            }
            let source = Source {
                candidate: &record.candidate_hash,
                publication: &record.object_hash,
            };
            match sources.entry(record.sequence) {
                std::collections::btree_map::Entry::Occupied(old) => {
                    if old.get().candidate != source.candidate
                        || old.get().publication != source.publication
                    {
                        return Err(Error::new(
                            ErrorCode::LogFork,
                            "disk history publication sources conflict",
                        ));
                    }
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(source);
                }
            }
            if sources.len() > MAX_PUBLICATIONS {
                return Err(budget());
            }
        }
    }
    Ok(sources)
}

/// Visit the complete authenticated source prefix without installing a catalog.
/// The callback borrows each admitted State only for that one publication.
pub(in crate::engine) fn visit_published_prefix(
    engine: &Engine,
    through: u64,
    checkpoint: &impl Fn() -> Result<()>,
    mut visit: impl FnMut(
        &Candidate,
        &str,
        &Publication,
        &str,
        &crate::history_package::AdmittedState,
    ) -> Result<()>,
) -> Result<()> {
    checkpoint()?;
    if engine.config.generation().is_some() {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "disk history requires a complete genesis chain; membership archive replay is not implemented",
        ));
    }
    if through > engine.state.sequence {
        return Err(invalid(
            "disk history claims a publication beyond the admitted head",
        ));
    }
    let sources = sources(engine, through)?;
    let members = engine.members();
    let mut previous_snapshot = None;
    let mut previous_sequence = 0u64;
    for (sequence, source) in sources {
        checkpoint()?;
        let candidate: Candidate = verified_object_observed_at(
            &engine.disk_resources,
            &members,
            "candidate",
            source.candidate,
            &mut |error| engine.metadata_integrity.fault(error),
        )?;
        engine.verify_candidate_membership(&candidate)?;
        if sequence == 0
            || candidate.sequence != sequence
            || candidate.pool_id != engine.config.pool_id
            || candidate.previous_snapshot != previous_snapshot
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "disk history candidate does not extend the complete authenticated prefix",
            ));
        }
        let publication: Publication = verified_object_observed_at(
            &engine.disk_resources,
            &members,
            "publication",
            source.publication,
            &mut |error| engine.metadata_integrity.fault(error),
        )?;
        verified_publication_seal_observed_at(
            &engine.disk_resources,
            &members,
            &candidate,
            &publication,
            &mut |error| engine.metadata_integrity.fault(error),
        )?;
        let snapshot = verified_recovery_snapshot_at(
            &engine.disk_resources,
            &members,
            &candidate.snapshot_hash,
        )?;
        let admitted = snapshot.for_candidate(&candidate)?;
        let state = admitted.state();
        let result = state
            .operations
            .get(&candidate.operation_id)
            .ok_or_else(|| invalid("disk history candidate has no original operation receipt"))?;
        if state.sequence != sequence
            || result.operation_id != candidate.operation_id
            || result.sequence != sequence
            || result.request_digest != candidate.request_digest
            || (publication.recovery.is_none() && result != &publication.result)
        {
            return Err(invalid(
                "disk history receipt differs from the published snapshot",
            ));
        }
        visit(
            &candidate,
            source.candidate,
            &publication,
            source.publication,
            admitted,
        )?;
        previous_sequence = sequence;
        previous_snapshot = Some(candidate.snapshot_hash.clone());
    }
    checkpoint()?;
    if previous_sequence != through {
        return Err(invalid(
            "disk history boundary has no complete published prefix",
        ));
    }
    if through == engine.state.sequence && previous_snapshot != engine.snapshot {
        return Err(invalid(
            "disk history prefix differs from the admitted current snapshot",
        ));
    }
    Ok(())
}

pub(super) fn capture(
    engine: &Engine,
    identity: DiskRegistrationIdentity,
    through: u64,
) -> Result<DiskHistorySnapshot> {
    let mut header = DiskHistoryHeader {
        identity: identity.clone(),
        through_sequence: 0,
        through_snapshot: None,
        publications: 0,
    };
    let mut changes = Changes {
        rows: Vec::new(),
        bytes: 0,
    };
    let mut previous = DiskCatalogSnapshot::empty(identity.clone());
    visit_published_prefix(
        engine,
        through,
        &|| Ok(()),
        |candidate, hash, _, _, admitted| {
            let next = snapshot(admitted.state(), identity.clone())?;
            changes.append(candidate, hash, &previous, &next)?;
            previous = next;
            header.through_sequence = candidate.sequence;
            header.through_snapshot = Some(candidate.snapshot_hash.clone());
            header.publications += 1;
            Ok(())
        },
    )?;
    if through == engine.state.sequence && !previous.same_rows(&snapshot(&engine.state, identity)?)
    {
        return Err(invalid(
            "disk history prefix differs from the admitted current snapshot",
        ));
    }
    Ok(DiskHistorySnapshot {
        header,
        changes: changes.rows,
    })
}

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

pub(super) fn capture(
    engine: &Engine,
    identity: DiskRegistrationIdentity,
    through: u64,
) -> Result<DiskHistorySnapshot> {
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
    for (sequence, source) in sources {
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
            || candidate.pool_id != identity.pool_id
            || candidate.previous_snapshot != header.through_snapshot
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
        let admitted = verified_recovery_snapshot_at(
            &engine.disk_resources,
            &members,
            &candidate.snapshot_hash,
        )?;
        let state = admitted.for_candidate(&candidate)?.state();
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
        let next = snapshot(state, identity.clone())?;
        changes.append(&candidate, source.candidate, &previous, &next)?;
        previous = next;
        header.through_sequence = sequence;
        header.through_snapshot = Some(candidate.snapshot_hash.clone());
        header.publications += 1;
    }
    if header.through_sequence != through {
        return Err(invalid(
            "disk history boundary has no complete published prefix",
        ));
    }
    if through == engine.state.sequence
        && (header.through_snapshot != engine.snapshot
            || !previous.same_rows(&snapshot(&engine.state, identity)?))
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

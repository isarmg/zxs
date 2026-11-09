//! Stream authenticated snapshots. Keep only adjacent local projections, never
//! an all-history State cache. Incomplete/unsupported provenance is not a prefix.
use super::*;
use crate::{
    journal::{Candidate, Publication},
    publication_evidence::*,
};

mod backup;
mod journals;

struct Source {
    candidate: String,
    publication: String,
}

#[derive(Default)]
struct Prefix {
    sequence: u64,
    snapshot: Option<String>,
    publications: usize,
}
impl Prefix {
    fn observe(&mut self, candidate: &Candidate) -> Result<()> {
        if candidate.sequence <= self.sequence || candidate.previous_snapshot != self.snapshot {
            return Err(Error::new(
                ErrorCode::LogFork,
                "disk history candidate does not extend the complete authenticated prefix",
            ));
        }
        if self.publications == MAX_PUBLICATIONS {
            return Err(budget());
        }
        self.sequence = candidate.sequence;
        self.snapshot = Some(candidate.snapshot_hash.clone());
        self.publications += 1;
        Ok(())
    }

    fn finish(&self, engine: &Engine, through: u64) -> Result<()> {
        if self.sequence != through {
            return Err(invalid(
                "disk history boundary has no complete published prefix",
            ));
        }
        if through == engine.state.sequence && self.snapshot != engine.snapshot {
            return Err(invalid(
                "disk history prefix differs from the admitted current snapshot",
            ));
        }
        Ok(())
    }
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
            "this publication visitor requires fixed Genesis; Backup disk history uses its native archive admission",
        ));
    }
    if through > engine.state.sequence {
        return Err(invalid(
            "disk history claims a publication beyond the admitted head",
        ));
    }
    let sources = journals::sources(engine, through, checkpoint)?;
    let mut prefix = Prefix::default();
    visit_sources(engine, sources, checkpoint, &mut prefix, &mut visit)?;
    prefix.finish(engine, through)
}

fn visit_sources(
    engine: &Engine,
    sources: BTreeMap<u64, Source>,
    checkpoint: &impl Fn() -> Result<()>,
    prefix: &mut Prefix,
    mut visit: impl FnMut(
        &Candidate,
        &str,
        &Publication,
        &str,
        &crate::history_package::AdmittedState,
    ) -> Result<()>,
) -> Result<()> {
    let members = engine.members();
    for (sequence, source) in sources {
        checkpoint()?;
        let candidate: Candidate = verified_object_observed_at(
            &engine.disk_resources,
            &members,
            "candidate",
            &source.candidate,
            &mut |error| engine.metadata_integrity.fault(error),
        )?;
        engine.verify_candidate_membership(&candidate)?;
        if sequence == 0
            || candidate.sequence != sequence
            || candidate.pool_id != engine.config.pool_id
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
            &source.publication,
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
        prefix.observe(&candidate)?;
        visit(
            &candidate,
            &source.candidate,
            &publication,
            &source.publication,
            admitted,
        )?;
    }
    checkpoint()
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
    let mut observe =
        |candidate: &Candidate, hash: &str, admitted: &crate::history_package::AdmittedState| {
            let next = snapshot(admitted.state(), identity.clone())?;
            changes.append(candidate, hash, &previous, &next)?;
            previous = next;
            header.through_sequence = candidate.sequence;
            header.through_snapshot = Some(candidate.snapshot_hash.clone());
            header.publications += 1;
            Ok(())
        };
    if engine.config.generation().is_some() {
        backup::visit(engine, &identity, through, &mut observe)?;
    } else {
        visit_published_prefix(
            engine,
            through,
            &|| Ok(()),
            |candidate, hash, _, _, state| observe(candidate, hash, state),
        )?;
    }
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

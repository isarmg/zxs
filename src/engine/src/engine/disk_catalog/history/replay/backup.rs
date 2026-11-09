//! A completed Backup retains the original binding history. Its new targets
//! have no pre-Backup rows; their first arrivals are the actual first slot.
use super::*;
use crate::engine::membership_transition::pending::activation::backup::startup;
use crate::history_package::AdmittedState;
use std::sync::atomic::AtomicBool;

pub(super) fn visit(
    engine: &Engine,
    identity: &DiskRegistrationIdentity,
    through: u64,
    mut visit: impl FnMut(&Candidate, &str, &AdmittedState) -> Result<()>,
) -> Result<()> {
    let generation = engine
        .config
        .generation()
        .expect("invariant: generation replay");
    if !generation.portable.is_backup() {
        return Err(Error::new(
            ErrorCode::UnsupportedOperation,
            "disk history has no authenticated replay for this membership generation",
        ));
    }
    if through > engine.state.sequence {
        return Err(invalid(
            "disk history claims a publication beyond the admitted head",
        ));
    }
    // Re-read every selected root's own archive, first Publication, registry,
    // selector and writer binding. Neither a SQLite nor a progress marker can
    // furnish this proof, and no failed root is replaced with a peer.
    let admitted = startup::verify_completed(engine)?;
    admitted.require_config(&engine.config)?;
    let first = admitted.first();
    let retained = first.original_config().disks.iter().any(|disk| {
        identity.pool_id == first.original_config().pool_id
            && identity.disk_id == disk.disk_id
            && identity.slot_id == disk.slot_id
            && identity.binding_generation == disk.binding_generation
    });
    if !retained
        && !first
            .targets()
            .iter()
            .any(|target| target.identity == *identity)
    {
        return Err(invalid(
            "disk history identity is outside the original Backup bindings",
        ));
    }
    let mut prefix = Prefix::default();
    let mut original = Prefix::default();
    admitted.replay_source(&AtomicBool::new(false), |candidate, state| {
        if candidate.pool_id != identity.pool_id {
            return Err(invalid(
                "Backup disk history archive belongs to another pool",
            ));
        }
        original.observe(candidate)?;
        if !retained && !snapshot(state.state(), identity.clone())?.is_empty() {
            return Err(invalid(
                "new Backup disk cannot inherit an original disk's historical rows",
            ));
        }
        if candidate.sequence <= through {
            let hash = storage_contract::object_hash("candidate", candidate)?;
            prefix.observe(candidate)?;
            visit(candidate, &hash, state)?;
        }
        Ok(())
    })?;
    if original.sequence != admitted.source().state().sequence
        || original.snapshot != first.candidate().previous_snapshot
        || first.candidate().pool_id != identity.pool_id
    {
        return Err(invalid(
            "Backup disk history lost its complete original publication boundary",
        ));
    }
    let mut sources = journals::sources(engine, through, &|| Ok(()))?;
    if through >= first.sequence() {
        let actual = sources.remove(&first.sequence()).ok_or_else(|| {
            invalid("Backup disk history has no actual first Publication in its generation journal")
        })?;
        if actual.candidate != first.hash()
            || actual.publication != admitted.publication().report().publication_hash
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "Backup disk history first journal differs from its native proof",
            ));
        }
        // The first slot has the special Backup membership contract. It is not
        // admitted through the ordinary later-slot registry verifier.
        prefix.observe(first.candidate())?;
        visit(first.candidate(), first.hash(), admitted.published())?;
    }
    if sources.keys().any(|sequence| *sequence <= first.sequence()) {
        return Err(invalid(
            "Backup generation journal contains a foreign historical prefix",
        ));
    }
    visit_sources(
        engine,
        sources,
        &|| Ok(()),
        &mut prefix,
        |candidate, hash, _, _, state| visit(candidate, hash, state),
    )?;
    prefix.finish(engine, through)
}

//! Join every original archived publication to the actual active-generation
//! journals. No cached candidate list, SQLite or present-day snapshot is a prefix.
use super::*;

pub(super) fn visit(
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
    engine.require_native_move_authority()?;
    let generation = engine
        .config
        .generation()
        .ok_or_else(|| invalid("Current generation absent"))?;
    let first_sequence = generation.first_sequence()?;
    if through < first_sequence {
        return Err(Error::new(
            ErrorCode::UnsupportedOperation,
            "Current disk prefix must include its authenticated activation",
        ));
    }
    let members = engine.members();
    let first = members
        .first()
        .ok_or_else(|| invalid("Current generation has no metadata owner"))?;
    // Every current metadata owner must retain its own complete control proof.
    // A missing member is not repaired or silently substituted with its peer.
    for disk in &members {
        checkpoint()?;
        let handle = engine.disk_resources.get(disk)?;
        let lease = handle.root.admit_io()?;
        engine._locks.verify_disk(disk, &handle.root)?;
        let observe = || {
            checkpoint()?;
            lease.check_running()
        };
        let writer = engine
            .journals
            .get(&disk.disk_id)
            .ok_or_else(|| invalid("Current disk prefix has no held writer"))?;
        writer.verify_retained_name()?;
        let seal = validate_seal_observed(&engine.config, disk, &mut |_| observe())?;
        validate_stored_seal(&handle.seal, &seal)?;
        let mut bytes = 0usize;
        generation.verify_archive_observed(
            &handle.root,
            disk,
            &mut |count| {
                observe()?;
                bytes = bytes.checked_add(count).ok_or_else(budget)?;
                if bytes > 128 * 1024 * 1024 {
                    return Err(budget());
                }
                Ok(())
            },
            32_768,
        )?;
        writer.verify_retained_name()?;
        lease.check_running()?;
    }
    let mut prefix = Prefix::default();
    let handle = engine.disk_resources.get(first)?;
    let lease = handle.root.admit_io()?;
    generation.visit_native_archive(
        &handle.root,
        first,
        &|| {
            checkpoint()?;
            lease.check_running()
        },
        |candidate, hash, publication, publication_hash, state| {
            if generation.historical_candidate(candidate.sequence) != Some(hash)
                || candidate.pool_id != engine.config.pool_id
            {
                return Err(invalid(
                    "Current archive is outside its exact original candidate prefix",
                ));
            }
            engine.verify_candidate_membership(candidate)?;
            prefix.observe(candidate)?;
            visit(candidate, hash, publication, publication_hash, state)
        },
    )?;
    if prefix.sequence != generation.base.sequence || prefix.snapshot != generation.snapshot() {
        return Err(invalid(
            "Current archive does not end at the original activation boundary",
        ));
    }
    let sources = journals::sources(engine, through, checkpoint)?;
    if sources.keys().any(|sequence| *sequence < first_sequence)
        || !sources.contains_key(&first_sequence)
    {
        return Err(invalid(
            "Current journal lacks its own first publication or substitutes an archived slot",
        ));
    }
    visit_sources(
        engine,
        sources,
        checkpoint,
        &mut prefix,
        |candidate, hash, publication, publication_hash, state| {
            if candidate.sequence == first_sequence {
                generation.verify_activation_candidate(candidate)?;
            }
            if publication.recovery.is_some() {
                return Err(Error::new(
                    ErrorCode::UnsupportedOperation,
                    "Current move prefix does not replace original snapshots with recovery outcomes",
                ));
            }
            visit(candidate, hash, publication, publication_hash, state)
        },
    )?;
    lease.check_running()?;
    prefix.finish(engine, through)
}

//! A planned stop supplies a restrictive probe set, not membership authority.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::store::PreparedStop;

pub(in crate::engine::membership_transition::pending::activation) fn verify_prepared_exit_stop(
    bootstrap: &BootstrapStore,
    stop: &PreparedStop,
    portable: &PortableAuthority,
    projected: &ValidatedConfig,
) -> Result<ValidatedConfig> {
    verify_prepared_exit_stop_with_io(bootstrap, stop, portable, projected, &mut NamedRootIo)
}

fn verify_prepared_exit_stop_with_io(
    bootstrap: &BootstrapStore,
    stop: &PreparedStop,
    portable: &PortableAuthority,
    projected: &ValidatedConfig,
    io: &mut impl RootIo,
) -> Result<ValidatedConfig> {
    let locator = bootstrap.stop_locator(stop).map_err(management_error)?;
    if !portable.is_exit() || !portable.matches_metadata_projection(projected)? {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "planned Exit stop differs from projected lineage",
        ));
    }
    let intent = portable.intent()?;
    let effect = intent
        .effect
        .as_ref()
        .expect("invariant: verified Exit effect");
    let identity = effect.identity(&intent.origin.config().pool_id);
    let transition = object_hash(INTENT_KIND, &intent)?;
    let hints: Vec<_> = locator.stop_hints().collect();
    if !hints.iter().any(|stop| {
        stop.disk_id == identity.disk_id
            && stop.slot_id == identity.slot_id
            && stop.binding_generation == identity.binding_generation
            && stop.operation_id == intent.operation_id
            && stop.intent_digest.to_string() == transition
    }) {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "planned locator does not exclude the exact Exit target",
        ));
    }
    for stop in &hints {
        portable.verify_bootstrap_stop(
            &crate::DiskRegistrationIdentity {
                pool_id: stop.pool_id.into(),
                disk_id: stop.disk_id.into(),
                slot_id: stop.slot_id.into(),
                binding_generation: stop.binding_generation,
            },
            stop.operation_id,
            &stop.intent_digest.to_string(),
        )?;
    }
    // Addresses come only from the exact immutable plan/chain. Rebinding the
    // projection here grants read-only probes, never the voter set or quorum.
    let probe_config = locator.restrict_config(projected.clone(), projected)?;
    let probes = locator.select(&probe_config).map_err(management_error)?;
    let evidence = discover_probes_with_io(probes, &probe_config, &hints, io)?;
    if evidence.config.generation_hash() != Some(portable.hash()?.as_str()) {
        return Err(Error::new(
            ErrorCode::LogFork,
            "retained certificates and archives select another Exit authority",
        ));
    }
    let config = locator.restrict_config(evidence.config, projected)?;
    portable
        .restrict_io(config)?
        .with_workspace_limits(Some(projected.effective_workspace_limits()))
}

#[cfg(test)]
#[path = "../../../../../../../../test/engine/unit/engine/exit_stop_probe_tests.rs"]
mod tests;

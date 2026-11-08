//! Published selection to actual fanout Config and exact native registry graph.
//! This projection retains original voters and does NOT install Current, admit
//! target I/O, register a runtime disk or release the original source fence.
use super::*;
use crate::engine::membership_transition::BackupFirstPublicationHash;

/// Configuration/registry semantics reconstructed from the exact original
/// published choice. This is not an Engine configuration input or an active
/// namespace/writer proof. In particular `config_hash` does not admit disk I/O.
#[derive(Clone, Debug)]
pub struct BackupConfigurationReport {
    pub publication: BackupTargetPublicationReport,
    pub config_hash: String,
    pub registry: crate::DiskRegistryCommitment,
    pub registry_objects: usize,
    pub registry_bytes: u64,
    pub configured_disks: Vec<DiskConfig>,
    pub original_metadata_members: Vec<String>,
    pub complete_data_route: Vec<String>,
    pub retained_replaced_terminal: DiskRegistrationIdentity,
}

/// Internal native encoders and exact candidate. No returned field is an I/O
/// admission; consumers must read the target's real graph before using it.
pub(super) struct ResolvedConfiguration {
    pub(super) report: BackupConfigurationReport,
    pub(super) config: crate::ValidatedConfig,
    pub(super) registry: crate::PreparedDiskRegistry,
    pub(super) candidate: crate::engine::membership_transition::PreparedBackupFirstCandidate,
}

pub(super) fn inspect(
    prepared: &PreparedBackupRegistry,
    decision: &crate::engine::membership_transition::AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupConfigurationReport> {
    Ok(resolve(prepared, decision, expected, stop)?.report)
}

pub(super) fn resolve(
    prepared: &PreparedBackupRegistry,
    decision: &crate::engine::membership_transition::AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<ResolvedConfiguration> {
    // Actual common Publication and every target's own native proofs/body are
    // required BEFORE returning a newly selected topology or registry digest.
    // No source or SYSTEM root is read by this detached path.
    let publication = first_publication::observe(prepared, decision, expected, stop)?;
    let (_, candidate) = first_candidate::read_verified(prepared, decision, stop)?;
    let first = &prepared.targets[0];
    let job = prepared.task.selection().first();
    let source = snapshot::read(&first.root, job, stop)?;
    let published = candidate.read_state_at(
        &first
            .root
            .private_child(&first.root.path().join("backup-generation-first-candidate"))?,
        stop,
    )?;
    if object_hash("backup-prepared-state", source.state())?
        != prepared.source_evidence().state_hash()
    {
        return Err(invalid(
            "Backup configuration lost its exact full source State",
        ));
    }
    let config = crate::ValidatedConfig::for_backup_projection(
        candidate.original_config(),
        prepared.projection(),
    )?;
    let mut route = prepared.source_evidence().retained_payload_slots();
    route.reverse();
    let primary = route
        .first()
        .ok_or_else(|| invalid("Backup selection lacks its primary"))?
        .clone();
    let mut nodes = prepared.projection().selected_nodes();
    nodes.sort_by_key(|node| (node.level, node.identity.slot_id.to_ascii_lowercase()));
    route.extend(nodes.iter().map(|node| node.identity.slot_id.clone()));
    let topology = crate::route_topology::Topology::new(config.disks.iter().map(|disk| {
        crate::route_topology::Node {
            slot: &disk.slot_id,
            upstream: disk.upstream_slot_id.as_deref(),
            level: disk.level,
        }
    }))?;
    if topology.explicit(&primary, &route)? != route {
        return Err(invalid(
            "Backup configuration does not preserve its exact complete lower chain",
        ));
    }
    let bindings: BTreeMap<_, _> = route
        .iter()
        .map(|slot| {
            let disk = config
                .disks
                .iter()
                .find(|disk| &disk.slot_id == slot)
                .expect("invariant: explicit topology admitted every slot");
            (slot.clone(), disk.binding_generation)
        })
        .collect();
    // Retired/Purged/Migrated versions also remain part of the indivisible
    // historical unit. Only the earlier payload barrier omits unavailable body.
    for placement in published.state().placements.values() {
        preparation_checkpoint(stop)?;
        if placement.primary_slot_id != primary
            || placement.route_slot_ids != route
            || placement.binding_generations != bindings
            || usize::try_from(placement.target_copies).ok() != Some(route.len())
            || placement.topology_generation != published.state().topology_generation
        {
            return Err(invalid(
                "Backup configuration splits or reduces a retained historical unit",
            ));
        }
    }
    let registry = crate::PreparedDiskRegistry::for_backup_publication(
        candidate.original_config(),
        &source,
        &job.snapshot,
        &config,
        &published,
        candidate.snapshot_hash(),
        candidate.authority_hash(),
    )?;
    let registry_bytes = registry.objects().try_fold(0u64, |sum, (_, _, bytes)| {
        sum.checked_add(bytes.len() as u64).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "Backup registry byte count overflow",
            )
        })
    })?;
    let report = BackupConfigurationReport {
        publication,
        config_hash: object_hash("backup-effective-config", &config)?,
        registry: registry.commitment().clone(),
        registry_objects: registry.objects().len(),
        registry_bytes,
        configured_disks: config.disks.clone(),
        original_metadata_members: config
            .metadata_members()
            .map(|disk| disk.disk_id.clone())
            .collect(),
        complete_data_route: route,
        retained_replaced_terminal: prepared.projection.replaced_terminal.clone(),
    };
    decision.verify_targets(prepared, stop)?;
    recovery::require_isolated(&prepared.targets)?;
    prepared.recheck_names()?;
    Ok(ResolvedConfiguration {
        report,
        config,
        registry,
        candidate,
    })
}

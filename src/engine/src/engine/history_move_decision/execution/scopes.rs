//! Actual final database set, consumed by the move executor, not another public
//! preparation report. Complete source prefix + this exact native candidate.
use super::*;
use crate::database_install::InstallAdmission;
use crate::engine::disk_catalog::{PreparedDiskScopes, root_install};
use std::{
    os::unix::fs::MetadataExt,
    sync::{Arc, atomic::AtomicBool},
};

struct DiskPair {
    identity: DiskRegistrationIdentity,
    owner: Arc<DiskHandle>,
    expected: [Vec<u8>; 2],
    pending: root_install::PreparedRootDatabases,
}
pub(super) struct Databases {
    pairs: Vec<DiskPair>,
}

pub(super) fn file_scopes(
    engine: &Engine,
    projection: &FileHistoryMoveProjection,
    candidate: &Candidate,
) -> BTreeSet<(String, String, String)> {
    let mut files = BTreeSet::new();
    for id in projection.capture().version_ids() {
        let version = &engine.state.versions[id];
        for slot in &engine.state.placements[id].route_slot_ids {
            files.insert((
                slot.clone(),
                version.history_tree_id.clone(),
                version.file_id.clone(),
            ));
        }
    }
    for evidence in &candidate.versions {
        for slot in &evidence.placement.route_slot_ids {
            files.insert((
                slot.clone(),
                evidence.version.history_tree_id.clone(),
                evidence.version.file_id.clone(),
            ));
        }
    }
    files
}

pub(super) fn require_resources(engine: &Engine, candidate: &Candidate) -> Result<()> {
    if engine.config.generation().is_some() || engine.current_namespaces.is_some() {
        return Err(invalid(
            "selected move cannot change original membership authority",
        ));
    }
    for disk in &engine.config.disks {
        let handle = engine.disk_resources.get(disk)?;
        let lease = handle.root.admit_io()?;
        let seal = validate_seal_observed(&engine.config, disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&handle.seal, &seal)?;
        root_install::require_unselected(&handle.root)?;
        if disk.metadata_member {
            engine
                .journals
                .get(&disk.disk_id)
                .ok_or_else(|| invalid("move has no original journal writer"))?
                .verify_retained_name()?;
        }
        lease.check_running()?;
    }
    engine.verify_candidate_membership(candidate)
}

fn full(engine: &Engine, version: &FileVersion, placement: &Placement) -> Result<()> {
    for slot in &placement.route_slot_ids {
        let disk = engine.disk(slot)?;
        if placement.binding_generations.get(slot) != Some(&disk.binding_generation) {
            return Err(invalid("move FULL has a changed original binding"));
        }
        let body = engine.disk_resources.get(disk)?.verify_payload(
            &version_dir(disk, &version.history_tree_id, &version.version_id),
            version,
        )?;
        if body.metadata()?.nlink() != 1 {
            return Err(invalid(
                "move requires independent FULL files, not shared hardlinks",
            ));
        }
        body.check_running()?;
    }
    Ok(())
}
pub(super) fn verify_full(
    engine: &Engine,
    projection: &FileHistoryMoveProjection,
    candidate: &Candidate,
) -> Result<()> {
    for id in projection.capture().version_ids() {
        full(
            engine,
            &engine.state.versions[id],
            &engine.state.placements[id],
        )?;
    }
    for evidence in &candidate.versions {
        full(engine, &evidence.version, &evidence.placement)?;
    }
    Ok(())
}
pub(super) fn verify_published_full(engine: &Engine, published: &PublishedMove) -> Result<()> {
    require_resources(engine, &published.candidate)?;
    for evidence in &published.candidate.versions {
        full(engine, &evidence.version, &evidence.placement)?;
    }
    Ok(())
}

impl Databases {
    pub(super) fn prepare(
        engine: &Engine,
        candidate: &Candidate,
        hash: &str,
        decided: bool,
    ) -> Result<Self> {
        let stop = AtomicBool::new(false);
        let mut scopes = PreparedDiskScopes::new(&engine.config)?;
        crate::engine::disk_catalog::visit_published_prefix(
            engine,
            engine.state.sequence,
            &|| Ok(()),
            |candidate, hash, _, _, state| scopes.observe(candidate, hash, state, &stop),
        )?;
        // A candidate-bound authenticated snapshot is sufficient to prepare SQL,
        // never to apply it. The PublishedMove capability is required below.
        let snapshot = verified_recovery_snapshot_at(
            &engine.disk_resources,
            &engine.members(),
            &candidate.snapshot_hash,
        )?;
        let admitted = snapshot.for_candidate(candidate)?;
        if !decided {
            let system_bytes =
                crate::engine::index::preparation::encode_observed(admitted, &|| Ok(()))?.len()
                    as u64;
            let system = DiskRoot::open(&engine.root)?;
            let required = system_bytes
                .checked_mul(3)
                .and_then(|bytes| bytes.checked_add(engine.config.metadata_reserve_bytes))
                .ok_or_else(limit)?;
            if system.available()? < required {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "move SYSTEM transaction must retain recovery headroom",
                ));
            }
            if system
                .optional_metadata(&system.path().join("index.sqlite"))?
                .is_some_and(|metadata| {
                    metadata.len() > crate::engine::index::preparation::MAX_BYTES as u64
                })
            {
                return Err(Error::new(
                    ErrorCode::ResourceLimit,
                    "move SYSTEM live database exceeds its bounded readback",
                ));
            }
        }
        scopes.observe(candidate, hash, admitted, &stop)?;
        let admission = if decided {
            InstallAdmission::DecidedRecovery
        } else {
            InstallAdmission::NewOperation
        };
        let mut pairs = Vec::new();
        let mut bytes = 0u64;
        scopes.finish(admitted, &stop, |identity, current, history| {
            bytes = bytes
                .checked_add(current.len() as u64 + history.len() as u64)
                .ok_or_else(limit)?;
            if bytes > 128 * 1024 * 1024 {
                return Err(limit());
            }
            let disk = engine.disk(&identity.slot_id)?;
            let owner = Arc::clone(engine.disk_resources.get(disk)?);
            let expected = [current, history];
            let pending = root_install::PreparedRootDatabases::capture(
                &owner.root,
                &identity,
                expected.clone(),
                candidate.sequence,
                admission,
            )?;
            pending.require_capacity(
                &owner.root,
                admission,
                engine.config.metadata_reserve_bytes,
            )?;
            if !decided
                && disk.capacity_bytes.is_some_and(|capacity| {
                    engine
                        .used_bytes(&identity.slot_id)
                        .checked_add(pending.storage_bytes())
                        .is_none_or(|bytes| bytes > capacity)
                })
            {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "move disk databases exceed configured capacity",
                ));
            }
            pairs.push(DiskPair {
                identity,
                owner,
                expected,
                pending,
            });
            Ok(())
        })?;
        Ok(Self { pairs })
    }
    pub(super) fn stage(&mut self, engine: &Engine) -> Result<()> {
        let stop = AtomicBool::new(false);
        for pair in &mut self.pairs {
            let disk = engine.disk(&pair.identity.slot_id)?;
            let lease = pair.owner.root.admit_io()?;
            let seal =
                validate_seal_observed(&engine.config, disk, &mut |_| lease.check_running())?;
            validate_stored_seal(&pair.owner.seal, &seal)?;
            pair.pending.stage_move(&pair.owner.root, &stop)?;
            lease.check_running()?;
        }
        Ok(())
    }
    pub(super) fn install(self, engine: &mut Engine, published: &PublishedMove) -> Result<()> {
        let stop = AtomicBool::new(false);
        require_resources(engine, &published.candidate)?;
        for pair in self.pairs {
            pair.pending.apply(&pair.owner.root, &stop)?;
            root_install::require_published_pair(
                &pair.owner.root,
                &pair.identity,
                &pair.expected,
                &stop,
            )?;
            #[cfg(feature = "fault-injection")]
            engine.failpoint("move_after_disk_database_pair")?;
        }
        Ok(())
    }
}

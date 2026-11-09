//! Pending common installers, still before any joint journal decision. No
//! alias exchange, synthetic disk-history row, SYSTEM writer or name mutation.
use super::*;
use crate::container_storage::PreparedNames;
use crate::database_install::{InstallAdmission, PreparedInstall};

const MAX_CURRENT_BYTES: u64 = 128 * 1024 * 1024;

pub(super) struct Bindings {
    pub(super) files: BTreeMap<(String, String, String), String>,
    pub(super) disks: BTreeMap<String, String>,
}

struct PendingDisk {
    disk: DiskConfig,
    owner: Arc<DiskHandle>,
    install: PreparedInstall,
}
pub(super) struct Databases {
    files: PreparedNames,
    disks: Vec<PendingDisk>,
}

fn file_scopes(
    engine: &Engine,
    projection: &FileHistoryMoveProjection,
    frozen: &FrozenCandidate,
) -> BTreeSet<(String, String, String)> {
    let mut scopes = BTreeSet::new();
    for id in projection.capture().version_ids() {
        let version = &engine.state().versions[id];
        for slot in &engine.state().placements[id].route_slot_ids {
            scopes.insert((
                slot.clone(),
                version.history_tree_id.clone(),
                version.file_id.clone(),
            ));
        }
    }
    for evidence in frozen.versions() {
        for slot in &evidence.placement.route_slot_ids {
            scopes.insert((
                slot.clone(),
                evidence.version.history_tree_id.clone(),
                evidence.version.file_id.clone(),
            ));
        }
    }
    scopes
}

impl Databases {
    pub(super) fn capture(
        engine: &Engine,
        projection: &FileHistoryMoveProjection,
        frozen: &FrozenCandidate,
        materials: &Materials,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<Self> {
        let transition = crate::state_projection::ProjectionTransition::new(
            engine.state(),
            frozen.state(),
            None,
        )?;
        crate::container_storage::validate_new_projection_paths(&transition)?;
        let files = crate::container_storage::prepare_names(
            &engine.config,
            &engine.disk_resources,
            &transition,
            MAX_METADATA,
        )?;
        files.unpublished_file_database_reservations(&file_scopes(engine, projection, frozen))?;
        let disks = capture_disks(engine, frozen, materials, cancellation)?;
        Ok(Self { files, disks })
    }
    pub(super) fn read_bindings(
        engine: &Engine,
        projection: &FileHistoryMoveProjection,
        frozen: &FrozenCandidate,
        materials: &Materials,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<Bindings> {
        let transition = crate::state_projection::ProjectionTransition::new(
            engine.state(),
            frozen.state(),
            None,
        )?;
        let files = crate::container_storage::read_unpublished_file_databases(
            &engine.config,
            &engine.disk_resources,
            transition.next(),
            &file_scopes(engine, projection, frozen),
            &mut || checkpoint(cancellation),
        )?;
        let pending = capture_disks(engine, frozen, materials, cancellation)?;
        let disks = disk_bindings(engine, &pending, cancellation)?;
        Ok(Bindings { files, disks })
    }
    pub(super) fn bindings(
        &self,
        engine: &Engine,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<Bindings> {
        checkpoint(cancellation)?;
        let files = self.files.unpublished_file_database_identities()?;
        let disks = disk_bindings(engine, &self.disks, cancellation)?;
        Ok(Bindings { files, disks })
    }
    pub(super) fn preflight(
        &self,
        engine: &Engine,
        projection: &FileHistoryMoveProjection,
        inputs: (
            &Materials,
            &crate::business::NativeMoveDraft,
            &scopes::Scopes,
        ),
        frozen: &FrozenCandidate,
        targets: &[store::Target],
        additional_history_budget: u64,
    ) -> Result<()> {
        let (materials, native, scopes) = inputs;
        let files = self
            .files
            .unpublished_file_database_reservations(&file_scopes(engine, projection, frozen))?;
        for pending in &self.disks {
            let slot = &pending.disk.slot_id;
            let participant = materials
                .record
                .disks
                .iter()
                .find(|item| item.identity.slot_id == *slot)
                .expect("invariant: pending current was captured from this original participant");
            let root = &pending.owner.root;
            let (reserved, entries) = pending.install.reservation();
            InstallAdmission::NewOperation.check(root, root.path(), reserved, entries)?;
            let (file_reserved, file_bytes) = files.get(slot).copied().unwrap_or((0, 0));
            let physical = reserved
                .checked_add(file_reserved)
                .and_then(|bytes| bytes.checked_add(installation::RESERVATION))
                .and_then(|bytes| bytes.checked_add(engine.config.metadata_reserve_bytes))
                .ok_or_else(limit)?;
            if physical > root.available()? {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "pending move databases would consume metadata reserve",
                ));
            }
            let ordinary = targets
                .iter()
                .find(|target| target.slot() == slot)
                .map(store::Target::retained_bytes)
                .transpose()?
                .unwrap_or(0);
            let required =
                super::super::store::scoped_storage_bytes(materials, participant, native, scopes)?
                    .checked_add(
                        engine
                            .history_move_choice
                            .as_ref()
                            .expect("invariant: selected move")
                            .storage_bytes(),
                    )
                    .and_then(|bytes| {
                        bytes.checked_add(frozen.retained_bytes(participant.carries_state()))
                    })
                    .and_then(|bytes| bytes.checked_add(ordinary))
                    .and_then(|bytes| bytes.checked_add(file_bytes.max(file_reserved)))
                    .and_then(|bytes| bytes.checked_add(installation::RESERVATION))
                    .and_then(|bytes| {
                        bytes.checked_add((pending.install.byte_len() as u64 + 8192).max(reserved))
                    })
                    .and_then(|bytes| bytes.checked_add(engine.used_bytes(slot)))
                    .and_then(|bytes| bytes.checked_add(additional_history_budget))
                    .ok_or_else(limit)?;
            if pending
                .disk
                .capacity_bytes
                .is_some_and(|capacity| required > capacity)
            {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "all retained move copies and database intents exceed configured capacity",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn stage(
        &mut self,
        engine: &Engine,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<(usize, usize)> {
        let files = self
            .files
            .stage_unpublished_file_databases(&mut |_| checkpoint(cancellation))?;
        for pending in &mut self.disks {
            checkpoint(cancellation)?;
            let lease = pending.owner.root.admit_io()?;
            let seal = validate_seal_observed(&engine.config, &pending.disk, &mut |_| {
                lease.check_running()
            })?;
            validate_stored_seal(&pending.owner.seal, &seal)?;
            pending
                .install
                .stage_unpublished(&pending.owner.root, &mut |_| {
                    checkpoint(cancellation)?;
                    lease.check_running()
                })?;
        }
        self.verify(engine, cancellation)?;
        Ok((files, self.disks.len()))
    }
    pub(super) fn verify(
        &self,
        engine: &Engine,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.files.verify_unpublished_file_databases()?;
        disk_bindings(engine, &self.disks, cancellation)?;
        Ok(())
    }
}

fn capture_disks(
    engine: &Engine,
    frozen: &FrozenCandidate,
    materials: &Materials,
    cancellation: &HistoryWindowCancellation,
) -> Result<Vec<PendingDisk>> {
    let mut disks = Vec::new();
    let mut bytes = 0u64;
    for participant in &materials.record.disks {
        checkpoint(cancellation)?;
        let disk = engine.disk(&participant.identity.slot_id)?.clone();
        let owner = Arc::clone(engine.disk_resources.get(&disk)?);
        let install = crate::engine::disk_catalog::prepare_move_current_install(
            &owner.root,
            frozen.native(),
            &participant.identity,
            engine.state().sequence,
        )?;
        bytes = bytes
            .checked_add(install.byte_len() as u64)
            .ok_or_else(limit)?;
        if bytes > MAX_CURRENT_BYTES {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "move current installation images exceed 128 MiB",
            ));
        }
        disks.push(PendingDisk {
            disk,
            owner,
            install,
        });
    }
    Ok(disks)
}

fn disk_bindings(
    engine: &Engine,
    disks: &[PendingDisk],
    cancellation: &HistoryWindowCancellation,
) -> Result<BTreeMap<String, String>> {
    let mut identities = BTreeMap::new();
    for pending in disks {
        checkpoint(cancellation)?;
        let lease = pending.owner.root.admit_io()?;
        let seal = validate_seal_observed(&engine.config, &pending.disk, &mut |_| {
            lease.check_running()
        })?;
        validate_stored_seal(&pending.owner.seal, &seal)?;
        let identity = pending.install.unpublished_identity(&pending.owner.root)?;
        lease.check_running()?;
        identities.insert(pending.disk.slot_id.clone(), identity);
    }
    Ok(identities)
}

//! The exact unpublished routing write set. No operation receipt, event, logical
//! history, content disposition or source pathname is rewritten by this stage.
use super::*;
use crate::history_package::AdmittedState;

const MAX_ROUTING_BYTES: usize = 16 * 1024 * 1024;

struct Selection {
    targets: Vec<BackupPreparedTopologyNode>,
    primary: DiskRegistrationIdentity,
    replaced: DiskRegistrationIdentity,
    retained: Vec<String>,
    slots: Vec<String>,
    bindings: BTreeMap<String, u64>,
    topology_generation: u64,
}
impl Selection {
    fn new(
        chain: &[DiskRegistrationIdentity],
        mut targets: Vec<BackupPreparedTopologyNode>,
        replaced: &DiskRegistrationIdentity,
        before: &State,
    ) -> Result<Self> {
        targets.sort_by_key(|target| (target.level, target.identity.slot_id.to_ascii_lowercase()));
        let retained = chain
            .iter()
            .map(|id| id.slot_id.clone())
            .collect::<Vec<_>>();
        let slots = retained
            .iter()
            .cloned()
            .chain(targets.iter().map(|target| target.identity.slot_id.clone()))
            .collect::<Vec<_>>();
        let bindings = chain
            .iter()
            .chain(targets.iter().map(|target| &target.identity))
            .map(|id| (id.slot_id.clone(), id.binding_generation))
            .collect::<BTreeMap<_, _>>();
        if bindings.len() != slots.len() {
            return Err(invalid(
                "generation routing repeats a selected or retained slot",
            ));
        }
        Ok(Self {
            targets,
            primary: chain[0].clone(),
            replaced: replaced.clone(),
            retained,
            slots,
            bindings,
            topology_generation: before
                .topology_generation
                .checked_add(1)
                .ok_or_else(budget)?,
        })
    }

    fn placement(
        &self,
        previous: &storage_contract::Placement,
    ) -> Result<storage_contract::Placement> {
        let primary = &self.primary;
        let replaced = &self.replaced;
        if previous.primary_slot_id != primary.slot_id
            || !previous.route_slot_ids.starts_with(&self.retained)
            || previous
                .route_slot_ids
                .iter()
                .any(|slot| !self.retained.contains(slot) && slot != &replaced.slot_id)
            || self
                .retained
                .iter()
                .any(|slot| previous.binding_generations.get(slot) != self.bindings.get(slot))
            || (previous.route_slot_ids.contains(&replaced.slot_id)
                && previous.binding_generations.get(&replaced.slot_id)
                    != Some(&replaced.binding_generation))
        {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "generation inputs require every retained file version on the complete unchanged lower chain; unrelated or split routes are not reassigned",
            ));
        }
        let mut next = previous.clone();
        next.route_slot_ids = self.slots.clone();
        next.binding_generations = self.bindings.clone();
        next.target_copies = u32::try_from(self.slots.len()).map_err(|_| budget())?;
        next.topology_generation = self.topology_generation;
        Ok(next)
    }

    fn paths(&self, before: &State, file: &str) -> Result<BTreeMap<String, String>> {
        let location = before
            .file_locations
            .get(file)
            .or_else(|| before.file_names.get(file))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::UnsupportedOperation,
                    "generation input needs a retained primary owner path for every whole file",
                )
            })?;
        name_policy::PortablePath::parse(location)?;
        let mut paths = before
            .file_backup_paths
            .get(file)
            .cloned()
            .unwrap_or_default();
        for target in &self.targets {
            let path = format!("{}/{}", target.source_prefix.join("/"), location);
            name_policy::PortablePath::parse(&path)?;
            if paths
                .insert(target.identity.slot_id.clone(), path)
                .is_some()
            {
                return Err(invalid(
                    "fresh target already has a retained backup owner path",
                ));
            }
        }
        Ok(paths)
    }
}

fn charge(value: &impl Serialize, used: &mut usize) -> Result<()> {
    *used = used
        .checked_add(canonical_json(value)?.len())
        .ok_or_else(budget)?;
    if *used > MAX_ROUTING_BYTES {
        return Err(budget());
    }
    Ok(())
}

pub(super) fn project(
    source: &AdmittedState,
    routes: &writer_routes::Routes,
    stop: &AtomicBool,
) -> Result<State> {
    let report = routes.report();
    let selection = Selection::new(
        &report.retained_source_chain,
        report.targets,
        &report.retained_replaced_terminal,
        source.state(),
    )?;
    project_units(source.state(), routes.units(), &selection, stop)
}

pub(in crate::engine::backup_preparation) fn project_frozen(
    source: &State,
    evidence: &BackupSourceFenceEvidence,
    projection: &BackupRegistryProjection,
    stop: &AtomicBool,
) -> Result<State> {
    let (chain, units) =
        writer_routes::frozen_selection(projection, &evidence.task.original, stop)?;
    let selection = Selection::new(
        &chain,
        projection
            .targets
            .iter()
            .map(|target| target.node.clone())
            .collect(),
        &projection.replaced_terminal,
        source,
    )?;
    project_units(source, &units, &selection, stop)
}

fn project_units(
    before: &State,
    units: &EntityPlacement,
    selection: &Selection,
    stop: &AtomicBool,
) -> Result<State> {
    let mut after = before.clone();
    after.begin_tracking();
    // This is still the original sequence. Only a future authenticated Backup
    // publication may advance it and add the exact native operation receipt.
    after.topology_generation = selection.topology_generation;
    let mut covered = BTreeSet::new();
    let mut bytes = 0;
    for file in &units.files {
        preparation_checkpoint(stop)?;
        let paths = selection.paths(before, &file.file_id)?;
        charge(&paths, &mut bytes)?;
        after.file_backup_paths.insert(file.file_id.clone(), paths);
        for id in &file.version_ids {
            preparation_checkpoint(stop)?;
            let version = before
                .versions
                .get(id)
                .ok_or_else(|| invalid("whole-file version is absent from frozen State"))?;
            let previous = before
                .placements
                .get(id)
                .ok_or_else(|| invalid("whole-file version lacks a frozen placement"))?;
            if version.history_tree_id != file.history_tree_id
                || version.file_id != file.file_id
                || !covered.insert(id.clone())
            {
                return Err(invalid(
                    "generation write set repeats or crosses a whole-file history",
                ));
            }
            let placement = selection.placement(previous)?;
            charge(&placement, &mut bytes)?;
            after.placements.insert(id.clone(), placement);
        }
    }
    if covered.iter().ne(before.versions.keys()) || covered.iter().ne(before.placements.keys()) {
        return Err(invalid(
            "generation routing omits a retained version or placement",
        ));
    }
    // Clone plus this closed three-field write set retains all old operation
    // results/principals, D/undo/retirement proof, notes, branches and node edges.
    // Unused old backup paths are preserved; no old physical file is removed.
    Ok(after)
}

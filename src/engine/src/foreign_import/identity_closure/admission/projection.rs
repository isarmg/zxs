//! Exact pure recomputation used both by the joint producer and cold replay.
//! Old receipts and credentials stay in the source archive, never live authority.
use super::super::graph::remap::Remap;
use super::*;

mod audit;
mod merge;

impl ForeignAdmissionRecipe {
    pub(crate) fn joint_state(
        &self,
        base: &State,
        original: &Config,
        target: &Config,
        fence: u64,
    ) -> Result<State> {
        let names = self.naming(base, original, target)?;
        let remap = Remap::new(&names.objects);
        let mut target_chain: Vec<_> = target
            .disks
            .iter()
            .filter(|disk| !original.disks.iter().any(|old| old.disk_id == disk.disk_id))
            .collect();
        target_chain.sort_by_key(|disk| disk.level);
        let slots = self.slot_mapping(&target_chain)?;
        let mut next = base.clone();
        next.sequence = self.first_sequence(fence)?;
        next.topology_generation = base.topology_generation.checked_add(1).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "foreign topology generation exhausted",
            )
        })?;
        next.business_generation = base.business_generation.checked_add(1).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "foreign business generation exhausted",
            )
        })?;
        let mut audit = audit::Audit::new(self, base, &names.principals, fence)?;
        for history in self.source.0.histories.keys() {
            let source = HistoryManifest::from_state(&self.source.0, history)?;
            let mut mapped = remap.manifest(&source)?;
            audit.rewrite(&mut mapped)?;
            bind_route(&mut mapped, &target_chain, &slots, next.topology_generation)?;
            mapped.validate()?;
            merge::include(&mut next, mapped);
        }
        // The complete target is checked, not just selected history projections.
        // This is an in-memory transformation boundary, not a codec round trip.
        crate::history_package::validate_prepared_state(&next)?;
        Ok(next)
    }

    /// A linear original chain is mapped by its admitted levels. Old side routes
    /// and missing lower identities are rejected, never silently discarded.
    fn slot_mapping(&self, target: &[&DiskConfig]) -> Result<BTreeMap<String, String>> {
        let mut current = self
            .source_configuration
            .disks
            .iter()
            .find(|disk| disk.slot_id == self.source_slot)
            .ok_or_else(|| invalid("foreign recipe lost its original source slot"))?;
        if current.level != self.source_level {
            return Err(invalid(
                "foreign source level differs from its original Config",
            ));
        }
        let mut slots = BTreeMap::new();
        loop {
            let selected = target
                .get(current.level as usize - 1)
                .ok_or_else(|| invalid("foreign target omitted an original lower level"))?;
            slots.insert(current.slot_id.clone(), selected.slot_id.clone());
            match &current.upstream_slot_id {
                Some(parent) => {
                    current = self
                        .source_configuration
                        .disks
                        .iter()
                        .find(|disk| disk.slot_id == *parent)
                        .expect("invariant: admitted original Config chain")
                }
                None => break,
            }
        }
        if current.slot_id != self.source_primary || slots.len() != target.len() {
            return Err(invalid(
                "foreign source does not close over the complete original primary chain",
            ));
        }
        Ok(slots)
    }
}

fn map_slot<'a>(slots: &'a BTreeMap<String, String>, original: &str) -> Result<&'a str> {
    slots
        .get(original)
        .map(String::as_str)
        .ok_or_else(|| invalid("foreign retained route lies outside the complete selected chain"))
}
fn map_paths(paths: &mut BTreeMap<String, String>, slots: &BTreeMap<String, String>) -> Result<()> {
    *paths = paths
        .iter()
        .map(|(slot, path)| Ok((map_slot(slots, slot)?.to_owned(), path.clone())))
        .collect::<Result<_>>()?;
    Ok(())
}
fn bind_route(
    mapped: &mut HistoryManifest,
    target: &[&DiskConfig],
    slots: &BTreeMap<String, String>,
    topology: u64,
) -> Result<()> {
    let primary = target.first().expect("invariant: exact complete new chain");
    let count = u32::try_from(target.len()).expect("invariant: chain bounded to 32");
    if mapped.repository.target_copies > count
        || mapped
            .placements
            .values()
            .any(|placement| placement.target_copies > count)
        || mapped
            .protection_policies
            .values()
            .any(|policy| policy.target_copies > count)
    {
        return Err(Error::new(
            ErrorCode::UnsupportedOperation,
            "foreign target chain cannot reduce a retained protection obligation",
        ));
    }
    mapped.repository.primary_slot_id = primary.slot_id.clone();
    mapped.repository.target_copies = count;
    mapped.repository.smb_enabled = false;
    mapped.repository.smb_binding_generation = 1;
    for version in mapped.versions.values_mut() {
        // Creation placement remains available in the archived original State.
        map_slot(slots, &version.creation_primary_slot_id)?;
        version.creation_primary_slot_id = primary.slot_id.clone();
    }
    for placement in mapped.placements.values_mut() {
        for slot in &placement.route_slot_ids {
            map_slot(slots, slot)?;
        }
        for slot in placement.binding_generations.keys() {
            map_slot(slots, slot)?;
        }
        placement.primary_slot_id = primary.slot_id.clone();
        placement.target_copies = count;
        placement.route_slot_ids = target.iter().map(|disk| disk.slot_id.clone()).collect();
        placement.binding_generations = target
            .iter()
            .map(|disk| (disk.slot_id.clone(), disk.binding_generation))
            .collect();
        placement.topology_generation = topology;
    }
    for rule in mapped.placement_rules.values_mut() {
        rule.primary_slot_id = map_slot(slots, &rule.primary_slot_id)?.to_owned();
    }
    for policy in mapped.protection_policies.values_mut() {
        policy.target_copies = count;
        map_paths(&mut policy.backup_prefixes, slots)?;
    }
    for paths in mapped.file_backup_paths.values_mut() {
        map_paths(paths, slots)?;
    }
    Ok(())
}

//! Pure original-cut admission of the ordinary FILE facts in an outside
//! mapping. Only the Worktree owner can bind this to an original occurrence.
//! Physical parent/BODY inodes are observed at live issue and must again match
//! real held material on use; a prior State cannot attest either inode.
use super::{registered_namespace, Failure};
use crate::business::file_draft::{self, ordinary::OrdinaryOccurrence};
use crate::persist::snapshot_worktree::outside_selection::OutsideSelectionWitness;
use crate::{Result, State, ValidatedConfig};
use storage_contract::file_container::site::FileSite;

pub(crate) fn original(
    state: &State, config: &ValidatedConfig, witness: &OutsideSelectionWitness,
) -> Result<OrdinaryOccurrence> {
    let f = witness.fields();
    let target = &f.target;
    if config.generation().is_some() || config.pool_id != f.pool.to_string()
        || state.file_deletions.contains_key(&target.file.to_string())
    {
        return Err(Failure::WholeContainerLifecycleUnsupported.into());
    }
    let record = state.file_containers.get(&target.file).ok_or(Failure::ContainerScope)?;
    if !matches!(record.site, FileSite::Created {})
        || record.working.unborn().is_some() || record.working.draft().is_some()
        || record.site.witness(target.file)? != f.site || record.created_by != f.birth
        || record.history_tree_id != f.history || record.owner_disk != f.owner
        || record.namespace.branch != f.namespace_branch
        || record.namespace.parent.ordinary() != Some(f.original_parent)
    {
        return Err(Failure::ContainerScope.into());
    }
    // Use the identical native selected occurrence rule as live SMB; this
    // binds the actual named directory chain, path, stable ENTRY/FILE,
    // selected branch/node/generation and live Version at the issuing cut.
    let (selected, occurrence) = file_draft::resolve_with_occurrence(state, target)?;
    let placement = &selected.placement;
    let version = state.versions.get(&f.selection.version_id().to_string())
        .ok_or(Failure::ContainerScope)?;
    if selected.history != f.history || selected.base != f.selection
        || crate::business::file_nodes::committed_name(state, target.file, f.selection.node_id())?
            != f.logical_name
        || version.storage_id != f.storage || version.content_hash != f.hash
        || version.logical_size != f.size || version.logical_size > config.max_file_bytes
        || version.version_sequence != f.version_sequence.get() || version.version_sequence > state.sequence
        || placement.primary_slot_id != f.slot.as_str()
        || placement.route_slot_ids.as_slice() != [f.slot.as_str()]
        || placement.binding_generations.get(f.slot.as_str()) != Some(&f.owner_generation.get())
    {
        return Err(Failure::ContainerScope.into());
    }
    let origin = registered_namespace(state, config, target.repository, f.namespace_branch)?;
    let root = &origin.origin.genesis;
    let disk = origin.disk;
    let replica = record.replicas.get(&f.owner).ok_or(Failure::ContainerScope)?;
    if origin.origin.history != f.history
        || root.root_id != f.root || root.root_entity != f.root_entity || !root.source_chain.is_empty()
        || root.registration.pool_id != f.pool.to_string() || root.registration.disk_id != f.owner.to_string()
        || root.registration.slot_id != f.slot.as_str() || root.registration.binding_generation != f.owner_generation.get()
        || disk.slot_id != root.registration.slot_id || disk.binding_generation != root.registration.binding_generation
        || disk.level != 1 || disk.upstream_slot_id.is_some()
        || replica.root_id != f.root || !replica.prefixes.is_empty()
        || replica.entity != f.container || replica.preparation != f.container_preparation
    {
        return Err(Failure::ContainerScope.into());
    }
    Ok(occurrence)
}

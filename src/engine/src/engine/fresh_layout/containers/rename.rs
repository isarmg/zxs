//! Exact same-parent ordinary namespace changes. This value carries no root or
//! decision authority; the physical consumer must retain the actual old inode.
use super::super::{FreshLayoutError as Failure, namespace};
use crate::{OperationResult, Result, State};
use name_policy::LookupName;
use std::collections::BTreeMap;
use storage_contract::FileId;
use storage_contract::file_container::ContainerNamespace;

pub(crate) struct ContainerNameChange {
    before: ContainerNamespace,
    after: ContainerNamespace,
}

impl ContainerNameChange {
    pub(crate) fn source(&self) -> &LookupName {
        &self.before.name
    }

    pub(crate) fn target(&self) -> &ContainerNamespace {
        &self.after
    }
}

/// Project only names proved by the permanent, actor-bound path facts and the
/// actual main-namespace occurrences. A cross-parent move is a different
/// transaction, even when its source and destination are on the same disk.
pub(in crate::engine) fn project(
    previous: &State,
    next: &State,
    result: &OperationResult,
) -> Result<BTreeMap<FileId, ContainerNameChange>> {
    let mut changes = BTreeMap::new();
    for file in crate::container_storage::affected_file_material(previous, next) {
        let file = file.parse::<FileId>()?;
        let Some(old) = previous.file_containers.get(&file) else { continue; };
        let target = namespace::file_target(next, file, old.history_tree_id)?;
        // Only the ordinary repository's main namespace supplies the outer
        // spelling. In particular, FILE branch display metadata or selecting a
        // different committed node must not independently drive a disk rename.
        let key = file.to_string();
        if old.working.unborn().is_none() && !old.namespace.parent.is_worktree()
            && (old.namespace != target
                || previous.file_locations.get(&key) != next.file_locations.get(&key)
                || previous.file_names.get(&key) != next.file_names.get(&key))
            && crate::container_storage::current_container_name(next, &key)?.as_str()
                != target.name.as_str()
        {
            return Err(Failure::ContainerScope.into());
        }
        if old.namespace == target {
            continue;
        }
        super::require_current_container(previous, file)?;
        super::require_current_container(next, file)?;
        let mut expected = old.namespace.clone();
        expected.name = target.name.clone();
        if expected != target || old.namespace.name == target.name
            || old.working.draft().is_some()
        {
            return Err(Failure::WholeContainerLifecycleUnsupported.into());
        }
        changes.insert(file, ContainerNameChange {
            before: old.namespace.clone(),
            after: target,
        });
    }
    if changes.is_empty() {
        return Ok(changes);
    }
    // First slice is a non-content transaction. A mixed rename/write needs a
    // distinct ordering proof for its new BODY; do not silently split it here.
    if previous.versions != next.versions || previous.placements != next.placements
        || next.operations.get(&result.operation_id) != Some(result)
        || previous.operations.contains_key(&result.operation_id)
        || result.sequence != next.sequence
        || !crate::persist::path_changes::is_kind(&result.kind)
    {
        return Err(Failure::ContainerScope.into());
    }
    crate::engine::disk_registry::current::verify_path_changes(previous, next, None, None)?;
    let facts = crate::persist::path_changes::read(result)?;
    for (file, change) in &changes {
        let mut matching = facts.iter().filter(|fact| {
            fact.file_id == *file && fact.entry_id == change.before.entry
                && fact.before.repository_id == change.before.repository
                && fact.after.repository_id == change.after.repository
                && fact.before_version_id == fact.after_version_id
        });
        let fact = matching.next().ok_or(Failure::ContainerScope)?;
        if matching.next().is_some()
            || fact.before.path.file_name() != change.source().as_str()
            || fact.after.path.file_name() != change.after.name.as_str()
            || fact.before.path.parent() != fact.after.path.parent()
        {
            return Err(Failure::ContainerScope.into());
        }
    }
    Ok(changes)
}

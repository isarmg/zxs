//! Existing Born and Unborn owners before the directory has physical feedback.
//! The actual complete FILE producer, not a provisional receipt, fixes this set.
use super::*;
use crate::engine::file_draft::WorkingBatchHandoff;

impl ContainerPreparation {
    pub(crate) fn for_working_batch(
        context: &PlanningContext<'_>,
        source: &WorkingBatchHandoff,
        prepared: &[Prepared],
    ) -> Result<Self> {
        validate_state(context.state)?;
        source.require_context(context)?;
        source.require_prepared(prepared)?;
        let first = unborn::first_working_batch(context, source, prepared)?;
        let next = source.state().clone();
        validate_state(&next)?;
        if inventory(context.state)? != inventory(&next)?
            || next.namespace_directories != context.state.namespace_directories
        {
            return Err(Failure::ContainerScope.into());
        }
        let mut expected = context.state.file_containers.clone();
        let mut targets = BTreeSet::new();
        for item in source.items() {
            let file = item.target().file;
            let previous = context.state.file_containers.get(&file)
                .ok_or(Failure::ContainerScope)?;
            let first_commit = first.contains_key(&file) && previous.working.unborn().is_some();
            if (previous.working.draft().is_none() && !first_commit)
                || previous.replicas.len() != 1
                || !(matches!(item.working_after(), FileWorkingState::ManualClean { .. })
                    || (first_commit && matches!(
                        item.working_after(), FileWorkingState::Automatic { .. },
                    )))
            {
                return Err(Failure::ContainerScope.into());
            }
            expected.get_mut(&file).ok_or(Failure::ContainerScope)?.working =
                item.working_after().clone();
            targets.insert((file, previous.owner_disk));
        }
        if next.file_containers != expected {
            return Err(Failure::ContainerScope.into());
        }
        // The same collector is used by final FILE/name material preparation.
        // Unselected existing metadata may also be affected by the real shared
        // namespace constructor; no extra working clear is granted here.
        for file in crate::container_storage::affected_file_material(context.state, &next) {
            let file = file.parse::<FileId>()?;
            let record = next.file_containers.get(&file).ok_or(Failure::ContainerScope)?;
            targets.extend(record.replicas.keys().map(|disk| (file, *disk)));
        }
        for payload in prepared {
            let file = payload.version.file_id.parse::<FileId>()?;
            let record = next.file_containers.get(&file).ok_or(Failure::ContainerScope)?;
            if payload.placement.route_slot_ids.len() != 1
                || next.versions.get(&payload.version.version_id) != Some(&payload.version)
                || next.placements.get(&payload.version.version_id) != Some(&payload.placement)
            {
                return Err(Failure::ContainerScope.into());
            }
            let owner = context.disk(&payload.placement.route_slot_ids[0])?;
            if owner.disk_id.parse::<DiskId>()? != record.owner_disk
                || !targets.contains(&(file, record.owner_disk))
            {
                return Err(Failure::ContainerScope.into());
            }
        }
        for (file, disk) in &targets {
            require_current_container(context.state, *file)?;
            let record = next.file_containers.get(file).ok_or(Failure::ContainerScope)?;
            let before = context.state.file_containers.get(file).ok_or(Failure::ContainerScope)?;
            if record.namespace != before.namespace || record.replicas != before.replicas
                || !record.replicas.contains_key(disk)
            {
                return Err(Failure::ContainerScope.into());
            }
        }
        Ok(Self {
            next, targets, additions: BTreeMap::new(), directory_additions: BTreeSet::new(),
            directory_targets: BTreeSet::new(), renames: BTreeMap::new(), birth: None, first,
            draft_reads: source.draft_reads().iter().flatten().cloned().collect(),
        })
    }
}

/// A cold caller must first loan the actual hash-bound original snapshot to
/// the full directory replay. An unknown kind on the ordinary path stays shut.
pub(crate) fn verify_directory_transition<'a>(
    previous: &'a State,
    next: &'a State,
    candidate: &'a crate::journal::Candidate,
    proof: &crate::snapshot_worktree::drafts::VerifiedDirectoryDraft<'_>,
) -> Result<VerifiedContainerTransition<'a>> {
    let result = next.operations.get(&candidate.operation_id).ok_or(Failure::ContainerScope)?;
    proof.require_pair(previous, next, result)
        .map_err(crate::snapshot_worktree::drafts::DirectoryPublicationError::into_engine)?;
    validate_state(previous)?;
    validate_state(next)?;
    let first = unborn::first_directory(previous, next, result, proof)?;
    let mut expected = previous.file_containers.clone();
    for file in proof.working_files() {
        let before = previous.file_containers.get(&file).ok_or(Failure::ContainerScope)?;
        let after = next.file_containers.get(&file).ok_or(Failure::ContainerScope)?;
        let first_commit = first.contains_key(&file) && before.working.unborn().is_some();
        if (before.working.draft().is_none() && !first_commit) || before.replicas.len() != 1
            || !(matches!(after.working, FileWorkingState::ManualClean { .. })
                || (first_commit && matches!(after.working, FileWorkingState::Automatic { .. })))
        {
            return Err(Failure::ContainerScope.into());
        }
        expected.get_mut(&file).ok_or(Failure::ContainerScope)?.working = after.working.clone();
    }
    if next.file_containers != expected
        || next.namespace_directories != previous.namespace_directories
        || inventory(previous)? != inventory(next)?
    {
        return Err(Failure::ContainerScope.into());
    }
    Ok(VerifiedContainerTransition {
        previous, next, candidate, renames: BTreeMap::new(), birth: None, first,
        draft_reads: proof.draft_reads().to_vec(),
    })
}

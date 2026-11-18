//! Continue each selected FILE on a single accumulating prospective State.
//! This does not submit a FILE transaction or operate on a shared working draft.
use super::allocations::{Allocation, Replay};
use super::{PublishedContent, content, invalid};
use crate::directory_worktree::projection::FileResult;
use crate::engine::{PlanningContext, Prepared, ReadSource};
use crate::persist::snapshot_worktree::content_batch::Output;
use crate::snapshot_worktree::{OriginalRow, ReservedFileVersion};
use crate::working_tree::snapshot::SnapshotFileSource;
use crate::{Result, State};
use std::collections::BTreeSet;
use storage_contract::{CommitId, FileNodeId, OperationId, Principal, TreeId};

#[derive(Clone, Copy)]
pub(crate) enum Trace<'a> {
    Allocate,
    Exact(&'a [Allocation]),
}

pub(crate) fn advance(
    context: &PlanningContext<'_>, next: &mut State, prepared: &mut Vec<Prepared>,
    row: &OriginalRow, body: &ReadSource, version_owner: &mut ReservedFileVersion,
    operation: OperationId, principal: &Principal, message: &str, trace: Trace<'_>,
) -> Result<Output> {
    let prior = next.clone();
    let (primary, copies) = context.new_file_placement_for_file(
        &row.target.repository.to_string(), Some(&row.target.file.to_string()), row.target.path.as_str(),
    )?;
    let disk = context.disk(&primary)?;
    if copies != 1 || disk.disk_id != row.owner.to_string() || disk.level != 1
        || disk.upstream_slot_id.is_some() || next.versions.contains_key(&row.edit.material.version_id.to_string())
        || next.file_containers.get(&row.target.file).is_none_or(|record| {
            record.working != row.clean || record.namespace != row.namespace
                || record.owner_disk != row.owner || record.history_tree_id != row.history
        })
        || next.branches.get(&row.base.branch_id().to_string())
            .is_none_or(|branch| branch.head_commit_id != row.branch_base.to_string())
    {
        return Err(invalid());
    }
    row.container.verify()?;
    let version = context.build_original_content_version(next, prepared, crate::business::VersionBuild {
        history: &row.history.to_string(), file_id: &row.target.file.to_string(),
        path: &row.target.path, source: body, primary: &primary, copies,
        source_ids: vec![row.original.version_id.to_string()], checkpoint: false,
    }, version_owner)?;
    version_owner.require_consumed()?;
    if version.version_id != row.edit.material.version_id.to_string()
        || version.logical_size != row.edit.material.size || version.content_hash != row.edit.material.content_hash
    {
        return Err(invalid());
    }
    let placement = next.placements.get(&version.version_id).cloned().ok_or_else(invalid)?;
    let limit = super::trace_limit(&prior, &row.target)?;
    let mut replay = match trace {
        Trace::Allocate => None,
        Trace::Exact(frozen) if frozen.len() <= limit => Some(Replay::new(&prior, frozen)?),
        Trace::Exact(_) => return Err(invalid()),
    };
    let mut allocations = Vec::new();
    content::publish(next, &row.target, row.base, row.edit.material.version_id, message, principal.as_str(),
        operation, &mut |prefix| {
            if let Some(replay) = replay.as_mut() {
                return replay.next(prefix);
            }
            if allocations.len() >= limit { return Err(invalid()); }
            let id = Allocation::generate(prefix)?;
            let text = id.text(prefix)?;
            allocations.push(id);
            Ok(text)
        })?;
    if let Some(replay) = replay { replay.finish()?; }
    let allocations = match trace {
        Trace::Allocate => {
            // Reject a real allocated ID colliding with the frozen source or
            // another ID in this same row rather than silently overwriting it.
            Replay::new(&prior, &allocations)?;
            allocations
        }
        Trace::Exact(frozen) => frozen.to_vec(),
    };
    let branch = next.branches.get(&row.base.branch_id().to_string()).ok_or_else(invalid)?;
    let commit = next.commits.get(&branch.head_commit_id).ok_or_else(invalid)?;
    let commit_id: CommitId = commit.commit_id.parse()?;
    let tree: TreeId = commit.tree_id.parse()?;
    let issued: BTreeSet<_> = allocations.iter().copied().collect();
    if commit.operation_id != operation || commit.author != principal.as_str() || commit.message != message
        || commit.parent_commit_ids != [row.branch_base.to_string()]
        || !issued.contains(&Allocation::Commit(commit_id)) || !issued.contains(&Allocation::Tree(tree))
        || next.file_containers.get(&row.target.file).is_none_or(|record| record.working != row.clean)
    {
        return Err(invalid());
    }
    Ok(Output {
        path: row.edit.path.clone(), commit: commit_id, tree,
        file: FileResult { path: row.edit.path.clone(), material: row.edit.material,
            source: SnapshotFileSource::Container { branch: row.base.branch_id(),
                node: FileNodeId::Version(row.edit.material.version_id) } },
        content: PublishedContent { version, placement, allocations },
    })
}

/// No archive or pathname grants admission. During occupancy, every ordinary
/// write (including an unrelated command on the same ordinary branch) must
/// preserve exactly these selected heads, working revisions and named owners.
pub(crate) fn require_frozen(state: &State, plan: &crate::persist::snapshot_worktree::content_batch::Reservation)
    -> Result<()>
{
    let repository = state.repositories.get(&plan.host.namespace.repository_id.to_string()).ok_or_else(invalid)?;
    let ordinary = state.branches.get(&repository.main_branch_id).ok_or_else(invalid)?;
    if !repository.file_manager_root || ordinary.branch_id != plan.host.namespace.branch_id.to_string()
        || ordinary.generation != plan.ordinary_generation.get() || ordinary.head_commit_id != plan.ordinary_head.to_string()
        || ordinary.state != crate::BranchState::Active
    { return Err(invalid()); }
    let mut entries = state.commits.get(&ordinary.head_commit_id).ok_or_else(invalid)?.entries.clone();
    let names = crate::business::namespace::container_name::current_names(state, &entries)?;
    crate::business::namespace::container_name::apply_file_names(&mut entries, &names)?;
    let tree = crate::NamespaceTree::from_current_state(state, &repository.repository_id, &entries);
    let mut ids = Vec::new();
    for input in &plan.rows {
        let target = storage_contract::file_draft::DraftLocation {
            repository: plan.host.namespace.repository_id, path: input.edit.path.clone(),
            entry: input.namespace.entry, file: input.original.file_id,
        };
        let selected = super::from_worktree_namespace(state, &target, tree)?;
        let container = state.file_containers.get(&target.file).ok_or_else(invalid)?;
        let replica = container.replicas.get(&input.owner).ok_or_else(invalid)?;
        let branch = state.branches.get(&input.base.branch_id().to_string()).ok_or_else(invalid)?;
        let version = state.versions.get(&input.original.version_id.to_string()).ok_or_else(invalid)?;
        if input.base != selected.base || input.history != selected.history
            || input.history != container.history_tree_id || input.clean != container.working
            || input.namespace != container.namespace || input.owner != container.owner_disk
            || input.owner_root != replica.root_id || input.owner_entity != replica.entity
            || version.logical_size != input.original.size || version.content_hash != input.original.content_hash
            || branch.head_commit_id != input.branch_base.to_string()
            || selected.placement.primary_slot_id != plan.host.registration.slot_id
            || selected.placement.binding_generations.get(&plan.host.registration.slot_id)
                != Some(&plan.host.registration.binding_generation)
            || state.versions.contains_key(&input.edit.material.version_id.to_string())
            || input.allocations.len() > super::trace_limit(state, &target)?
        { return Err(invalid()); }
        // Validate the actual per-container short storage ID before freezing.
        crate::container_storage::allocate_version_storage_id(
            state, &target.file.to_string(), input.edit.material.version_id,
        )?;
        ids.extend(input.allocations.iter().copied());
    }
    drop(Replay::new(state, &ids)?);
    Ok(())
}

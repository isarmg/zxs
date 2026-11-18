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

/// Full State reconstruction from the *original* reserve and prospective
/// result. Candidate admission must separately verify physical source/destination
/// bytes and the actual pre-existing published original/outcome prefix.
pub(crate) fn replay(
    config: &crate::ValidatedConfig, previous: &State, result: &crate::OperationResult,
    principal: &Principal, plan: &crate::persist::snapshot_worktree::content_batch::Reservation,
    value: &crate::persist::snapshot_worktree::content_batch::Publication,
) -> Result<State> {
    use crate::persist::snapshot_worktree::{self as codec, Record};
    let decode_error = |source| super::worktree::failure(crate::snapshot_worktree::RegistrationError::Codec(source));
    if result.kind != codec::CONTENT_PUBLISH || result.operation_id != plan.publish_operation
        || result.sequence <= previous.sequence || previous.operations.contains_key(&result.operation_id)
        || config.generation().is_some() || value.rows.len() != plan.rows.len() || value.rows.is_empty()
        || value.reserve != plan.request.operation_id || value.reservation != Record::ReserveContentBatch(plan.clone())
            .commitment().map_err(decode_error)?
    { return Err(invalid()); }
    require_frozen(previous, plan)?;
    let decoded = codec::decode(result).map_err(decode_error)?.ok_or_else(invalid)?;
    if decoded.record != Record::PublishContentBatch(value.clone()) || decoded.membership_epoch != config.membership_epoch()
        || decoded.receipt(result.operation_id, storage_contract::Sequence::new(result.sequence), principal)
            .map_err(decode_error)? != *result
    { return Err(invalid()); }
    let mut expected = super::unborn::begin(previous, storage_contract::Sequence::new(result.sequence))?;
    let repository = previous.repositories.get(&plan.host.namespace.repository_id.to_string()).ok_or_else(invalid)?;
    let mut entries = previous.commits.get(&plan.ordinary_head.to_string()).ok_or_else(invalid)?.entries.clone();
    let names = crate::business::namespace::container_name::current_names(previous, &entries)?;
    crate::business::namespace::container_name::apply_file_names(&mut entries, &names)?;
    let tree = crate::NamespaceTree::from_current_state(previous, &repository.repository_id, &entries);
    for (i, (row, out)) in plan.rows.iter().zip(&value.rows).enumerate() {
        let target = storage_contract::file_draft::DraftLocation { repository: plan.host.namespace.repository_id,
            entry: row.namespace.entry, file: row.original.file_id, path: row.edit.path.clone() };
        let selection = super::from_worktree_namespace(previous, &target, tree)?;
        if out.commit != row.commit || out.tree != row.tree || out.path != target.path || out.file.path != target.path
            || out.file.material != row.edit.material || out.content.allocations != row.allocations
            || out.file.source != (SnapshotFileSource::Container { branch: row.base.branch_id(),
                node: FileNodeId::Version(row.edit.material.version_id) })
            || out.content.version.version_id != row.edit.material.version_id.to_string()
            || out.content.version.checksum_chunk_bytes != config.checksum_chunk_bytes
            || decoded.protection.get(i) != Some(&crate::persist::file_draft::content_protection_for_owner(
                row.owner, &out.content)?)
        { return Err(invalid()); }
        content::replay_full(previous, &mut expected, &target, &selection, row.edit.material.size,
            row.edit.material.content_hash, &out.content, &plan.request.message, principal.as_str(), plan.publish_operation)?;
        let branch = expected.branches.get(&row.base.branch_id().to_string()).ok_or_else(invalid)?;
        let commit = expected.commits.get(&branch.head_commit_id).ok_or_else(invalid)?;
        if commit.commit_id != row.commit.to_string() || commit.tree_id != row.tree.to_string()
            || commit.parent_commit_ids != [row.branch_base.to_string()]
            || expected.file_containers.get(&target.file).is_none_or(|r| r.working != row.clean)
        { return Err(invalid()); }
    }
    crate::container_storage::capture_changed_names(previous, &mut expected);
    super::unborn::finish_state(previous, expected, result, principal)
}

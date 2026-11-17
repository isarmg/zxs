//! One actual occurrence supplies the FILE branch and its selected base.
use super::DraftError as Failure;
use crate::{BranchKind, BranchState, ContentState, Error, Result, State};
use storage_contract::file_container::FileContainerRecord;
use storage_contract::file_draft::{DraftBase, DraftLocation, FileWorkingState};
use storage_contract::{FileId, Generation, HistoryTreeId, NamespaceEntryKind, Placement};

pub(crate) struct Selection {
    pub(crate) history: HistoryTreeId,
    pub(crate) base: DraftBase,
    pub(crate) placement: Placement,
}

fn missing() -> Error {
    Error::FileDraft(Failure::Selection)
}

pub(crate) fn resolve(state: &State, target: &DraftLocation) -> Result<Selection> {
    // The first writer is deliberately one root-level, already owned container.
    // A sparse worktree path or same FileId in an unrelated namespace is not it.
    if target.path.parent().is_some() {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    let record = state.file_containers.get(&target.file).ok_or_else(missing)?;
    let repository = state.repositories.get(&target.repository.to_string())
        .ok_or_else(missing)?;
    let main = state.branches.get(&repository.main_branch_id).ok_or_else(missing)?;
    if !repository.file_manager_root || main.state != BranchState::Active
        || record.namespace.repository != target.repository
        || record.namespace.branch.to_string() != repository.main_branch_id
        || record.namespace.entry != target.entry
        || record.namespace.name.as_str() != target.path.file_name()
    {
        return Err(missing());
    }
    let mut entries = state.commits.get(&main.head_commit_id)
        .ok_or_else(missing)?.entries.clone();
    let names = super::super::namespace::container_name::current_names(state, &entries)?;
    super::super::namespace::container_name::apply_file_names(&mut entries, &names)?;
    let tree = super::super::NamespaceTree::from_current_state(
        state, &repository.repository_id, &entries,
    );
    let entry = tree.get_key(target.path.key()).ok_or_else(missing)?;
    if entry.kind != NamespaceEntryKind::File || entry.file_id != target.file.to_string()
        || entry.entry_id != target.entry.to_string()
    {
        return Err(missing());
    }
    // This is the exact path context installed by the real FILE selection.
    // Do not fall back to the main branch or a global file_selections default.
    let context = state.file_contexts.get(&super::super::directory::context_key_from_key(
        &repository.repository_id, target.path.key(),
    )).ok_or_else(missing)?;
    let branch = state.branches.get(&context.branch_id).ok_or_else(missing)?;
    if branch.branch_kind != BranchKind::File || branch.state != BranchState::Active
        || branch.file_id.as_deref() != Some(entry.file_id.as_str())
        || branch.history_tree_id != repository.history_tree_id
        || context.inherited_from.is_some()
        || state.file_branch_aliases.contains_key(&branch.branch_id)
    {
        return Err(missing());
    }
    // Pending FILE branches inherit a real immutable version. Resolve that
    // exact selected HEAD below; only an explicit commit materializes the branch.
    let head = super::super::file_nodes::branch_head(state, &branch.branch_id)?;
    let node = context.node_id.as_deref().unwrap_or(&entry.version_id);
    if node != head || super::super::file_nodes::node_version(state, &entry.file_id, node)?
        != entry.version_id
    {
        return Err(missing());
    }
    let version = state.versions.get(&entry.version_id).ok_or_else(missing)?;
    let placement = state.placements.get(&entry.version_id).ok_or_else(missing)?;
    if version.history_tree_id != repository.history_tree_id
        || version.file_id != entry.file_id
        || state.content_states.get(&entry.version_id) != Some(&ContentState::Available)
        || record.replicas.len() != 1 || placement.route_slot_ids.len() != 1
        || placement.target_copies != 1
    {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    Ok(Selection {
        history: repository.history_tree_id.parse()?,
        base: DraftBase::new(
            branch.branch_id.parse()?, node.parse()?, entry.version_id.parse()?,
            Generation::new(context.selection_generation),
        ),
        placement: placement.clone(),
    })
}

/// Cross-record integrity only. The selected head may have advanced after the
/// original dirty base; that is a conflict for writes, not a fake missing body.
pub(crate) fn validate(
    state: &State,
    file: FileId,
    record: &FileContainerRecord,
) -> Result<()> {
    if record.working.revision().generation().get() == 0 {
        return Err(super::invalid());
    }
    let FileWorkingState::ManualDirty { base, .. } = &record.working else {
        return Ok(());
    };
    let branch = state.branches.get(&base.branch_id().to_string())
        .ok_or_else(super::invalid)?;
    let version = state.versions.get(&base.version_id().to_string())
        .ok_or_else(super::invalid)?;
    if branch.branch_kind != BranchKind::File
        || branch.file_id.as_deref() != Some(file.to_string().as_str())
        || branch.history_tree_id != record.history_tree_id.to_string()
        || version.history_tree_id != branch.history_tree_id
        || version.file_id != file.to_string()
        || super::super::file_nodes::retained_node_version(
            state, &file.to_string(), &base.node_id().to_string(),
        )? != version.version_id
    {
        return Err(super::invalid());
    }
    let head = super::super::file_nodes::retained_branch_head(state, &branch.branch_id)?;
    if !super::super::file_nodes::belongs_to_branch(
        state, &branch.branch_id, &head, &base.node_id().to_string(),
    ) {
        return Err(super::invalid());
    }
    Ok(())
}

pub(crate) fn target(file: FileId, record: &FileContainerRecord) -> Result<DraftLocation> {
    Ok(DraftLocation {
        repository: record.namespace.repository,
        path: name_policy::PortablePath::parse(record.namespace.name.as_str())?,
        entry: record.namespace.entry,
        file,
    })
}

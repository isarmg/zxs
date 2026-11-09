//! Exact restoration of an occurrence displaced by a native transaction.
//! The owning transaction admits the retained evidence; this layer owns the
//! non-overwriting namespace transition shared by live undo and replay.
use super::*;
use crate::{BranchState, Commit, FileContext, FileDeletionNode, NamespaceEntryKind, OperationId};
use data_serialization::{Deserialize, Serialize};
use json_codec::json;

mod proof;
pub(crate) use proof::{admit_retained_undo, verify_plan, verify_publication};
pub(super) use proof::original;

const COMMIT_KIND: &str = "file_deletion_restore";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub(crate) struct DisplacedOccurrence {
    pub(crate) namespace_head: String,
    pub(crate) displaced: Entry,
    pub(crate) displaced_context: FileContext,
}

#[derive(Clone, Copy)]
pub(crate) struct RestorePublication<'a> {
    pub(crate) operation_id: OperationId,
    pub(crate) principal: &'a str,
}

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "restoration differs from its original displaced occurrence",
    )
}

fn collision() -> Error {
    Error::new(
        ErrorCode::NameCollision,
        "the deleted file's original path or identity is occupied; preserve its deletion and move the occupant before restoring",
    )
}

fn identity(operation: OperationId, prefix: &str, label: &str) -> Result<String> {
    let digest = crate::object_hash(
        "file-occurrence-restore-v1",
        &json!({"operation_id": operation, "prefix": prefix, "label": label}),
    )?;
    Ok(format!("{prefix}_{}", &digest[..32]))
}

fn original_namespace<'a>(
    state: &'a State,
    node: &FileDeletionNode,
    evidence: &DisplacedOccurrence,
) -> Result<&'a Commit> {
    let original = state.commits.get(&evidence.namespace_head).ok_or_else(invalid)?;
    let entry = &evidence.displaced;
    let context = &evidence.displaced_context;
    let selected = state.branches.get(&context.branch_id).ok_or_else(invalid)?;
    let version = state.versions.get(&entry.version_id).ok_or_else(invalid)?;
    if original.history_tree_id != node.history_tree_id
        || !original.entries.iter().any(|old| old == entry)
        || entry.kind != NamespaceEntryKind::File
        || entry.file_id != node.file_id
        || entry.path != node.path
        || version.file_id != node.file_id
        || version.history_tree_id != node.history_tree_id
        || version.logical_size != entry.size
        || selected.file_id.as_deref() != Some(node.file_id.as_str())
        || selected.history_tree_id != node.history_tree_id
        || context.inherited_from.is_some()
        || node.branch_heads.get(&context.branch_id).map(String::as_str)
            != Some(context.node_id.as_deref().unwrap_or(&entry.version_id))
    {
        return Err(invalid());
    }
    Ok(original)
}

fn parent_chain<'a>(
    original: &'a [Entry],
    current: &'a [Entry],
    displaced: &Entry,
) -> Result<Vec<&'a Entry>> {
    let mut parents = Vec::new();
    let mut key = displaced.lookup_key();
    while let Some((parent, _)) = key.rsplit_once('/') {
        let old = original
            .binary_search_by(|entry| entry.lookup_key().cmp(parent))
            .ok()
            .map(|index| &original[index])
            .ok_or_else(invalid)?;
        let now = current
            .binary_search_by(|entry| entry.lookup_key().cmp(parent))
            .ok()
            .map(|index| &current[index])
            .ok_or_else(collision)?;
        if old.kind != NamespaceEntryKind::Directory || old != now {
            return Err(collision());
        }
        parents.push(now);
        key = parent;
    }
    Ok(parents)
}

fn require_parents(
    state: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    repository: &Repository,
    parents: &[&Entry],
) -> Result<()> {
    for directory in std::iter::once(repository.repository_id.as_str())
        .chain(parents.iter().map(|parent| parent.entry_id.as_str()))
    {
        if state.directory_git_repositories.contains_key(directory) {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "displaced ordinary files cannot be restored into initialized directories",
            ));
        }
        if let Some(view) = view
            && let Some(branch) = state.directory_selections.get(directory)
        {
            view.require_directory_branch_write(state, branch)?;
        }
    }
    Ok(())
}

fn namespace<'a>(
    previous: &'a State,
    current: Option<&'a crate::engine::CurrentNamespaces>,
    repository: &Repository,
) -> Result<&'a [Entry]> {
    let branch = previous.branches.get(&repository.main_branch_id).ok_or_else(invalid)?;
    if !repository.file_manager_root || branch.state != BranchState::Active {
        return Err(lifecycle::frozen());
    }
    let head = previous.commits.get(&branch.head_commit_id).ok_or_else(invalid)?;
    let entries = match current {
        Some(view) => {
            view.require_branch_write(previous, &branch.branch_id)?;
            view.branch_entries(&branch.branch_id, &head.commit_id)?
        }
        None => &head.entries,
    };
    // A retained occurrence cannot grant authority to reimport another hidden
    // row. A partially hidden owner remains a recovery conflict, not a fallback.
    if entries != head.entries {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "displaced restoration requires its complete current owner namespace",
        ));
    }
    Ok(entries)
}

fn commit(
    previous: &Commit,
    entries: Vec<Entry>,
    sequence: u64,
    request: RestorePublication<'_>,
) -> Result<Commit> {
    Ok(Commit {
        namespace_deletions: None,
        commit_id: identity(request.operation_id, "commit", "namespace")?,
        history_tree_id: previous.history_tree_id.clone(),
        kind: COMMIT_KIND.into(),
        parent_commit_ids: vec![previous.commit_id.clone()],
        tree_id: identity(request.operation_id, "tree", "namespace")?,
        sequence,
        operation_id: request.operation_id,
        author: request.principal.into(),
        message: "Restore the original displaced file occurrence".into(),
        entries,
        pending_file_versions: previous.pending_file_versions.clone(),
        provenance: None,
    })
}

/// Does not remove D or grant publication authority. Both happen in the caller's
/// single original undo, after the complete transition is independently checked.
pub(crate) fn restore(
    previous: &State,
    next: &mut State,
    current: Option<&crate::engine::CurrentNamespaces>,
    node: &FileDeletionNode,
    evidence: &DisplacedOccurrence,
    request: RestorePublication<'_>,
) -> Result<()> {
    let original = original_namespace(previous, node, evidence)?;
    let repository = previous.repositories.get(&node.repository_id).ok_or_else(invalid)?;
    if repository.history_tree_id != node.history_tree_id
        || previous.file_deletions.get(&node.file_id) != Some(node)
        || previous.file_selections.get(&node.file_id) != Some(&evidence.displaced_context.branch_id)
        || previous.file_write_origins.contains_key(&node.file_id)
    {
        return Err(invalid());
    }
    let entries = namespace(previous, current, repository)?;
    let parents = parent_chain(&original.entries, entries, &evidence.displaced)?;
    require_parents(previous, current, repository, &parents)?;
    let key = crate::business::context_key_from_key(&node.repository_id, node.path.key());
    if entries.iter().any(|entry| {
        entry.lookup_key() == node.path.key()
            || entry.entry_id == evidence.displaced.entry_id
            || entry.kind == NamespaceEntryKind::File && entry.file_id == node.file_id
    }) || previous.file_contexts.contains_key(&key)
    {
        return Err(collision());
    }
    let file_branch = &previous.branches[&evidence.displaced_context.branch_id];
    if file_branch.state != BranchState::Active
        || previous.file_branch_aliases.contains_key(&file_branch.branch_id)
    {
        return Err(lifecycle::frozen());
    }
    if let Some(view) = current {
        view.require_branch_write(previous, &file_branch.branch_id)?;
    }
    let branch = &previous.branches[&repository.main_branch_id];
    let head = &previous.commits[&branch.head_commit_id];
    let mut restored = entries.to_vec();
    restored.push(evidence.displaced.clone());
    crate::business::validate_entries(&mut restored, usize::MAX)?;
    let restored = commit(head, restored, next.sequence, request)?;
    if previous.commits.contains_key(&restored.commit_id)
        || previous.commits.values().any(|old| old.tree_id == restored.tree_id)
    {
        return Err(invalid());
    }
    next.file_contexts.insert(key, evidence.displaced_context.clone());
    let main = next.branches.get_mut(&repository.main_branch_id)
        .expect("invariant: the original namespace branch was admitted");
    main.head_commit_id = restored.commit_id.clone();
    main.generation = main.generation.checked_add(1).ok_or_else(|| {
        Error::new(ErrorCode::ResourceLimit, "namespace generation exhausted")
    })?;
    next.commits.insert(restored.commit_id.clone(), restored);
    let mut ordinal = 0u64;
    crate::business::record_restored_namespace_with_ids(
        next,
        &repository.repository_id,
        &evidence.displaced.path,
        &mut |prefix| {
            ordinal = ordinal.checked_add(1).ok_or_else(|| {
                Error::new(ErrorCode::ResourceLimit, "restoration identity budget exhausted")
            })?;
            identity(request.operation_id, prefix, &format!("directory-{ordinal}"))
        },
    )
}

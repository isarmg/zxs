//! Publish immutable FILE nodes into their own branch and current ordinary namespace.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

/// Advance a public branch's metadata head while keeping its body immutable.
/// Tags preserve an unwritten branch's inheritance; adoption during a merge
/// completes that branch even though it needs no duplicate FULL.
pub(crate) fn advance_branch_node(
    state: &mut State,
    branch: &str,
    node: &str,
    materialize: bool,
    principal: &str,
    operation: OperationId,
) -> Result<()> {
    advance_branch_node_with_ids(
        state,
        branch,
        node,
        materialize,
        principal,
        operation,
        &mut |prefix| Ok(new_id(prefix)?),
    )
}

pub(super) fn advance_branch_node_with_ids(
    state: &mut State,
    branch: &str,
    node: &str,
    materialize: bool,
    principal: &str,
    operation: OperationId,
    ids: &mut impl FnMut(&str) -> Result<String>,
) -> Result<()> {
    advance_branch_with_ids(
        state, branch, node, materialize, principal, operation, NodeCommit::Metadata, ids,
    )
}

pub(super) enum NodeCommit<'a> {
    Metadata,
    Content(&'a str),
}

pub(super) fn advance_branch_with_ids(
    state: &mut State,
    branch: &str,
    node: &str,
    materialize: bool,
    principal: &str,
    operation: OperationId,
    mode: NodeCommit<'_>,
    ids: &mut impl FnMut(&str) -> Result<String>,
) -> Result<()> {
    let previous = state.branches.get(branch).cloned().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::FileBranchNotFound,
            "file branch missing",
        )
    })?;
    let file = previous.file_id.as_deref().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::FileBranchRequired,
            "an independent file branch is required",
        )
    })?;
    let version = super::file_nodes::node_version(state, file, node)?;
    let body = &state.versions[&version];
    let logical_size = body.logical_size;
    let current = state.commits.get(&previous.head_commit_id).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "file branch snapshot missing",
        )
    })?;
    let mut entries = current.entries.clone();
    let mut found = false;
    for entry in entries
        .iter_mut()
        .filter(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == file)
    {
        entry.version_id = version.clone();
        entry.size = logical_size;
        found = true;
    }
    if !found {
        return Err(Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "file branch body missing",
        ));
    }
    let pending = if materialize {
        BTreeMap::new()
    } else {
        previous.pending_file_versions.clone()
    };
    let (kind, message, provenance, selected_node) = match mode {
        NodeCommit::Metadata => (
            "file_metadata", "Advance file metadata node", Some(node.to_owned()),
            Some(node.to_owned()),
        ),
        NodeCommit::Content(message) => ("file_content", message, None, None),
    };
    let id = ids("commit")?;
    state.commits.insert(
        id.clone(),
        Commit {
            namespace_deletions: None,
            commit_id: id.clone(),
            history_tree_id: previous.history_tree_id,
            kind: kind.into(),
            parent_commit_ids: vec![current.commit_id.clone()],
            tree_id: ids("tree")?,
            sequence: state.sequence,
            operation_id: operation,
            author: principal.into(),
            message: message.into(),
            entries,
            pending_file_versions: pending.clone(),
            provenance,
        },
    );
    let target = state.branches.get_mut(branch).expect("checked file branch");
    target.head_commit_id = id;
    target.file_node_id = selected_node;
    target.pending_file_versions = pending;
    target.generation = target.generation.checked_add(1).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "branch generation exhausted",
        )
    })?;
    Ok(())
}

/// Owner and writable FILE inputs have already been checked by the merge planner.
/// No other body, folder, branch or saved historical tree is updated.
pub(crate) struct NodePublication<'a> {
    pub(crate) file: &'a str,
    pub(crate) matching_branches: &'a BTreeSet<String>,
    pub(crate) target_branch: &'a str,
    pub(crate) target_node: &'a str,
    pub(crate) principal: &'a str,
    pub(crate) operation: OperationId,
    pub(crate) maximum: usize,
}

pub(super) fn publish_node(
    engine: &Engine,
    state: &mut State,
    repository: &Repository,
    input: &NodePublication<'_>,
) -> Result<()> {
    let version = super::file_nodes::node_version(state, input.file, input.target_node)?;
    let size = state.versions[&version].logical_size;
    let root = &repository.main_branch_id;
    let current = engine.namespace_tree(root)?;
    let mut entries = current.entries().to_vec();
    let mut changed = false;
    for (index, entry) in entries.iter_mut().enumerate().filter(|(_, entry)| {
        entry.kind == NamespaceEntryKind::File && entry.file_id == input.file
    }) {
        if current.visible_entry(index).is_none() {
            continue;
        }
        let mut context = namespace::file_context::context_at_key(
            state, &repository.repository_id, entry.lookup_key(),
        ).ok_or_else(|| Error::new(ErrorCode::FileBranchConflict, "current FILE selection missing"))?;
        if !input.matching_branches.contains(&context.branch_id) {
            continue;
        }
        if context.inherited_from.is_some() {
            return Err(Error::new(
                ErrorCode::FileBranchConflict, "merge needs an ordinary FILE selection",
            ));
        }
        context.branch_id = input.target_branch.into();
        context.node_id = Some(input.target_node.into());
        context.selection_generation = context.selection_generation.checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "file selection exhausted"))?;
        entry.version_id = version.clone();
        entry.size = size;
        state.file_contexts.insert(
            namespace::file_context::context_key_from_key(
                &repository.repository_id, entry.lookup_key(),
            ),
            context,
        );
        changed = true;
    }
    if !changed {
        return Ok(());
    }
    validate_entries(&mut entries, input.maximum)?;
    if let Some(view) = engine.admitted_current_namespaces()? {
        view.validate_draft(root, &entries)?;
        view.remove_hidden_contexts(state, &repository.repository_id, root);
    }
    let pending = namespace::file_context::selected_pending(state, repository, &entries)?;
    let live = entries.iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .map(|entry| entry.file_id.clone())
        .collect::<BTreeSet<_>>();
    let main = &engine.state().branches[root];
    let id = new_id("commit")?;
    state.commits.insert(
        id.clone(),
        Commit {
            namespace_deletions: None,
            commit_id: id.clone(),
            history_tree_id: repository.history_tree_id.clone(),
            kind: "file_node_publication".into(),
            parent_commit_ids: vec![main.head_commit_id.clone()],
            tree_id: new_id("tree")?,
            sequence: state.sequence,
            operation_id: input.operation,
            author: input.principal.into(),
            message: "Publish current file references".into(),
            entries,
            pending_file_versions: pending,
            provenance: None,
        },
    );
    let main = state.branches.get_mut(root).expect("invariant: admitted ordinary namespace root");
    main.head_commit_id = id;
    main.pending_file_versions.retain(|file, _| live.contains(file) && file != input.file);
    main.generation = main.generation.checked_add(1).ok_or_else(|| {
        Error::new(ErrorCode::ResourceLimit, "branch generation exhausted")
    })?;
    state.file_selections.insert(input.file.into(), input.target_branch.into());
    state.file_write_origins.remove(input.file);
    Ok(())
}

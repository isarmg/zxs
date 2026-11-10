//! A committed directory-owned file leaves by a complete independent history
//! fork. The source stays in its repository; ordinary copy and direct rename
//! retain their separate contracts.
use super::*;
use crate::business::history_copy::{
    self, CopyKind, CopyRoute, Destination, FileHistoryCopyRequest,
};

fn unsupported() -> Error {
    Error::new(
        ErrorCode::UnsupportedOperation,
        "directory history export requires one available file, one unchanged outer repository, and an existing non-overwriting destination",
    )
}

/// Explicit user commits, not working checkpoints or the staging index, decide
/// whether a departure is a retaining fork. Independent child references do
/// not make their files owned by the parent repository.
fn committed_departure(
    state: &State,
    repository: &str,
    entries: &[Entry],
    source: &Entry,
    destination: &name_policy::PortablePath,
) -> Result<bool> {
    let owners = rename::owners(state, repository, entries);
    let Some(owner) = rename::owner_at(&owners, source.lookup_key()) else {
        return Ok(false);
    };
    if rename::owner_at(&owners, destination.key()) == Some(owner) {
        return Ok(false);
    }
    rename::has_committed_departure(
        state,
        &BTreeMap::from([(owner, BTreeSet::from([source.file_id.as_str()]))]),
    )
}

/// Explicit branch moves must not bypass the live rename ownership rule. Until
/// batch history exports share the same publication, reject before removing a
/// historically owned source. Ordinary copy still creates a new current file.
pub(in crate::business) fn require_direct_scope_transfer(
    state: &State,
    source: &DirectoryBranch,
    target: &DirectoryBranch,
    entry: &Entry,
) -> Result<()> {
    if source.directory_id == target.directory_id {
        return Ok(());
    }
    let files = source
        .entries
        .iter()
        .filter(|member| {
            member.kind == NamespaceEntryKind::File
                && (member.lookup_key() == entry.lookup_key()
                    || namespace::is_descendant(member.lookup_key(), entry.lookup_key()))
        })
        .map(|member| member.file_id.as_str())
        .collect::<BTreeSet<_>>();
    if rename::has_committed_departure(
        state,
        &BTreeMap::from([(source.directory_id.as_str(), files)]),
    )? {
        return Err(unsupported());
    }
    Ok(())
}

pub(in crate::business) fn require_scope(
    state: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    request: &FileHistoryCopyRequest,
    entries: &[Entry],
) -> Result<()> {
    if request.source_repository_id != request.target_repository_id
        || request.expected_source_head != request.expected_target_head
    {
        return Err(unsupported());
    }
    let repo = &state.repositories[&request.source_repository_id];
    let main = &state.branches[&repo.main_branch_id];
    let retained = &state.commits[&request.expected_source_head].entries;
    if entries != retained.as_slice()
        || !main.pending_file_versions.is_empty()
        || state
            .file_deletions
            .values()
            .any(|node| node.history_tree_id == repo.history_tree_id)
    {
        return Err(unsupported());
    }
    let source_path = name_policy::LookupPath::parse(&request.source_path)?;
    let destination = name_policy::PortablePath::parse(&request.target_name)?;
    let source = namespace::find_unordered_key(entries, source_path.key())
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .ok_or_else(|| Error::new(ErrorCode::FileNotFound, "directory export source missing"))?;
    if !committed_departure(state, &repo.repository_id, entries, source, &destination)? {
        return Err(unsupported());
    }
    if namespace::find_unordered_key(entries, destination.key()).is_some()
        || state
            .file_contexts
            .contains_key(&directory::context_key_from_key(
                &repo.repository_id,
                destination.key(),
            ))
    {
        return Err(Error::new(
            ErrorCode::NameCollision,
            "directory export never replaces a target",
        ));
    }
    let mut parent = destination.parent();
    while let Some(path) = parent {
        if !namespace::find_unordered_key(entries, path.key())
            .is_some_and(|entry| entry.kind == NamespaceEntryKind::Directory)
        {
            return Err(Error::new(
                ErrorCode::ParentNotDirectory,
                "directory export requires every destination ancestor to exist",
            ));
        }
        parent = path.parent();
    }
    // A new working checkpoint may only extend the actual selected, writable
    // owner. It must not import hidden entries from an older retained tree.
    for branch in state.directory_branches.values().filter(|branch| {
        branch.repository_id == repo.repository_id
            && state.directory_selections.get(&branch.directory_id)
                == Some(&branch.directory_branch_id)
            && (branch.path.is_empty()
                || namespace::is_descendant(
                    destination.key(),
                    &name_policy::path_key(&branch.path),
                ))
    }) {
        if state.directory_tree_selections.get(&branch.directory_id) != Some(&branch.head_tree_id) {
            return Err(unsupported());
        }
        if let Some(view) = view {
            view.require_directory_branch_write(state, &branch.directory_branch_id)?;
            view.directory_projection(state, &branch.directory_branch_id)?
                .require_commit_source(state)?;
        }
    }
    Ok(())
}

pub(in crate::business) fn apply(
    previous: &State,
    next: &mut State,
    view: Option<&crate::engine::CurrentNamespaces>,
    request: &FileHistoryCopyRequest,
    route: &CopyRoute,
    principal: &str,
    operation: OperationId,
) -> Result<Value> {
    let projection = history_copy::retained::project(
        previous,
        next,
        view,
        Destination::DirectoryExport(request),
        route,
        principal,
        operation,
    )?;
    let mut ordinal = 0u64;
    let mut ids = |prefix: &str| -> Result<String> {
        let hash = object_hash(
            "directory-history-export-checkpoint",
            &json!({"operation_id":operation,"prefix":prefix,"ordinal":ordinal.to_string()}),
        )?;
        ordinal = ordinal.checked_add(1).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "directory export identity budget exceeded",
            )
        })?;
        let id = format!("{prefix}_{}", &hash[..32]);
        if previous.commits.contains_key(&id)
            || previous.branches.contains_key(&id)
            || previous.directory_branches.contains_key(&id)
            || previous.directory_trees.contains_key(&id)
        {
            return Err(history_copy::invalid());
        }
        Ok(id)
    };
    directory::record_directory_edits_with_ids(
        next,
        &request.target_repository_id,
        std::slice::from_ref(&request.target_name),
        None,
        &mut ids,
    )?;
    let mut result = projection.result;
    result["retained_lifecycle"] = json!(projection.lifecycle);
    Ok(result)
}

impl Engine {
    /// Read-only capture for an SMB rename. None means the ordinary direct
    /// rename path, not that a copy or delete has already happened.
    pub fn directory_file_export_request(
        &self,
        repository: &str,
        source_path: &name_policy::LookupPath,
        destination: &name_policy::PortablePath,
    ) -> Result<Option<FileHistoryCopyRequest>> {
        let state = self.state();
        let object = self.locate_current_object(repository, source_path)?;
        let repo = object.repository();
        let view = self.admitted_current_namespaces()?;
        let entries = current_entries(state, view, repository)?;
        let source = object.entry();
        if source.kind != NamespaceEntryKind::File
            || !committed_departure(state, repository, entries, source, destination)?
        {
            return Ok(None);
        }
        let head = &state.branches[&repo.main_branch_id].head_commit_id;
        let request = FileHistoryCopyRequest {
            source_repository_id: repository.into(),
            source_path: source.path().into(),
            expected_source_head: head.clone(),
            target_repository_id: repository.into(),
            target_name: destination.as_str().into(),
            expected_target_head: head.clone(),
        };
        // Capture the complete supported graph before recording a submit intent.
        // The same checks run again under the mutation's exact original request.
        CopyKind::DirectoryExport.sources(state, view, &request)?;
        Ok(Some(request))
    }

    pub fn export_directory_file_history(
        &mut self,
        request: &FileHistoryCopyRequest,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.copy_file_history_kind(request, principal, operation, CopyKind::DirectoryExport)
    }
}

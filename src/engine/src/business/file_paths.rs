//! Namespace facts preserve selected content nodes without extending their ancestry.
use crate::business::{file, file_nodes, namespace, now};
use crate::{
    Engine, Entry, Error, FileContext, NamespaceEntryKind, OperationId, OperationResult,
    Result, State, new_id,
};
use json_codec::json;

pub(crate) fn source_node(state: &State, file: &str, version: &str, node: &str) -> Result<()> {
    let valid = file_nodes::node_version(state, file, node)? == version;
    if !valid {
        return Err(Error::new(
            storage_contract::ErrorCode::FileNodeNotFound,
            "node does not describe the selected file body",
        ));
    }
    Ok(())
}

pub(super) struct PathChange<'a> {
    pub(super) old: &'a Entry,
    pub(super) new: &'a Entry,
    pub(super) after_entries: &'a [Entry],
    pub(super) context: Option<FileContext>,
}

pub(crate) struct ChangedPath {
    pub(crate) context: FileContext,
    pub(crate) fact: crate::persist::path_changes::PathChanged,
}

pub(crate) fn record_change(
    state: &mut State,
    repository: &str,
    change: PathChange<'_>,
    principal: &str,
    operation: OperationId,
) -> Result<ChangedPath> {
    record_change_with_ids(
        state,
        repository,
        repository,
        change,
        principal,
        operation,
        now()?,
        &mut |prefix| Ok(new_id(prefix)?),
    )
}

pub(super) fn record_change_with_ids(
    state: &mut State,
    before_repository: &str,
    repository: &str,
    change: PathChange<'_>,
    principal: &str,
    operation: OperationId,
    timestamp: u64,
    ids: &mut impl FnMut(&str) -> Result<String>,
) -> Result<ChangedPath> {
    use crate::persist::path_changes::{PathChanged, identity, invalid};
    use storage_contract::file_lifecycle::PathLocation;
    use storage_contract::{Principal, Sequence, Timestamp};

    let PathChange {
        old,
        new,
        after_entries,
        context,
    } = change;
    if old.file_id != new.file_id || old.entry_id != new.entry_id {
        return Err(invalid());
    }
    let repo = state.repositories.get(repository).cloned().ok_or_else(invalid)?;
    let main = state.branches.get(&repo.main_branch_id).ok_or_else(invalid)?;
    let mut current = state.commits.get(&main.head_commit_id).cloned().ok_or_else(invalid)?;
    // A newly required file helper starts directly at the admitted final
    // namespace. Its fork remains the original immutable main commit.
    current.entries = after_entries
        .iter()
        .filter(|entry| {
            entry.lookup_key() == new.lookup_key()
                || entry.kind == NamespaceEntryKind::Directory
                    && namespace::is_descendant(new.lookup_key(), entry.lookup_key())
        })
        .cloned()
        .collect();
    namespace::sorted(&mut current.entries);
    current.operation_id = operation;
    current.author = principal.into();
    let needs_helper = !context.as_ref().is_some_and(|context| {
        state.branches.get(&context.branch_id).is_some_and(|branch| {
            branch.file_id.as_deref() == Some(old.file_id.as_str())
        })
    });
    if needs_helper {
        if !current.entries.iter().any(|entry| entry == new) {
            return Err(invalid());
        }
        let mut key = new.lookup_key();
        while let Some((parent, _)) = key.rsplit_once('/') {
            if !current.entries.iter().any(|entry| {
                entry.lookup_key() == parent && entry.kind == NamespaceEntryKind::Directory
            }) {
                return Err(invalid());
            }
            key = parent;
        }
    }
    let mut context = match context {
        Some(context) => context,
        None => FileContext {
            node_id: None,
            selection_generation: 0,
            branch_id: file::ensure_shared_file_branch_with_ids(state, &current, new, ids)?,
            inherited_from: None,
        },
    };
    if !state
        .branches
        .get(&context.branch_id)
        .is_some_and(|branch| branch.file_id.as_deref() == Some(old.file_id.as_str()))
    {
        context.branch_id = file::ensure_shared_file_branch_with_ids(state, &current, new, ids)?;
    }
    let parent = context.node_id.as_deref().unwrap_or(&old.version_id);
    let source = state.versions.get(&old.version_id).ok_or_else(invalid)?;
    if source.file_id == old.file_id {
        source_node(state, &old.file_id, &old.version_id, parent)?;
    } else if context.node_id.is_some()
        || !state.branches.get(&context.branch_id).is_some_and(|branch| {
            branch.pending_file_versions.get(&old.file_id) == Some(&old.version_id)
        })
    {
        // A borrowed body is allowed only through its already admitted pending
        // source. It still has a namespace fact, but gains no owned content node.
        return Err(invalid());
    }
    if new.version_id != old.version_id {
        let version = state.versions.get(&new.version_id).ok_or_else(invalid)?;
        if version.file_id != old.file_id
            || version.history_tree_id != repo.history_tree_id
            || version.version_sequence != state.sequence
            || version.logical_size != new.size
            || version.source_version_ids.len() != 1
            || version.source_version_ids[0] != old.version_id
        {
            return Err(invalid());
        }
        // The actual content writer advances the version, never a path node.
        context.node_id = None;
    }
    let before = PathLocation {
        repository_id: before_repository.parse()?,
        path: old.location().clone(),
    };
    let after = PathLocation {
        repository_id: repository.parse()?,
        path: new.location().clone(),
    };
    if before == after {
        return Err(invalid());
    }
    let file_id = old.file_id.parse()?;
    let entry_id = old.entry_id.parse()?;
    let fact = PathChanged {
        event_id: identity(operation, file_id, entry_id, &before, &after)?,
        operation_id: operation,
        file_id,
        entry_id,
        history_tree_id: repo.history_tree_id.parse()?,
        branch_id: context.branch_id.parse()?,
        before_version_id: old.version_id.parse()?,
        after_version_id: new.version_id.parse()?,
        before,
        after,
        sequence: Sequence::new(state.sequence),
        occurred_at: Timestamp::from_unix_seconds(timestamp),
        author: Principal::new(principal.into())
            .map_err(|source| Error::PathChange(crate::PathChangeError::Principal(source)))?,
    };
    Ok(ChangedPath { context, fact })
}

impl Engine {
    pub fn set_file_node_note(
        &mut self,
        file: &str,
        node: &str,
        note: &storage_contract::Note,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        let note = note.as_str();
        self.mutate(
            "file.path.metadata",
            principal,
            operation,
            json!({"file_id":file,"node_id":node,"note":note}),
            |engine, state, _| {
                let history = Self::file_node_note_history(engine.state(), file, node)?;
                engine.repository_for_history(history)?;
                if note.is_empty() {
                    state.file_path_notes.remove(node);
                } else {
                    state.file_path_notes.insert(node.into(), note.into());
                }
                Ok(json!({"file_id":file,"node_id":node,"note":note}))
            },
        )
    }
    pub fn create_file_branch_from_node(
        &mut self,
        file: &str,
        version: &str,
        node: &str,
        name: &storage_contract::BranchName,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(
            "file.branch.create",
            principal,
            operation,
            json!({"file_id":file,"source_version_id":version,"source_node_id":node,"name":name.as_str()}),
            |engine, state, _| {
                source_node(state, file, version, node)?;
                let created = file::create_file_branch_change(
                    &engine.planning_context(),
                    state,
                    file::FileBranchCreation {file_id:file,source_version:version,name:name.as_str(),name_key:name.key()},
                    principal,
                    operation,
                )?;
                state
                    .branches
                    .get_mut(&created.branch_id)
                    .expect("invariant: branch was created in this mutation")
                    .file_node_id = Some(node.into());
                let mut result = created.receipt();
                result["source_node_id"] = json!(node);
                Ok(result)
            },
        )
    }
}

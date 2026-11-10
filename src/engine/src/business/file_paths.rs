//! Public immutable path events reuse real version bodies and proof identities.
use super::*;
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
    pub(super) context: Option<FileContext>,
}

pub(crate) fn rename_node(
    state: &mut State,
    repository: &str,
    old: &Entry,
    new: &Entry,
    context: Option<FileContext>,
    principal: &str,
    operation: OperationId,
) -> Result<FileContext> {
    rename_node_with_ids(
        state,
        repository,
        PathChange { old, new, context },
        principal,
        operation,
        now()?,
        &mut |prefix| Ok(new_id(prefix)?),
    )
}

pub(super) fn rename_node_with_ids(
    state: &mut State,
    repository: &str,
    change: PathChange<'_>,
    principal: &str,
    operation: OperationId,
    timestamp: u64,
    ids: &mut impl FnMut(&str) -> Result<String>,
) -> Result<FileContext> {
    let PathChange { old, new, context } = change;
    let repo = state.repositories.get(repository).cloned().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "file repository missing",
        )
    })?;
    let main = state.branches.get(&repo.main_branch_id).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "repository main branch missing",
        )
    })?;
    let current = state
        .commits
        .get(&main.head_commit_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "repository head missing",
            )
        })?;
    let mut context = if let Some(c) = context {
        c
    } else {
        FileContext {
            node_id: None,
            selection_generation: 0,
            branch_id: file::ensure_shared_file_branch_with_ids(state, &current, old, ids)?,
            inherited_from: None,
        }
    };
    if !state
        .branches
        .get(&context.branch_id)
        .is_some_and(|b| b.file_id.as_deref() == Some(&old.file_id))
    {
        context.branch_id = file::ensure_shared_file_branch_with_ids(state, &current, old, ids)?;
    }
    let parent = context
        .node_id
        .clone()
        .unwrap_or_else(|| old.version_id.clone());
    source_node(state, &old.file_id, &old.version_id, &parent)?;
    let advance_head =
        file_nodes::branch_head(state, &context.branch_id).is_ok_and(|head| head == parent);
    let parent = if new.version_id == old.version_id {
        parent
    } else {
        // A single normal save may write and rename one visible occurrence.
        // The path event follows that actual new body, whose immutable source
        // edge already preserves the prior node/version ancestry.
        let version = state.versions.get(&new.version_id).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::VersionNotFound,
                "renamed upload version missing",
            )
        })?;
        if new.entry_id != old.entry_id
            || new.file_id != old.file_id
            || version.file_id != old.file_id
            || version.history_tree_id != repo.history_tree_id
            || version.version_sequence != state.sequence
            || version.logical_size != new.size
            || version.source_version_ids.len() != 1
            || version.source_version_ids[0] != old.version_id
        {
            return Err(Error::new(
                storage_contract::ErrorCode::WorkspaceChanged,
                "renamed upload does not continue the exact visible source body",
            ));
        }
        new.version_id.clone()
    };
    let node_id = ids("fnode")?;
    state.file_path_nodes.insert(
        node_id.clone(),
        FilePathNode {
            node_id: node_id.clone(),
            history_tree_id: repo.history_tree_id,
            file_id: old.file_id.clone(),
            version_id: new.version_id.clone(),
            parent_node_ids: vec![parent],
            branch_id: context.branch_id.clone(),
            path: new.path().to_owned(),
            previous_path: old.path().to_owned(),
            sequence: state.sequence,
            created_at: timestamp,
            operation_id: operation,
            author: principal.into(),
        },
    );
    if advance_head {
        file_publication::advance_branch_node_with_ids(
            state,
            &context.branch_id,
            &node_id,
            new.version_id != old.version_id,
            principal,
            operation,
            ids,
        )?;
    }
    context.node_id = Some(node_id);
    Ok(context)
}

pub(crate) fn operation_nodes(state: &State, operation: OperationId) -> Vec<String> {
    state
        .file_path_nodes
        .values()
        .filter(|node| node.operation_id == operation && node.sequence == state.sequence)
        .map(|node| node.node_id.clone())
        .collect()
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
                let mut result = file::create_file_branch_change(
                    &engine.planning_context(),
                    state,
                    file::FileBranchCreation {file_id:file,source_version:version,name:name.as_str(),name_key:name.key()},
                    principal,
                    operation,
                )?;
                let branch = result["branch_id"].as_str().ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::StateInvalid,
                        "branch receipt missing",
                    )
                })?;
                state
                    .branches
                    .get_mut(branch)
                    .expect("created branch")
                    .file_node_id = Some(node.into());
                result["source_node_id"] = json!(node);
                Ok(result)
            },
        )
    }
}

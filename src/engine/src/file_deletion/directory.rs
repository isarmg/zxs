//! Reversible ordinary directory containers inside an initialized repository.
//! One native operation owns the container marker and every newly created file
//! D. Raw namespace rows, user commits, index and retained bodies stay intact.
use super::*;
use crate::{
    DirectoryBatchKind, DirectoryNamespaceBatch, FileDeletionNode, NamespaceEntryKind, OperationId,
    OperationResult,
};
use data_serialization::{Deserialize, Serialize};
use json_codec::{Value, json};
use std::collections::BTreeMap;
use storage_contract::directory_git::{
    DirectoryDeletionMember, DirectoryDeletionNode, UndoDirectoryDeletionBody,
};

mod freeze;
mod proof;
mod source;
pub(crate) use source::{retained_entries, mounted_entries};
pub(crate) mod visibility;
pub(crate) use freeze::verify as verify_frozen;
pub(crate) use proof::{admit, admit_original, node_for, shape as admit_node_shape, verify_publication};

pub(crate) const DELETE: &str = "directory.delete";
pub(crate) const UNDO: &str = "directory.delete.undo";

pub(crate) fn is_kind(kind: &str) -> bool {
    matches!(kind, DELETE | UNDO)
}

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "directory deletion differs from its exact container and file lifecycle",
    )
}

fn frozen() -> Error {
    Error::new(
        ErrorCode::BranchNotActive,
        "this directory is soft deleted; restore its exact directory deletion before changing its contents or restoring a member separately",
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct DeleteInput {
    repository_id: String,
    request: DirectoryNamespaceBatch,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct UndoInput {
    repository_id: String,
    owner_directory_id: String,
    directory_id: String,
    deletion_node_id: String,
}

pub(crate) struct Admitted {
    pub(crate) node: DirectoryDeletionNode,
    pub(crate) files: Vec<FileDeletionNode>,
    pub(crate) undo: bool,
}

pub(crate) fn identity(operation: OperationId, prefix: &str, owner: &str) -> Result<String> {
    let hash = crate::object_hash(
        "directory-deletion-identity",
        &json!({"operation_id": operation, "prefix": prefix, "owner": owner}),
    )?;
    Ok(format!("{prefix}_{}", &hash[..32]))
}

/// Dispatch only; the native operation rechecks the entire selected source.
/// A retained operation selects its original protocol before current visibility.
pub(crate) fn requested(
    state: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
    request: &DirectoryNamespaceBatch,
) -> Result<bool> {
    if request.kind != DirectoryBatchKind::Delete {
        return Ok(false);
    }
    for item in &request.items {
        let Some(branch) = state
            .directory_branches
            .get(&item.source.directory_branch_id)
        else {
            continue;
        };
        let admitted = current
            .map(|view| view.batch_scope(state, &branch.directory_branch_id))
            .transpose()?;
        let scope = admitted.as_ref().map_or(branch, |scope| scope.branch());
        if scope.entries.iter().any(|entry| {
            entry.entry_id == item.source.entry_id && entry.kind == NamespaceEntryKind::Directory
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn receipt_budget(result: &Value, maximum: usize) -> Result<()> {
    if json_codec::to_vec(result)?.len() > maximum {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "complete directory deletion exceeds the recovery response budget",
        ));
    }
    Ok(())
}

struct Creation<'a> {
    principal: &'a str,
    operation: OperationId,
    timestamp: u64,
    maximum: usize,
}

fn apply_create(
    previous: &State,
    next: &mut State,
    current: Option<&crate::engine::CurrentNamespaces>,
    input: &DeleteInput,
    request: Creation<'_>,
) -> Result<Value> {
    let Creation {
        principal,
        operation,
        timestamp,
        maximum,
    } = request;
    let captured = source::capture(previous, current, input, maximum)?;
    let repository = &previous.repositories[&input.repository_id];
    let repository_id = HistoryRepositoryId::parse(&input.repository_id)?;
    let mut members = BTreeMap::new();
    let mut nodes = Vec::new();
    let mut frontier = (0, 0);
    for (file, path) in &captured.files {
        if let Some(deletion) = previous.file_deletions.get(file) {
            members.insert(
                file.clone(),
                DirectoryDeletionMember {
                    deletion_node_id: deletion.node_id.clone(),
                    created_by_directory: false,
                },
            );
            continue;
        }
        let file_id = HistoryFileId::parse(file)?;
        let lookup = name_policy::LookupPath::from(path);
        preflight::require_scope_writable(
            previous,
            current,
            repository,
            captured.namespace,
            file,
            &HistoryWindowCancellation::default(),
        )?;
        let capture = preflight::capture(
            previous,
            repository,
            captured.namespace,
            FileDeletionTarget {
                repository_id: &repository_id,
                file_id: &file_id,
                path: &lookup,
            },
            maximum,
            &HistoryWindowCancellation::default(),
        )?;
        frontier.0 += capture.branches().len();
        frontier.1 += capture.parent_node_ids().len();
        super::batch::require_frontier(frontier)?;
        let node = FileDeletionNode {
            node_id: identity(operation, "fdel", file)?,
            repository_id: repository.repository_id.clone(),
            history_tree_id: repository.history_tree_id.clone(),
            file_id: file.clone(),
            path: path.clone(),
            branch_heads: capture
                .branches()
                .iter()
                .map(|head| (head.branch().branch_id.clone(), head.node_id().to_owned()))
                .collect(),
            parent_node_ids: capture.parent_node_ids().to_vec(),
            operation_id: operation,
            author: principal.into(),
            sequence: next.sequence,
            created_at: timestamp,
        };
        members.insert(
            file.clone(),
            DirectoryDeletionMember {
                deletion_node_id: node.node_id.clone(),
                created_by_directory: true,
            },
        );
        nodes.push(node);
    }
    let node = DirectoryDeletionNode {
        node_id: identity(operation, "ddel", &captured.entry.entry_id)?,
        repository_id: repository.repository_id.clone(),
        history_tree_id: repository.history_tree_id.clone(),
        owner_directory_id: captured.owner_directory_id,
        directory_id: captured.entry.entry_id,
        path: captured.entry.path,
        namespace_head: captured.namespace_head,
        directory_branch_id: captured.branch,
        source_tree_id: captured.tree,
        operation_id: operation,
        author: principal.into(),
        sequence: next.sequence,
        created_at: timestamp,
        files: members,
    };
    let result = json!({
        "input": input, "directory_deletion": node, "deletion_nodes": nodes
    });
    receipt_budget(&result, super::batch::MAX_RESULT_BYTES - 1024)?;
    next.directory_git_repositories
        .get_mut(&node.owner_directory_id)
        .expect("invariant: capture admitted the initialized owner")
        .deletions
        .insert(node.directory_id.clone(), node);
    for node in nodes {
        next.file_deletions.insert(node.file_id.clone(), node);
    }
    Ok(result)
}

fn apply_undo(
    previous: &State,
    next: &mut State,
    current: Option<&crate::engine::CurrentNamespaces>,
    input: &UndoInput,
) -> Result<Value> {
    let node = previous
        .directory_git_repositories
        .get(&input.owner_directory_id)
        .filter(|git| git.repository_id == input.repository_id)
        .and_then(|git| git.deletions.get(&input.directory_id))
        .filter(|node| node.node_id == input.deletion_node_id)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::FileNodeNotFound,
                "current directory deletion differs or is absent",
            )
        })?;
    let original = previous
        .operations
        .get(&node.operation_id)
        .ok_or_else(invalid)?;
    let admitted = proof::admit_original(previous,original,node)?;
    if admitted.undo || admitted.node != *node {
        return Err(invalid());
    }
    source::require_restore(previous, current, node)?;
    for (file, member) in &node.files {
        if previous.file_deletions.get(file).map(|node| &node.node_id)
            != Some(&member.deletion_node_id)
        {
            return Err(invalid());
        }
    }
    for deletion in &admitted.files {
        if previous.directory_git_repositories.values().any(|git| {
            git.deletions.values().any(|other| {
                other.node_id != node.node_id && other.files.contains_key(&deletion.file_id)
            })
        }) {
            return Err(Error::new(
                ErrorCode::BranchNotActive,
                "restore the other deleted directory referencing this file D first",
            ));
        }
    }
    for deletion in &admitted.files {
        if next.file_deletions.remove(&deletion.file_id).as_ref() != Some(deletion) {
            return Err(invalid());
        }
    }
    let removed = next
        .directory_git_repositories
        .get_mut(&node.owner_directory_id)
        .expect("invariant: the exact deletion owner was admitted")
        .deletions
        .remove(&node.directory_id)
        .ok_or_else(invalid)?;
    let result = json!({
        "restored_directory_id": node.directory_id,
        "removed_directory_deletion": removed,
        "removed_deletion_nodes": admitted.files,
    });
    receipt_budget(&result, super::batch::MAX_RESULT_BYTES - 1024)?;
    Ok(result)
}

/// A member cannot partially escape a still-active directory restoration unit.
pub(super) fn require_individual_undo(state: &State, file: &str) -> Result<()> {
    if state.directory_git_repositories.values().any(|git| {
        git.deletions
            .values()
            .any(|node| node.files.contains_key(file))
    }) {
        Err(frozen())
    } else {
        Ok(())
    }
}

impl Engine {
    /// Read-only capture for an adapter that will durably retain this exact
    /// request before submission. It does not reserve or publish a deletion.
    pub fn prepare_current_directory_deletion(
        &self,
        repository: &str,
        path: &name_policy::LookupPath,
        expected_entry: &str,
    ) -> Result<Option<DirectoryNamespaceBatch>> {
        source::prepare(
            self.state(),
            self.admitted_current_namespaces()?,
            repository,
            path,
            expected_entry,
            self.config().max_entries,
        )
    }

    pub(crate) fn soft_delete_directory(
        &mut self,
        repository: &str,
        request: &DirectoryNamespaceBatch,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        let input = DeleteInput {
            repository_id: repository.into(),
            request: request.clone(),
        };
        self.mutate(
            DELETE,
            principal,
            operation,
            json!(&input),
            |engine, next, _| {
                apply_create(
                    engine.state(),
                    next,
                    engine.admitted_current_namespaces()?,
                    &input,
                    Creation {
                        principal,
                        operation,
                        timestamp: crate::business::now()?,
                        maximum: engine.config().max_entries,
                    },
                )
            },
        )
    }

    pub fn undo_directory_deletion(
        &mut self,
        repository: &str,
        request: &UndoDirectoryDeletionBody,
        principal: &str,
    ) -> Result<OperationResult> {
        storage_contract::validate_id(repository, "repo")?;
        if request.owner_directory_id != repository {
            storage_contract::validate_id(&request.owner_directory_id, "entry")?;
        }
        storage_contract::validate_id(&request.directory_id, "entry")?;
        storage_contract::validate_id(&request.deletion_node_id, "ddel")?;
        let input = UndoInput {
            repository_id: repository.into(),
            owner_directory_id: request.owner_directory_id.clone(),
            directory_id: request.directory_id.clone(),
            deletion_node_id: request.deletion_node_id.clone(),
        };
        self.mutate(
            UNDO,
            principal,
            request.operation_id,
            json!(&input),
            |engine, next, _| {
                apply_undo(
                    engine.state(),
                    next,
                    engine.admitted_current_namespaces()?,
                    &input,
                )
            },
        )
    }

    pub fn directory_deletion(
        &self,
        repository: &str,
        owner: &str,
        directory: &str,
    ) -> Option<&DirectoryDeletionNode> {
        self.state()
            .directory_git_repositories
            .get(owner)
            .filter(|git| git.repository_id == repository)
            .and_then(|git| git.deletions.get(directory))
    }
}

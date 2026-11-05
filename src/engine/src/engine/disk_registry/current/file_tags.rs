//! Tags publish immutable children through exact current occurrences. The same
//! pure plan authenticates every row during live publication and cold replay.
use super::*;
use crate::{
    Branch, BranchKind, CommitState, CurrentHealth, DirectoryBranch, DirectoryTree, FileContext,
    FileMetadataKind, FileMetadataNode, FileNodeTagsUpdateRequest, FileTagTarget, MAX_FILE_TAGS,
    OperationId, OperationResult, ProtectionAtPublish, Repository,
};
use data_serialization::Deserialize;
use json_codec::{Value, json};
mod publication;
mod replay;

const KIND: &str = "file.tags.update";

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "file tag publication differs from its authenticated request and current occurrences",
    )
}
fn missing() -> Error {
    Error::new(
        ErrorCode::FileNotFound,
        "tag source occurrence is absent from its current owner",
    )
}
fn increment(value: u64) -> Result<u64> {
    value.checked_add(1).ok_or_else(|| {
        Error::new(
            ErrorCode::ResourceLimit,
            "file tag publication generation exhausted",
        )
    })
}
fn at_key<'a>(entries: &'a [Entry], key: &str) -> Option<&'a Entry> {
    entries
        .binary_search_by(|entry| entry.lookup_key().cmp(key))
        .ok()
        .map(|i| &entries[i])
}
fn same_occurrence(left: &Entry, right: &Entry) -> bool {
    left.entry_id == right.entry_id
        && left.kind == right.kind
        && left.file_id == right.file_id
        && left.path() == right.path()
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Location {
    repository_id: String,
    path: String,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Input {
    file_id: String,
    targets: Vec<FileTagTarget>,
    add: Vec<String>,
    remove: Vec<String>,
    location: Option<Location>,
}
impl Input {
    fn body(&self) -> Value {
        let location = self.location.as_ref().map(|location| {
            json!({
                "repository_id": location.repository_id, "path": location.path
            })
        });
        json!({"file_id": self.file_id, "targets": self.targets, "add": self.add,
            "remove": self.remove, "location": location})
    }
}

struct Change {
    branch: String,
    source: String,
    node: String,
    version: String,
    tags: BTreeSet<String>,
    changed: bool,
    advance: bool,
    entries: Option<Vec<Entry>>,
    directories: BTreeSet<String>,
}

/// IDs are a domain-separated function of the permanent operation and ordinal.
/// Replay needs no random source, wall clock, body I/O or caller-supplied IDs.
struct Ids {
    operation: OperationId,
    ordinal: u64,
}
impl Ids {
    fn next(&mut self, prefix: &str) -> Result<String> {
        let digest = crate::object_hash(
            "current-file-tags-id",
            &json!({
                "operation_id": self.operation, "ordinal": self.ordinal.to_string(), "kind": prefix
            }),
        )?;
        self.ordinal = increment(self.ordinal)?;
        Ok(format!("{prefix}_{}", &digest[..32]))
    }
}

type Patch<T> = BTreeMap<String, Option<T>>;
#[derive(Default)]
struct Plan {
    branches: Patch<Branch>,
    commits: Patch<Commit>,
    file_metadata_nodes: Patch<FileMetadataNode>,
    directory_branches: Patch<DirectoryBranch>,
    directory_trees: Patch<DirectoryTree>,
    file_contexts: Patch<FileContext>,
    directory_tree_selections: Patch<String>,
    result: Value,
}

struct Planner<'a> {
    view: &'a CurrentNamespaces,
    previous: &'a State,
    input: &'a Input,
    principal: &'a str,
    operation: OperationId,
    sequence: u64,
    created_at: u64,
    maximum: usize,
    ids: Ids,
    changes: Vec<Change>,
    plan: Plan,
}

struct TagPublication {
    operation: OperationId,
    sequence: u64,
    created_at: u64,
}

impl CurrentNamespaces {
    pub(crate) fn update_current_file_tags(
        &self,
        previous: &State,
        next: &mut State,
        request: &FileNodeTagsUpdateRequest<'_>,
        maximum: usize,
    ) -> Result<Value> {
        let input = Input {
            file_id: request.file_id.into(),
            targets: request.targets.to_vec(),
            add: request.add.to_vec(),
            remove: request.remove.to_vec(),
            location: request.location.map(|location| Location {
                repository_id: location.repository_id.clone(),
                path: location.path.clone(),
            }),
        };
        let plan = self.file_tag_plan(
            previous,
            &input,
            request.principal,
            TagPublication {
                operation: request.operation_id,
                sequence: next.sequence,
                created_at: crate::business::now()?,
            },
            maximum,
        )?;
        let Plan {
            branches,
            commits,
            file_metadata_nodes,
            directory_branches,
            directory_trees,
            file_contexts,
            directory_tree_selections,
            result,
        } = plan;
        macro_rules! apply {
            ($field:ident) => {
                for (id, value) in $field {
                    match value {
                        Some(value) => {
                            next.$field.insert(id, value);
                        }
                        None => {
                            next.$field.remove(&id);
                        }
                    }
                }
            };
        }
        apply!(branches);
        apply!(commits);
        apply!(file_metadata_nodes);
        apply!(directory_branches);
        apply!(directory_trees);
        apply!(file_contexts);
        apply!(directory_tree_selections);
        Ok(result)
    }

    fn file_tag_plan(
        &self,
        previous: &State,
        input: &Input,
        principal: &str,
        publication: TagPublication,
        maximum: usize,
    ) -> Result<Plan> {
        let TagPublication {
            operation,
            sequence,
            created_at,
        } = publication;
        Planner {
            view: self,
            previous,
            input,
            principal,
            operation,
            sequence,
            created_at,
            maximum,
            ids: Ids {
                operation,
                ordinal: 0,
            },
            changes: Vec::new(),
            plan: Plan::default(),
        }
        .prepare()
    }
}

impl Planner<'_> {
    fn prepare(mut self) -> Result<Plan> {
        self.preflight()?;
        self.metadata_children()?;
        if self.changes.iter().any(|change| change.changed) {
            let history = &self.previous.branches[&self.changes[0].branch].history_tree_id;
            let repository = self
                .previous
                .repositories
                .values()
                .find(|repository| &repository.history_tree_id == history)
                .ok_or_else(stale)?
                .clone();
            self.publish_directories(&repository)?;
            self.publish_main(&repository)?;
        }
        let nodes = self
            .changes
            .iter()
            .map(|change| {
                let mut result = json!({"branch_id": change.branch, "source_node_id": change.source,
                "node_id": change.node, "tags": change.tags, "no_change": !change.changed,
                "advanced": change.changed && change.advance});
                if change.changed {
                    result["directory_branch_ids"] = json!(change.directories);
                }
                result
            })
            .collect::<Vec<_>>();
        self.plan.result = json!({"file_id": self.input.file_id, "nodes": nodes,
        "content_versions_created": "0", "source_request": self.input.body(),
        "metadata_created_at": self.created_at.to_string(),
        "namespace_tags": {
            "registry": self.view.registry,
            "activation_sequence": self.view.activation_sequence.to_string(),
            "mask_hash": self.view.mask_hash,
            "source_snapshot": self.view.source_snapshot,
            "source_sequence": self.view.source_sequence.to_string(),
        }});
        if let Some(branch) = self
            .plan
            .branches
            .values()
            .filter_map(Option::as_ref)
            .find(|branch| branch.file_id.is_none())
        {
            self.plan.result["branch_id"] = json!(branch.branch_id);
            self.plan.result["history_tree_id"] = json!(branch.history_tree_id);
            self.plan.result["commit_id"] = json!(branch.head_commit_id);
        }
        Ok(self.plan)
    }

    fn preflight(&mut self) -> Result<()> {
        let input = self.input;
        if input.targets.is_empty()
            || input.targets.len() > self.maximum
            || input.add.len() > MAX_FILE_TAGS
            || input.remove.len() > MAX_FILE_TAGS
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "a bounded nonempty tag target list is required",
            ));
        }
        for tag in input.add.iter().chain(&input.remove) {
            crate::validate_tag(tag)?;
        }
        let added: BTreeSet<_> = input.add.iter().cloned().collect();
        let removed: BTreeSet<_> = input.remove.iter().cloned().collect();
        if !added.is_disjoint(&removed) {
            return Err(Error::new(
                ErrorCode::InvalidTag,
                "a tag cannot be added and removed in one operation",
            ));
        }
        let mut seen = BTreeSet::new();
        for target in &input.targets {
            if !seen.insert((&target.branch_id, &target.source_node_id)) {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "duplicate tag target",
                ));
            }
            let branch = self
                .previous
                .branches
                .get(&target.branch_id)
                .filter(|branch| {
                    branch.branch_kind == BranchKind::File
                        && branch.file_id.as_deref() == Some(input.file_id.as_str())
                })
                .ok_or_else(|| {
                    Error::new(ErrorCode::FileBranchNotFound, "tag target branch missing")
                })?;
            let history = self
                .previous
                .histories
                .get(&branch.history_tree_id)
                .ok_or_else(invalid)?;
            let repository = self
                .previous
                .repositories
                .get(&history.repository_id)
                .ok_or_else(invalid)?;
            let head_commit = self
                .previous
                .commits
                .get(&branch.head_commit_id)
                .ok_or_else(invalid)?;
            let fork = self
                .previous
                .commits
                .get(&branch.fork_commit_id)
                .ok_or_else(invalid)?;
            let scope = self
                .view
                .branches
                .get(&target.branch_id)
                .ok_or_else(stale)?;
            if branch.branch_id != target.branch_id
                || history.history_tree_id != branch.history_tree_id
                || repository.repository_id != history.repository_id
                || repository.history_tree_id != branch.history_tree_id
                || head_commit.commit_id != branch.head_commit_id
                || head_commit.history_tree_id != branch.history_tree_id
                || fork.commit_id != branch.fork_commit_id
                || fork.history_tree_id != branch.history_tree_id
                || scope.head != branch.head_commit_id
                || self.changes.first().is_some_and(|first| {
                    self.previous.branches[&first.branch].history_tree_id != branch.history_tree_id
                })
            {
                return Err(invalid());
            }
            if branch.state != BranchState::Active
                || self
                    .previous
                    .file_branch_aliases
                    .contains_key(&target.branch_id)
            {
                return Err(Error::new(
                    ErrorCode::BranchNotActive,
                    "merged aliases are read-only",
                ));
            }
            let head = branch
                .file_node_id
                .clone()
                .or_else(|| {
                    head_commit
                        .entries
                        .iter()
                        .find(|entry| {
                            entry.kind == NamespaceEntryKind::File && entry.file_id == input.file_id
                        })
                        .map(|entry| entry.version_id.clone())
                })
                .ok_or_else(invalid)?;
            checked_node_version(
                self.previous,
                &input.file_id,
                &head,
                &branch.history_tree_id,
            )?;
            if head != target.expected_node_id {
                return Err(Error::new(
                    ErrorCode::FileBranchConflict,
                    "tag target head changed",
                ));
            }
            let version = checked_node_version(
                self.previous,
                &input.file_id,
                &target.source_node_id,
                &branch.history_tree_id,
            )?;
            if !crate::business::file_nodes::belongs_to_branch(
                self.previous,
                &target.branch_id,
                &head,
                &target.source_node_id,
            ) {
                return Err(Error::new(
                    ErrorCode::FileNodeNotFound,
                    "tag target node is outside the selected branch",
                ));
            }
            self.location(repository)?;
            let source = if head == target.source_node_id {
                Some(self.current_file_source(branch, repository, &version)?)
            } else {
                None
            };
            let old = crate::business::file_nodes::node_tags(self.previous, &target.source_node_id);
            let mut tags = old.clone();
            tags.retain(|tag| !removed.contains(tag));
            tags.extend(added.iter().cloned());
            if tags.len() > MAX_FILE_TAGS {
                return Err(Error::new(
                    ErrorCode::TagLimitExceeded,
                    "a file node supports at most 16 tags",
                ));
            }
            let entries = source.filter(|_| tags != old).map(<[Entry]>::to_vec);
            self.changes.push(Change {
                branch: target.branch_id.clone(),
                source: target.source_node_id.clone(),
                node: target.source_node_id.clone(),
                version,
                changed: tags != old,
                entries,
                tags,
                advance: head == target.source_node_id,
                directories: BTreeSet::new(),
            });
        }
        Ok(())
    }

    fn location(&self, repository: &Repository) -> Result<()> {
        let Some(location) = &self.input.location else {
            return Ok(());
        };
        name_policy::validate_path(&location.path)?;
        let head = &self.previous.branches[&repository.main_branch_id].head_commit_id;
        let entries = self.view.branch_entries(&repository.main_branch_id, head)?;
        if location.repository_id != repository.repository_id
            || !at_key(entries, &name_policy::path_key(&location.path)).is_some_and(|entry| {
                entry.kind == NamespaceEntryKind::File && entry.file_id == self.input.file_id
            })
        {
            return Err(missing());
        }
        Ok(())
    }

    fn current_file_source(
        &self,
        branch: &Branch,
        repository: &Repository,
        version: &str,
    ) -> Result<&[Entry]> {
        let source = self.view.writable_scope(self.previous, &branch.branch_id)?;
        let owner = self
            .view
            .writable_scope(self.previous, &repository.main_branch_id)?;
        let mut files = 0;
        let mut ancestors = BTreeSet::new();
        for entry in source
            .entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
        {
            if entry.file_id != self.input.file_id
                || entry.version_id != version
                || entry.size != self.previous.versions[version].logical_size
                || !at_key(&owner.entries, entry.lookup_key())
                    .is_some_and(|owner| same_occurrence(owner, entry))
                || self.input.location.as_ref().is_some_and(|location| {
                    name_policy::path_key(&location.path) != entry.lookup_key()
                })
            {
                return Err(missing());
            }
            files += 1;
            let mut key = entry.lookup_key();
            while let Some((parent, _)) = key.rsplit_once('/') {
                ancestors.insert(parent);
                key = parent;
            }
        }
        if files == 0
            || source.entries.len() != files + ancestors.len()
            || source.entries.iter().any(|entry| {
                entry.kind == NamespaceEntryKind::Directory
                    && (!ancestors.contains(entry.lookup_key())
                        || at_key(&owner.entries, entry.lookup_key()) != Some(entry))
            })
        {
            return Err(missing());
        }
        Ok(&source.entries)
    }
}

fn checked_node_version(state: &State, file: &str, node: &str, history: &str) -> Result<String> {
    let (id, owner, source_history, version) = match (
        state.versions.get(node),
        state.file_path_nodes.get(node),
        state.file_metadata_nodes.get(node),
    ) {
        (Some(body), None, None) => (
            &body.version_id,
            &body.file_id,
            &body.history_tree_id,
            &body.version_id,
        ),
        (None, Some(path), None) => (
            &path.node_id,
            &path.file_id,
            &path.history_tree_id,
            &path.version_id,
        ),
        (None, None, Some(metadata)) => (
            &metadata.node_id,
            &metadata.file_id,
            &metadata.history_tree_id,
            &metadata.version_id,
        ),
        (None, None, None) => {
            return Err(Error::new(
                ErrorCode::FileNodeNotFound,
                "tag source node is missing",
            ));
        }
        _ => return Err(invalid()),
    };
    if owner != file || source_history != history {
        return Err(Error::new(
            ErrorCode::FileNodeNotFound,
            "tag source node escapes its file or history",
        ));
    }
    let body = state.versions.get(version).ok_or_else(invalid)?;
    if id != node
        || body.version_id != *version
        || body.file_id != file
        || body.history_tree_id != history
    {
        return Err(invalid());
    }
    Ok(version.clone())
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_file_tags_tests.rs"]
mod tests;

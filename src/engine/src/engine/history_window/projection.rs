//! Copy only admitted small scalar facts after stable-key page selection.
use super::*;
use nodes::Node;
use query::Query;
use std::borrow::Cow;

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/history_window_projection_tests.rs"]
mod tests;

impl<'a> Query<'a> {
    pub(super) fn node(&self, id: &str) -> Result<Node<'a>> {
        nodes::node(self.state, self.scope(), id).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::FileNodeNotFound,
                "node is outside this file and history",
            )
        })
    }

    pub(super) fn allowed_version(&self, id: &str) -> bool {
        self.state.versions.get(id).is_some_and(|version| {
            version.file_id == self.scope().file && version.history_tree_id == self.scope().history
        })
    }

    pub(super) fn version(&self, id: &str) -> Result<&'a crate::FileVersion> {
        self.state
            .versions
            .get(id)
            .filter(|_| self.allowed_version(id))
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::VersionNotFound,
                    "version is outside this file and history",
                )
            })
    }

    pub(super) fn allowed_branch(&self, id: &str) -> bool {
        self.state.branches.get(id).is_some_and(|branch| {
            branch.file_id.as_deref() == Some(self.scope().file)
                && branch.history_tree_id == self.scope().history
        })
    }

    pub(super) fn merge(&self, id: &str) -> Result<&'a crate::FileBranchMerge> {
        self.state
            .file_branch_merges
            .get(id)
            .filter(|merge| {
                merge.file_id == self.scope().file && merge.history_tree_id == self.scope().history
            })
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::NotFound,
                    "merge is outside this file and history",
                )
            })
    }

    pub(super) fn field(&self, node: Node<'a>, field: HistoryField) -> Result<Cow<'a, str>> {
        self.cancel.checkpoint()?;
        Ok(match node {
            Node::Content(version) => match field {
                HistoryField::CreatedAt => Cow::Borrowed(&version.created_at),
                HistoryField::Message | HistoryField::Author => {
                    let commit = nodes::first_commit(
                        self.state,
                        self.scope(),
                        &version.version_id,
                        self.cancel,
                    )?;
                    Cow::Borrowed(match field {
                        HistoryField::Message => {
                            commit.map_or("", |commit| commit.message.as_str())
                        }
                        HistoryField::Author => commit.map_or("", |commit| commit.author.as_str()),
                        HistoryField::CreatedAt => {
                            unreachable!("invariant: creation time was matched independently")
                        }
                    })
                }
            },
            Node::Path(node) => match field {
                HistoryField::Message => Cow::Borrowed(""),
                HistoryField::Author => Cow::Borrowed(&node.author),
                HistoryField::CreatedAt => Cow::Owned(created_at(node.created_at)?),
            },
            Node::Metadata(node) => match field {
                HistoryField::Message => Cow::Borrowed(""),
                HistoryField::Author => Cow::Borrowed(&node.author),
                HistoryField::CreatedAt => Cow::Owned(created_at(node.created_at)?),
            },
        })
    }

    fn fields(&self, node: Node<'a>) -> Result<HistoryNodeFields> {
        if let Node::Content(version) = node {
            let commit =
                nodes::first_commit(self.state, self.scope(), &version.version_id, self.cancel)?;
            return content_fields(version, commit, self.cancel);
        }
        Ok(HistoryNodeFields {
            message: HistoryFieldInfo {
                byte_length: u64::try_from(self.field(node, HistoryField::Message)?.len())?,
            },
            author: HistoryFieldInfo {
                byte_length: u64::try_from(self.field(node, HistoryField::Author)?.len())?,
            },
            created_at: HistoryFieldInfo {
                byte_length: u64::try_from(self.field(node, HistoryField::CreatedAt)?.len())?,
            },
        })
    }

    fn download_name(&self, version: &'a crate::FileVersion) -> &str {
        self.context
            .path
            .as_ref()
            .map(|path| {
                path.as_str()
                    .rsplit('/')
                    .next()
                    .expect("invariant: portable paths have a final component")
            })
            .or_else(|| {
                self.state
                    .file_names
                    .get(self.scope().file)
                    .map(String::as_str)
            })
            .unwrap_or(&version.creation_name)
    }

    fn content_branch(&self, version: &str) -> Result<Option<HistoryBranchId>> {
        for branch in self.state.branches.values() {
            self.cancel.checkpoint()?;
            if !self.allowed_branch(&branch.branch_id) {
                continue;
            }
            if let Some(commit) = self.state.commits.get(&branch.head_commit_id) {
                if branch.pending_file_versions.contains_key(self.scope().file)
                    || commit.pending_file_versions.contains_key(self.scope().file)
                {
                    continue;
                }
                for entry in &commit.entries {
                    self.cancel.checkpoint()?;
                    if entry.file_id == self.scope().file && entry.version_id == version {
                        return Ok(Some(HistoryBranchId::parse(&branch.branch_id)?));
                    }
                }
            }
        }
        Ok(None)
    }

    pub(super) fn node_summary(
        &self,
        node: Node<'a>,
        commit: Option<&'a crate::Commit>,
        parents: HistoryParentSet,
    ) -> Result<HistoryNodeSummary> {
        self.cancel.checkpoint()?;
        let version = node.version(self.state);
        let (kind, branch_id, previous_path, note) = match node {
            Node::Content(version) => (
                HistoryNodeKind::Content,
                self.content_branch(&version.version_id)?,
                None,
                self.state.version_notes.get(&version.version_id),
            ),
            Node::Path(node) => (
                HistoryNodeKind::Rename,
                Some(HistoryBranchId::parse(&node.branch_id)?),
                Some(node.previous_path.clone()),
                self.state.file_path_notes.get(&node.node_id),
            ),
            Node::Metadata(node) => (
                match node.kind {
                    crate::FileMetadataKind::Tags => HistoryNodeKind::Tags,
                    crate::FileMetadataKind::Merge => HistoryNodeKind::Merge,
                },
                Some(HistoryBranchId::parse(&node.branch_id)?),
                None,
                self.state.file_path_notes.get(&node.node_id),
            ),
        };
        Ok(HistoryNodeSummary {
            node_id: HistoryNodeId::parse(node.id())?,
            version_id: HistoryVersionId::parse(&version.version_id)?,
            kind,
            sequence: node.sequence(commit),
            current: self
                .context
                .current_node_id
                .as_ref()
                .is_some_and(|current| current.as_str() == node.id()),
            branch_id,
            path: match node {
                Node::Content(version) => {
                    nodes::content_path(self.scope(), version, commit, self.cancel)?
                }
                Node::Path(_) | Node::Metadata(_) => {
                    nodes::path(self.state, self.scope(), node, self.cancel)?
                }
            }
            .into(),
            previous_path,
            note: note.map_or("", String::as_str).into(),
            tags: nodes::tags(self.state, self.scope(), node.id(), self.cancel)?
                .into_iter()
                .flatten()
                .cloned()
                .collect(),
            size: version.logical_size,
            content_hash: version.content_hash,
            content_state: self.state.content_states.get(&version.version_id).copied(),
            download_name: self.download_name(version).into(),
            storage_name: version.storage_name.clone(),
            parents,
            fields: match node {
                Node::Content(version) => content_fields(version, commit, self.cancel)?,
                Node::Path(_) | Node::Metadata(_) => self.fields(node)?,
            },
        })
    }

    pub(super) fn version_summary(
        &self,
        version: &'a crate::FileVersion,
    ) -> Result<HistoryVersionSummary> {
        self.cancel.checkpoint()?;
        let mut sources = 0usize;
        for source in &version.source_version_ids {
            self.cancel.checkpoint()?;
            sources += usize::from(self.allowed_version(source));
        }
        Ok(HistoryVersionSummary {
            version_id: HistoryVersionId::parse(&version.version_id)?,
            version_sequence: version.version_sequence,
            size: version.logical_size,
            content_hash: version.content_hash,
            content_state: self.state.content_states.get(&version.version_id).copied(),
            note: self
                .state
                .version_notes
                .get(&version.version_id)
                .map_or("", String::as_str)
                .into(),
            download_name: self.download_name(version).into(),
            storage_name: version.storage_name.clone(),
            source_count: u64::try_from(sources)?,
            fields: self.fields(Node::Content(version))?,
        })
    }

    fn content(
        &self,
        visible: Option<&str>,
        source: Option<&str>,
    ) -> Result<HistoryContentDisposition> {
        let selected = source.or(visible).filter(|id| !id.is_empty());
        if selected.is_some_and(|id| !self.allowed_version(id)) {
            return Err(Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "content disposition escapes this file or history",
            ));
        }
        Ok(HistoryContentDisposition {
            version_id: selected.map(HistoryVersionId::parse).transpose()?,
            source: match source {
                Some(source) => HistoryContentSource::Inherited {
                    source_version_id: if source.is_empty() {
                        None
                    } else {
                        Some(HistoryVersionId::parse(source)?)
                    },
                },
                None => HistoryContentSource::Owned,
            },
            stored_state: selected
                .and_then(|id| self.state.content_states.get(id))
                .copied(),
        })
    }

    pub(super) fn branch_summary(&self, branch: &'a crate::Branch) -> Result<HistoryBranchSummary> {
        self.cancel.checkpoint()?;
        let commit = self
            .state
            .commits
            .get(&branch.head_commit_id)
            .expect("invariant: admitted branch head has a commit");
        let mut visible = None;
        for entry in &commit.entries {
            self.cancel.checkpoint()?;
            if entry.kind == crate::NamespaceEntryKind::File && entry.file_id == self.scope().file {
                visible = Some(entry.version_id.as_str());
                break;
            }
        }
        let source = branch
            .pending_file_versions
            .get(self.scope().file)
            .or_else(|| commit.pending_file_versions.get(self.scope().file))
            .map(String::as_str);
        let head = branch.file_node_id.as_deref().or(visible);
        if let Some(head) = head {
            self.node(head)?;
        }
        let alias_of = self
            .state
            .file_branch_aliases
            .get(&branch.branch_id)
            .map(|id| HistoryBranchId::parse(id))
            .transpose()?;
        Ok(HistoryBranchSummary {
            branch_id: HistoryBranchId::parse(&branch.branch_id)?,
            name: branch.name.clone(),
            head_commit_id: HistoryCommitId::parse(&branch.head_commit_id)?,
            fork_commit_id: HistoryCommitId::parse(&branch.fork_commit_id)?,
            node_id: head.map(HistoryNodeId::parse).transpose()?,
            state: branch.state,
            branch_kind: branch.branch_kind,
            generation: branch.generation,
            current: self
                .context
                .selected_branch_id
                .as_ref()
                .is_some_and(|selected| selected.as_str() == branch.branch_id),
            read_only: branch.state != crate::BranchState::Active || alias_of.is_some(),
            alias_of,
            content: self.content(visible, source)?,
        })
    }

    pub(super) fn merge_summary(
        &self,
        merge: &crate::FileBranchMerge,
    ) -> Result<HistoryMergeSummary> {
        self.cancel.checkpoint()?;
        assert_eq!(
            merge.input_branch_ids.len(),
            merge.input_node_ids.len(),
            "invariant: admitted merge input pairs have equal length"
        );
        let mut inputs = 0usize;
        for (branch, node) in merge.input_branch_ids.iter().zip(&merge.input_node_ids) {
            self.cancel.checkpoint()?;
            inputs += usize::from(
                self.allowed_branch(branch)
                    && nodes::node(self.state, self.scope(), node).is_some(),
            );
        }
        let mut parents = 0usize;
        for parent in &merge.parent_node_ids {
            self.cancel.checkpoint()?;
            parents += usize::from(nodes::node(self.state, self.scope(), parent).is_some());
        }
        Ok(HistoryMergeSummary {
            merge_id: HistoryMergeId::parse(&merge.merge_id)?,
            sequence: merge.sequence,
            chosen_node_id: HistoryNodeId::parse(&merge.chosen_node_id)?,
            result_node_id: HistoryNodeId::parse(&merge.result_node_id)?,
            surviving_branch_id: HistoryBranchId::parse(&merge.surviving_branch_id)?,
            reused_node: merge.reused_node,
            operation_id: merge.operation_id,
            input_count: u64::try_from(inputs)?,
            parent_count: u64::try_from(parents)?,
        })
    }

    pub(super) fn context_summary(&self) -> Result<HistoryContextSummary> {
        let content = if let Some(path) = &self.context.path {
            let repository = &self.state.repositories[self.context.repository_id.as_str()];
            let branch = &self.state.branches[&repository.main_branch_id];
            let commit = &self.state.commits[&branch.head_commit_id];
            let entry = commit
                .entries
                .binary_search_by(|entry| entry.lookup_key().cmp(path.key()))
                .ok()
                .map(|index| &commit.entries[index])
                .filter(|entry| {
                    entry.kind == crate::NamespaceEntryKind::File
                        && entry.file_id == self.scope().file
                })
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::FileNotFound,
                        "current file path is absent from metadata observation",
                    )
                })?;
            let key = crate::business::context_key_from_key(
                &repository.repository_id,
                entry.lookup_key(),
            );
            let context = self.state.file_contexts.get(&key);
            let inherited = context.map_or_else(
                || {
                    self.state
                        .file_write_origins
                        .contains_key(self.scope().file)
                },
                |context| context.inherited_from.is_some(),
            );
            let source = if inherited {
                Some(entry.version_id.as_str())
            } else {
                self.context
                    .selected_branch_id
                    .as_ref()
                    .and_then(|branch| self.state.branches.get(branch.as_str()))
                    .and_then(|branch| branch.pending_file_versions.get(self.scope().file))
                    .map(String::as_str)
            };
            Some(self.content(Some(&entry.version_id), source)?)
        } else {
            None
        };
        Ok(HistoryContextSummary {
            note: self
                .state
                .file_notes
                .get(self.scope().file)
                .map_or("", String::as_str)
                .into(),
            content,
        })
    }
}

// Only the selected version and its resolved first commit are available here;
// field length projection cannot iterate the complete commit collection.
fn content_fields(
    version: &crate::FileVersion,
    commit: Option<&crate::Commit>,
    cancel: &HistoryWindowCancellation,
) -> Result<HistoryNodeFields> {
    cancel.checkpoint()?;
    Ok(HistoryNodeFields {
        message: HistoryFieldInfo {
            byte_length: u64::try_from(commit.map_or(0, |commit| commit.message.len()))?,
        },
        author: HistoryFieldInfo {
            byte_length: u64::try_from(commit.map_or(0, |commit| commit.author.len()))?,
        },
        created_at: HistoryFieldInfo {
            byte_length: u64::try_from(version.created_at.len())?,
        },
    })
}

fn created_at(timestamp: u64) -> Result<String> {
    let seconds = i64::try_from(timestamp).map_err(|source| {
        Error::from(source).context(
            storage_contract::ErrorCode::StateInvalid,
            "file node creation timestamp is outside the UTC range",
        )
    })?;
    Ok(date_time::OffsetDateTime::from_unix_timestamp(seconds)?
        .format(&date_time::format_description::well_known::Rfc3339)?)
}

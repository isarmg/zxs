//! Read-only directory history over one exact metadata root. Retained rows are
//! O(limit); cold selection scans directory headers with cancellation checkpoints.
use super::history_selection::{Position, Selection, Selector};
use super::history_window::HistoryCancellationCheckpoint;
use super::*;
use std::cmp::Reverse;
use storage_contract::directory_history::*;
use storage_contract::directory_staging::directory_tree_is_committed;
use storage_contract::history_window::{HistoryEndpoint, HistoryRoot};

type Key<'a> = (Reverse<u64>, &'a str);

pub struct DirectoryHistoryRead {
    view: MetadataView,
    request: DirectoryHistoryRequest,
    cancellation: HistoryWindowCancellation,
    identity: DirectoryHistoryIdentity,
    current: DirectoryHistoryCurrent,
}
fn changed() -> Error {
    Error::new(
        ErrorCode::HistoryChanged,
        "directory history changed; refresh its current window",
    )
}
fn missing() -> Error {
    Error::new(
        ErrorCode::DirectoryNotFound,
        "directory identity is absent from this repository",
    )
}
fn owns(branch: &DirectoryBranch, request: &DirectoryHistoryRequest) -> bool {
    branch.repository_id == request.repository_id() && branch.directory_id == request.directory_id()
}
fn committed_ancestor<'a>(
    state: &'a State,
    tree: &'a DirectoryTree,
    cancel: &HistoryWindowCancellation,
) -> Result<Option<&'a DirectoryTree>> {
    let mut cursor = Some(tree);
    while let Some(tree) = cursor {
        cancel.checkpoint()?;
        if directory_tree_is_committed(state, tree) {
            return Ok(Some(tree));
        }
        cursor = tree
            .parent_tree_id
            .as_ref()
            .and_then(|id| state.directory_trees.get(id));
    }
    Ok(None)
}
fn resolve(
    view: &MetadataView,
    request: &DirectoryHistoryRequest,
    cancel: &HistoryWindowCancellation,
) -> Result<(DirectoryHistoryIdentity, DirectoryHistoryCurrent)> {
    cancel.checkpoint()?;
    let state = view.business_projection();
    let repo = state
        .repositories
        .get(request.repository_id())
        .ok_or_else(|| Error::new(ErrorCode::RepositoryNotFound, "repository not found"))?;
    let root = HistoryRoot::parse(view.history_root(&repo.history_tree_id)?)?;
    if request
        .expected_root()
        .is_some_and(|expected| expected != root)
    {
        return Err(changed());
    }
    let namespaces = view.current_namespaces.admitted()?;
    let entries = crate::business::directory_git::current_entries(
        state,
        namespaces,
        request.repository_id(),
    )?;
    let mut present = request.directory_id() == request.repository_id();
    if !present {
        for entry in entries {
            cancel.checkpoint()?;
            if entry.kind == NamespaceEntryKind::Directory
                && entry.entry_id == request.directory_id()
            {
                present = true;
                break;
            }
        }
    }
    if !present {
        let mut retained = false;
        for branch in state.directory_branches.values() {
            cancel.checkpoint()?;
            if owns(branch, request) {
                retained = true;
                break;
            }
        }
        if !retained {
            return Err(missing());
        }
    }
    let current = if let Some(git) = present
        .then(|| state.directory_git_repositories.get(request.directory_id()))
        .flatten()
    {
        let branch = &state.directory_branches[&git.current_branch_id];
        let status = crate::business::directory_git::current_changes(
            state,
            namespaces,
            request.repository_id(),
            &branch.path,
            cancel,
        )?;
        DirectoryHistoryCurrent {
            directory_branch_id: Some(git.current_branch_id.clone()),
            tree_id: Some(branch.head_tree_id.clone()),
            committed_tree_id: git.branches[&git.current_branch_id].head_tree_id.clone(),
            staged: status.staged || status.needs_restage,
        }
    } else {
        DirectoryHistoryCurrent {
            directory_branch_id: None,
            tree_id: None,
            committed_tree_id: None,
            staged: false,
        }
    };
    Ok((
        DirectoryHistoryIdentity {
            repository_id: repo.repository_id.clone(),
            directory_id: request.directory_id().into(),
            history_tree_id: repo.history_tree_id.clone(),
            history_root: root,
            collection: request.collection(),
            limit: request.limit(),
        },
        current,
    ))
}
impl Engine {
    pub fn capture_directory_history(
        &self,
        request: DirectoryHistoryRequest,
        cancellation: HistoryWindowCancellation,
    ) -> Result<DirectoryHistoryRead> {
        let view = self.metadata_view();
        let (identity, current) = resolve(&view, &request, &cancellation)?;
        Ok(DirectoryHistoryRead {
            view,
            request,
            cancellation,
            identity,
            current,
        })
    }
    pub fn recheck_directory_history(&self, read: &DirectoryHistoryRead) -> Result<()> {
        let (identity, current) =
            resolve(&self.metadata_view(), &read.request, &read.cancellation)?;
        if identity != read.identity || current != read.current {
            return Err(changed());
        }
        Ok(())
    }
}
impl DirectoryHistoryRead {
    fn selector<'a, T>(&'a self) -> Result<Selector<Key<'a>, T>> {
        let position = if let Some(cursor) = self.request.cursor() {
            let key = (Reverse(cursor.sequence()), cursor.id());
            match cursor.direction() {
                DirectoryHistoryDirection::Before => Position::Before(key),
                DirectoryHistoryDirection::After => Position::After(key),
            }
        } else if let Some(anchor) = self.request.anchor_tree_id() {
            let state = self.view.business_projection();
            let tree = state
                .directory_trees
                .get(anchor)
                .filter(|tree| {
                    directory_tree_is_committed(state, tree)
                        && owns(
                            &state.directory_branches[&tree.directory_branch_id],
                            &self.request,
                        )
                })
                .ok_or_else(missing)?;
            Position::At((Reverse(tree.sequence), tree.tree_id.as_str()))
        } else {
            Position::Start
        };
        Selector::new(position, self.request.limit() as usize)
    }
    fn cursors<T>(&self, selected: &Selection<Key<'_>, T>) -> Result<DirectoryHistoryPagination> {
        let cursor = |key: &Key<'_>, direction| {
            self.request
                .page_cursor(self.identity.history_root, key.0.0, key.1, direction)
        };
        Ok(DirectoryHistoryPagination {
            previous_cursor: if selected.has_before {
                selected
                    .entries
                    .first()
                    .map(|(key, _)| cursor(key, DirectoryHistoryDirection::Before))
                    .transpose()?
            } else {
                None
            },
            next_cursor: if selected.has_after {
                selected
                    .entries
                    .last()
                    .map(|(key, _)| cursor(key, DirectoryHistoryDirection::After))
                    .transpose()?
            } else {
                None
            },
        })
    }
    pub fn page(&self) -> Result<DirectoryHistoryPage> {
        self.cancellation.checkpoint()?;
        let state = self.view.business_projection();
        let mut page = DirectoryHistoryPage {
            identity: self.identity.clone(),
            current: self.current.clone(),
            nodes: Vec::new(),
            branches: Vec::new(),
            page: DirectoryHistoryPagination {
                previous_cursor: None,
                next_cursor: None,
            },
        };
        match self.request.collection() {
            DirectoryHistoryCollection::Nodes => {
                let mut selector = self.selector()?;
                for tree in state.directory_trees.values() {
                    self.cancellation.checkpoint()?;
                    if storage_contract::directory_staging::directory_tree_is_committed(state, tree)
                        && owns(
                            &state.directory_branches[&tree.directory_branch_id],
                            &self.request,
                        )
                    {
                        selector.offer((Reverse(tree.sequence), tree.tree_id.as_str()), tree);
                    }
                }
                let selected = selector.finish()?;
                page.page = self.cursors(&selected)?;
                let ids = selected
                    .entries
                    .iter()
                    .map(|(_, tree)| tree.tree_id.as_str())
                    .collect::<BTreeSet<_>>();
                for (_, tree) in &selected.entries {
                    self.cancellation.checkpoint()?;
                    let parent = match tree
                        .parent_tree_id
                        .as_ref()
                        .and_then(|id| state.directory_trees.get(id))
                    {
                        Some(parent) => committed_ancestor(state, parent, &self.cancellation)?,
                        None => None,
                    };
                    page.nodes.push(DirectoryHistoryNode {
                        message: state.commits[&tree.commit_id].message.clone(),
                        tree_id: tree.tree_id.clone(),
                        directory_branch_id: tree.directory_branch_id.clone(),
                        parent_tree_id: parent.map(|tree| tree.tree_id.clone()),
                        parent_endpoint: parent.map(|tree| {
                            if ids.contains(tree.tree_id.as_str()) {
                                HistoryEndpoint::InWindow
                            } else {
                                HistoryEndpoint::OutsideWindow
                            }
                        }),
                        commit_id: tree.commit_id.clone(),
                        path: tree.path.clone(),
                        sequence: tree.sequence,
                    });
                }
            }
            DirectoryHistoryCollection::Branches => {
                let mut selector = self.selector()?;
                for branch in state.directory_branches.values() {
                    self.cancellation.checkpoint()?;
                    if owns(branch, &self.request)
                        && state
                            .directory_git_repositories
                            .get(&branch.directory_id)
                            .is_some_and(|git| {
                                git.branches.contains_key(&branch.directory_branch_id)
                            })
                    {
                        selector.offer(
                            (
                                Reverse(branch.sequence),
                                branch.directory_branch_id.as_str(),
                            ),
                            branch,
                        );
                    }
                }
                let selected = selector.finish()?;
                page.page = self.cursors(&selected)?;
                let mut statuses = crate::business::directory_git::BranchStatuses::new(
                    state,
                    self.view.current_namespaces.admitted()?,
                    self.request.repository_id(),
                    &self.cancellation,
                )?;
                for (_, branch) in selected.entries {
                    let git = &state.directory_git_repositories[&branch.directory_id];
                    let index = &git.branches[&branch.directory_branch_id];
                    let staged = if self.current.directory_branch_id.as_deref()
                        == Some(branch.directory_branch_id.as_str())
                    {
                        self.current.staged
                    } else {
                        statuses.staged(&branch.directory_branch_id)?
                    };
                    page.branches.push(DirectoryHistoryBranch {
                        committed_tree_id: index.head_tree_id.clone(),
                        staged,
                        directory_branch_id: branch.directory_branch_id.clone(),
                        name: branch.name.clone(),
                        parent_branch_id: branch.parent_branch_id.clone(),
                        head_tree_id: branch.head_tree_id.clone(),
                        materialized: branch.materialized,
                        current: self.current.directory_branch_id.as_deref()
                            == Some(branch.directory_branch_id.as_str()),
                        sequence: branch.sequence,
                    });
                }
            }
        }
        page.validate(&self.request)?;
        self.cancellation.checkpoint()?;
        Ok(page)
    }
}

//! Request-local namespace projections. Whole-tree work uses one detached
//! publication; only a complete bounded encoded response leaves that reader.
use super::wire::{PendingContexts, PendingResolution};
use super::{error, repo_for_branch};
use crate::{ApiError, DirectorySnapshot, TreeSnapshot};
use std::collections::BTreeMap;
use storage_engine::{Engine, PublishedReadView};

pub(super) fn capture_tree(
    view: PublishedReadView,
    request: crate::ReadTree<'_>,
    control: &crate::HistoryReadControl,
) -> Result<TreeSnapshot, ApiError> {
    let branch = request.branch;
    let (commit, snapshot) = branch_snapshot(view.state(), branch, request.commit)?;
    super::file_manager::activity::checkpoint(control)?;
    let observe = || control.checkpoint().map_err(storage_engine::Error::from);
    let tree = view
        .commit_namespace(commit, &observe)
        .expect("invariant: admitted tree commit exists in captured publication");
    let mut encoder = crate::backend_read::TreeEncoder::new(branch, commit)?;
    let mut files = Vec::new();
    let mut encoding_error = None;
    tree.walk(|entry| {
        control.checkpoint()?;
        if let Err(error) = encoder.entry(entry) {
            encoding_error = Some(error);
            return Ok(false);
        }
        if entry.kind == storage_contract::NamespaceEntryKind::File {
            // Every retained entry has already fit the encoded response budget.
            files.push(entry.clone());
        }
        Ok(true)
    })
    .map_err(super::file_manager::activity::read_error)?;
    if let Some(error) = encoding_error {
        return Err(error);
    }

    // Preserve namespace failures before pending-source resolution. Keep only
    // target-key contexts, including a first match with no inherited source.
    let source = PendingResolution::new(view.state(), branch, snapshot);
    let historical = historical_contexts(&view, &files, snapshot, source.historical(), control)?;
    let mut pending = BTreeMap::new();
    for entry in &files {
        super::file_manager::activity::checkpoint(control)?;
        let context = source
            .main_repository
            .and_then(|repository| view.file_context_for_current_entry(repository, entry));
        let contexts = PendingContexts {
            current: context.as_deref(),
            historical: historical.get(entry.lookup_key()).copied().flatten(),
        };
        if let Some(version) = source.resolve(entry, entry, contexts, || {
            view.pending_file_source(branch, &entry.file_id)
        }) {
            // The original canonical walk overwrote only for Some(source).
            pending.insert(entry.file_id.as_str(), version);
        }
    }
    super::file_manager::activity::checkpoint(control)?;
    encoder.finish(&pending)
}

fn historical_contexts<'a>(
    view: &'a PublishedReadView,
    files: &'a [storage_contract::Entry],
    snapshot: &storage_contract::Commit,
    historical: bool,
    control: &crate::HistoryReadControl,
) -> Result<BTreeMap<&'a str, Option<&'a storage_contract::FileContext>>, ApiError> {
    if !historical {
        return Ok(BTreeMap::new());
    }
    let mut contexts: BTreeMap<_, Option<&storage_contract::FileContext>> = BTreeMap::new();
    for entry in files {
        super::file_manager::activity::checkpoint(control)?;
        contexts.insert(entry.lookup_key(), None);
    }
    for tree in view.state().directory_trees.values() {
        super::file_manager::activity::checkpoint(control)?;
        if tree.commit_id != snapshot.commit_id {
            continue;
        }
        for (key, context) in &tree.file_contexts {
            super::file_manager::activity::checkpoint(control)?;
            if let Some(selected) = contexts.get_mut(key.as_str()) {
                selected.get_or_insert(context);
            }
        }
    }
    Ok(contexts)
}

pub(super) fn resolve_entry(
    engine: &Engine,
    request: crate::ResolveEntry<'_>,
) -> Result<crate::ResolvedEntry, ApiError> {
    let (commit_id, commit) = branch_snapshot(engine.state(), request.branch, request.commit)?;
    let tree = engine
        .namespace_tree_at(request.branch, commit_id)
        .map_err(error)?;
    let entry = tree
        .get_key(request.path.key())
        .map_err(error)?
        .ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "entry not found",
            )
        })?;
    let content = if entry.kind == storage_contract::NamespaceEntryKind::File {
        Some(super::branch::capture_content(
            engine.state(),
            Some(&entry.version_id),
            super::wire::pending_entry_source(engine, request.branch, commit, &entry)
                .map_err(error)?
                .as_deref(),
        ))
    } else {
        None
    };
    Ok(crate::ResolvedEntry {
        branch_id: request.branch.to_owned(),
        commit_id: commit_id.to_owned(),
        version: engine.state().versions.get(&entry.version_id).cloned(),
        entry,
        content,
    })
}

/// The same branch/head checks as Tree, without inspecting or copying entries.
/// Uses the current locked State or a coherently admitted immutable read view;
/// neither the selected commit nor the view is an authorization grant.
pub(super) fn branch_snapshot<'a>(
    state: &'a storage_contract::State,
    branch: &str,
    selected_commit: Option<&'a str>,
) -> Result<(&'a str, &'a storage_contract::Commit), ApiError> {
    super::repo_for_branch_in_state(state, branch)?;
    let branch_info = &state.branches[branch];
    let commit = selected_commit.unwrap_or(&branch_info.head_commit_id);
    let snapshot = state
        .commits
        .get(commit)
        .filter(|commit| commit.history_tree_id == branch_info.history_tree_id)
        .ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "commit not found",
            )
        })?;
    Ok((commit, snapshot))
}

/// Check metadata only; the caller has refreshed current identity under the same lock.
pub(super) fn authorize_current(
    state: &storage_contract::State,
    branch: &str,
) -> Result<(), ApiError> {
    branch_snapshot(state, branch, None)?;
    Ok(())
}

/// Copy only the current match; the caller holds current admission under the engine lock.
pub(super) fn lookup_current(
    engine: &Engine,
    branch: &str,
    path: &name_policy::LookupPath,
) -> Result<Option<storage_contract::Entry>, ApiError> {
    branch_snapshot(engine.state(), branch, None)?;
    let tree = engine.namespace_tree(branch).map_err(error)?;
    tree.get_key(path.key()).map_err(error)
}

pub(super) enum DirectoryRead {
    Captured(DirectorySnapshot),
    Page(DirectoryPagePlan),
}

/// At most one count-bounded namespace page. No engine or snapshot root escapes.
pub(super) struct DirectoryPagePlan {
    commit_id: String,
    children: Vec<storage_contract::Entry>,
}

impl DirectoryRead {
    pub(super) fn capture(
        engine: &Engine,
        request: crate::ReadDirectory<'_>,
    ) -> Result<Self, ApiError> {
        let crate::ReadDirectory {
            branch,
            entry_id,
            path,
            selection,
        } = request;
        repo_for_branch(engine, branch)?;
        let branch_info = engine.state().branches.get(branch).unwrap();
        let commit = match &selection {
            crate::DirectorySelection::Capture => &branch_info.head_commit_id,
            crate::DirectorySelection::Page { commit, .. } => {
                commit.unwrap_or(&branch_info.head_commit_id)
            }
        };
        let snapshot = engine
            .state()
            .commits
            .get(commit)
            .filter(|snapshot| snapshot.history_tree_id == branch_info.history_tree_id)
            .ok_or_else(|| {
                ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::NotFound,
                    "directory snapshot missing",
                )
            })?;
        let tree = engine.namespace_tree_at(branch, commit).map_err(error)?;
        let key = path.map_or("", |path| path.key());
        if path.is_none() {
            if entry_id != branch {
                return Err(ApiError::invalid("invalid root directory identity"));
            }
        } else {
            let directory = tree.get_key(key).map_err(error)?;
            if directory.is_none_or(|entry| {
                entry.kind != storage_contract::NamespaceEntryKind::Directory
                    || entry.entry_id != entry_id
            }) {
                return Err(ApiError::new(
                    ::http::StatusCode::CONFLICT,
                    storage_contract::ErrorCode::SmbRebindRequired,
                    "opened directory identity changed",
                ));
            }
        }
        match selection {
            crate::DirectorySelection::Capture => Ok(Self::Captured(DirectorySnapshot {
                commit_id: snapshot.commit_id.clone(),
                entries: Vec::new(),
                more: false,
            })),
            crate::DirectorySelection::Page { after, .. } => {
                DirectoryPagePlan::capture(&snapshot.commit_id, &tree, key, after).map(Self::Page)
            }
        }
    }
}

impl DirectoryPagePlan {
    fn capture(
        commit_id: &str,
        tree: &storage_engine::NamespaceTree<'_>,
        key: &str,
        after: Option<&str>,
    ) -> Result<Self, ApiError> {
        use smb_protocol::protocol::MAX_DIRECTORY_PAGE_ENTRIES;
        let prefix = if key.is_empty() {
            String::new()
        } else {
            format!("{key}/")
        };
        // Wire cursors carry the last returned entry's full lookup key; the
        // paged namespace positions by component key within one directory.
        // The cursor is located through the directory's index pages before
        // this page's entries are read; descendants are never enumerated.
        let after = after.map(|after| after.strip_prefix(prefix.as_str()).unwrap_or(after));
        let children = tree
            .directory_page(key, after, MAX_DIRECTORY_PAGE_ENTRIES + 1)
            .map_err(error)?
            .unwrap_or_default();
        Ok(Self {
            commit_id: commit_id.into(),
            children,
        })
    }

    pub(super) fn finish(self) -> Result<DirectorySnapshot, ApiError> {
        use smb_protocol::protocol::{
            MAX_DIRECTORY_PAGE_BYTES, MAX_DIRECTORY_PAGE_ENTRIES, Response,
        };
        let mut page = DirectorySnapshot {
            commit_id: self.commit_id,
            entries: Vec::new(),
            more: false,
        };
        // Reserve the larger internal response envelope. The wire strips the
        // commit id; false->true only reduces the encoded length.
        let mut bytes = json_codec::to_vec(&Response::Ok {
            result: json_codec::to_value(&page)
                .expect("invariant: internal JSON serialization succeeds"),
        })
        .expect("invariant: internal JSON serialization succeeds")
        .len();
        for entry in self.children {
            let extra = json_codec::to_vec(&entry)
                .expect("invariant: internal JSON serialization succeeds")
                .len()
                + usize::from(!page.entries.is_empty());
            if page.entries.len() >= MAX_DIRECTORY_PAGE_ENTRIES
                || bytes.saturating_add(extra) > MAX_DIRECTORY_PAGE_BYTES
            {
                if page.entries.is_empty() {
                    return Err(ApiError::limit("directory entry exceeds page byte budget"));
                }
                page.more = true;
                break;
            }
            bytes += extra;
            page.entries.push(entry);
        }
        Ok(page)
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/read_projection/tests.rs"]
mod tests;

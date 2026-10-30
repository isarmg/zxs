//! Request-scoped owned transport projections. Authorization and version selection
//! happen against the same locked engine; only owned data crosses the boundary.
//! This is not a cache or a reusable authorization grant, and carries no State,
//! private authentication data, content read plan, or publication capability.
use super::wire::PendingSources;
use super::{error, repo_for_branch};
use crate::{ApiError, DirectorySnapshot, TreeSnapshot};
use storage_engine::Engine;

impl TreeSnapshot {
    pub(super) fn capture(engine: &Engine, request: crate::ReadTree<'_>) -> Result<Self, ApiError> {
        let branch = request.branch;
        let (commit, snapshot) = branch_snapshot(engine.state(), branch, request.commit)?;
        #[cfg(test)]
        tests::record_tree_projection(snapshot.entries.len());
        let entries = engine.list_tree_at(branch, commit).map_err(error)?;
        let pending_file_versions =
            PendingSources::new(engine, branch, snapshot).pending_versions();
        Ok(Self {
            branch_id: branch.to_owned(),
            commit_id: commit.to_owned(),
            entries,
            pending_file_versions,
        })
    }
}

pub(super) fn resolve_entry(
    engine: &Engine,
    request: crate::ResolveEntry<'_>,
) -> Result<crate::ResolvedEntry, ApiError> {
    let (commit_id, commit) = branch_snapshot(engine.state(), request.branch, request.commit)?;
    let entries = engine
        .list_tree_at_ref(request.branch, commit_id)
        .map_err(error)?;
    let entry = entries
        .iter()
        .find(|entry| entry.lookup_key() == request.path.key())
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
            super::wire::pending_entry_source(engine, request.branch, commit, entry),
        ))
    } else {
        None
    };
    Ok(crate::ResolvedEntry {
        branch_id: request.branch.to_owned(),
        commit_id: commit_id.to_owned(),
        entry: entry.clone(),
        version: engine.state().versions.get(&entry.version_id).cloned(),
        content,
    })
}

/// The same branch/head checks as Tree, without inspecting or copying entries.
/// Callers must hold the engine admission lock; a snapshot is not an auth grant.
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
    Ok(tree.get_key(path.key()).cloned())
}

impl DirectorySnapshot {
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
            let directory = tree.get_key(key);
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
            crate::DirectorySelection::Capture => Ok(Self {
                commit_id: snapshot.commit_id.clone(),
                entries: Vec::new(),
                more: false,
            }),
            crate::DirectorySelection::Page { after, .. } => Self::page(
                engine.state(),
                &snapshot.commit_id,
                tree.entries(),
                key,
                after,
            ),
        }
    }

    fn page(
        state: &storage_contract::State,
        commit_id: &str,
        entries: &[storage_contract::Entry],
        key: &str,
        after: Option<&str>,
    ) -> Result<Self, ApiError> {
        use smb_protocol::protocol::{
            DirectoryEntry, FileMtime, MAX_DIRECTORY_PAGE_BYTES, MAX_DIRECTORY_PAGE_ENTRIES,
            Response,
        };
        let prefix = if key.is_empty() {
            String::new()
        } else {
            format!("{key}/")
        };
        let start = entries.partition_point(|entry| {
            entry.lookup_key() < prefix.as_str()
                || after.is_some_and(|after| entry.lookup_key() <= after)
        });
        let mut page = Self {
            commit_id: commit_id.into(),
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
        for entry in &entries[start..] {
            let Some(rest) = entry.lookup_key().strip_prefix(&prefix) else {
                break;
            };
            if rest.is_empty() || rest.contains('/') {
                continue;
            }
            let mtime = match entry.kind {
                storage_contract::NamespaceEntryKind::Directory => FileMtime::NotFile {},
                storage_contract::NamespaceEntryKind::File => {
                    let version = state.versions.get(&entry.version_id).ok_or_else(|| {
                        ApiError::new(
                            ::http::StatusCode::INTERNAL_SERVER_ERROR,
                            storage_contract::ErrorCode::MetadataUnavailable,
                            format!("selected directory version {} missing", entry.version_id),
                        )
                    })?;
                    crate::backend_read::version_mtime(version)?
                }
            };
            let entry = DirectoryEntry {
                entry: entry.clone(),
                mtime,
            };
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

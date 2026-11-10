//! Request-scoped owned transport projections. Authorization and version selection
//! happen against the same locked engine; only owned data crosses the boundary.
//! This is not a cache or a reusable authorization grant, and carries no State,
//! private authentication data, content read plan, or publication capability.
use super::wire::{PendingScope, PendingSources};
use super::{error, repo_for_branch};
use crate::{ApiError, DirectorySnapshot, TreeSnapshot};
use storage_engine::Engine;

impl TreeSnapshot {
    pub(super) fn capture(engine: &Engine, request: crate::ReadTree<'_>) -> Result<Self, ApiError> {
        let branch = request.branch;
        let (commit, snapshot) = branch_snapshot(engine.state(), branch, request.commit)?;
        let tree = match request.commit {
            Some(_) => engine.namespace_tree_at(branch, commit),
            None => engine.namespace_tree(branch),
        }
        .map_err(error)?;
        #[cfg(test)]
        tests::record_tree_projection(tree.entries().len());
        let entries = tree.visible_entries().cloned().collect();
        let scope = match request.commit {
            Some(_) => PendingScope::Historical,
            None => PendingScope::Current(tree),
        };
        let pending_file_versions =
            PendingSources::new(engine, branch, snapshot, scope).pending_versions();
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
    let tree = match request.commit {
        Some(_) => engine.namespace_tree_at(request.branch, commit_id),
        None => engine.namespace_tree(request.branch),
    }
    .map_err(error)?;
    let entry = tree.get_key(request.path.key()).ok_or_else(|| {
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
            PendingSources::new(
                engine,
                request.branch,
                commit,
                match request.commit {
                    Some(_) => PendingScope::Historical,
                    None => PendingScope::Current(tree),
                },
            )
            .checked(entry),
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

impl crate::backend_read::DirectoryRegistryBinding {
    fn capture(engine: &Engine) -> Self {
        Self {
            pool_id: engine.config().pool_id.clone(),
            publication: engine.current_disk_registry_commitment().map(|registry| {
                crate::backend_read::DirectoryRegistryPublication {
                    root_hash: registry.root_hash().into(),
                    view_generation: registry.view_generation(),
                }
            }),
        }
    }
}

impl crate::DirectoryReadSnapshot {
    fn capture(engine: &Engine, branch: &str) -> Result<Self, ApiError> {
        let namespace = engine.namespace_snapshot(branch).map_err(error)?;
        Ok(Self {
            branch_id: branch.into(),
            history_tree_id: engine.state().branches[branch].history_tree_id.clone(),
            commit_id: namespace.head_commit_id().into(),
            binding: crate::backend_read::DirectoryRegistryBinding::capture(engine),
            entries: match namespace.into_entries() {
                storage_engine::NamespaceEntries::Commit(commit) => {
                    crate::backend_read::DirectoryEntries::Commit(commit)
                }
                storage_engine::NamespaceEntries::Projected(entries) => {
                    crate::backend_read::DirectoryEntries::Projected(entries)
                }
            },
        })
    }

    fn authorize_current(&self, engine: &Engine, branch: &str) -> Result<(), ApiError> {
        // A current lookup must first pass the live registry admission, including
        // AwaitingPublication. The retained data is not historical authorization.
        engine.namespace_tree(branch).map_err(error)?;
        if self.branch_id != branch
            || self.history_tree_id != engine.state().branches[branch].history_tree_id
            || self.binding != crate::backend_read::DirectoryRegistryBinding::capture(engine)
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SmbRebindRequired,
                "opened directory registration changed",
            ));
        }
        Ok(())
    }
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
        let repository = repo_for_branch(engine, branch)?;
        let snapshot = match &selection {
            crate::DirectorySelection::Page {
                snapshot: Some(snapshot),
                ..
            } => {
                snapshot.authorize_current(engine, branch)?;
                crate::DirectoryReadSnapshot::clone(snapshot)
            }
            crate::DirectorySelection::Capture
            | crate::DirectorySelection::Page { snapshot: None, .. } => {
                crate::DirectoryReadSnapshot::capture(engine, branch)?
            }
        };
        let entries = snapshot.entries.as_slice();
        let key = path.map_or("", |path| path.key());
        if path.is_none() {
            if entry_id != branch {
                return Err(ApiError::invalid("invalid root directory identity"));
            }
        } else {
            let directory = entries
                .binary_search_by(|entry| entry.lookup_key().cmp(key))
                .ok()
                .map(|index| &entries[index]);
            if directory.is_none_or(|entry| {
                entry.kind != storage_contract::NamespaceEntryKind::Directory
                    || entry.entry_id != entry_id
                    || !storage_contract::directory_git::current_entry_visible(
                        &engine.state().directory_git_repositories,
                        &repository.repository_id,
                        entries,
                        entry,
                    )
            }) {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::SmbRebindRequired,
                    "opened directory identity changed",
                ));
            }
        }
        let page = match selection {
            crate::DirectorySelection::Capture => DirectoryPageProjection {
                commit_id: snapshot.commit_id.clone(),
                entries: Vec::new(),
                more: false,
            },
            crate::DirectorySelection::Page { after, .. } => DirectoryPageProjection::page(
                engine.state(),
                &repository.repository_id,
                &snapshot.commit_id,
                entries,
                key,
                after,
            )?,
        };
        Ok(Self {
            commit_id: page.commit_id,
            snapshot,
            entries: page.entries,
            more: page.more,
        })
    }
}

#[derive(data_serialization::Serialize)]
struct DirectoryPageProjection {
    commit_id: String,
    entries: Vec<smb_protocol::protocol::DirectoryEntry>,
    more: bool,
}
impl DirectoryPageProjection {
    fn page(
        state: &storage_contract::State,
        repository: &str,
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
            if rest.is_empty()
                || rest.contains('/')
                || (entry.kind == storage_contract::NamespaceEntryKind::File
                    && state.file_deletions.contains_key(&entry.file_id))
                || !storage_contract::directory_git::current_entry_visible(
                    &state.directory_git_repositories,
                    repository,
                    entries,
                    entry,
                )
            {
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
            let extra = encoded_size(&entry)
                .expect("invariant: internal JSON serialization succeeds")
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

// Measure the same JSON bytes without retaining a throwaway encoded buffer.
fn encoded_size(value: &impl data_serialization::Serialize) -> Result<usize, json_codec::Error> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .expect("invariant: encoded JSON fits the address space");
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    json_codec::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/read_projection/tests.rs"]
mod tests;

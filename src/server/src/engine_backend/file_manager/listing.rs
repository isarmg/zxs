//! Primary-disk selection over one retained namespace, followed by bounded
//! current projection after admission and namespace identity are rechecked.
use super::pagination::{Page, PageRequest, PageSelector};
use super::*;
use crate::backend_listing::SortKey;
/// Only required COW tables and the displayed virtual repositories' namespace
/// owners, plus O(1) directory projection ownership for branch-choice counts.
/// No unrelated commit or version table survives the detached scan.
struct ListingMetadata {
    #[cfg(test)]
    sequence: u64,
    repositories: storage_contract::TrackedMap<Repository>,
    namespaces: BTreeMap<String, storage_engine::NamespaceSnapshot>,
    directory_branches: storage_contract::TrackedMap<storage_contract::DirectoryBranch>,
    directory_namespaces: storage_engine::DirectoryNamespaceView,
    directory_selections: storage_contract::TrackedMap<String>,
}

impl ListingMetadata {
    fn capture(engine: &Engine, slot: &str, root: bool) -> Result<Self, ApiError> {
        let state = engine.state();
        let namespaces = if root {
            state
                .repositories
                .values()
                .filter(|repo| repo.primary_slot_id == slot && !repo.file_manager_root)
                .map(|repo| {
                    engine
                        .namespace_snapshot(&repo.main_branch_id)
                        .map(|namespace| (repo.repository_id.clone(), namespace))
                        .map_err(error)
                })
                .collect::<Result<_, _>>()?
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            #[cfg(test)]
            sequence: state.sequence,
            repositories: state.repositories.clone(),
            namespaces,
            directory_branches: state.directory_branches.clone(),
            directory_namespaces: engine.directory_namespace_view().map_err(error)?,
            directory_selections: state.directory_selections.clone(),
        })
    }
}

#[derive(Clone)]
enum Candidate {
    Repository(String),
    Entry(usize),
}

pub(in crate::engine_backend) enum ListingRead {
    Backup(Box<backup_namespace::BackupListingRead>),
    Primary(Box<PrimaryListing>),
}

struct DirectoryCapture {
    repository: Repository,
    within: String,
    within_key: String,
    namespace: storage_engine::NamespaceSnapshot,
    preview: Option<storage_engine::DirectoryBranchPreview>,
}

struct ListingScope {
    view: ListingMetadata,
    slot_id: String,
    path: String,
    directory: Option<DirectoryCapture>,
}

pub(in crate::engine_backend) struct PrimaryListing {
    scope: ListingScope,
    request: PageRequest,
}

pub(in crate::engine_backend) struct SelectedListing {
    scope: ListingScope,
    page: Page<Candidate>,
    directory_file_count: usize,
    directory_branches: Vec<crate::DirectoryBranchChoice>,
    directory_branches_supported: bool,
}

pub(in crate::engine_backend) struct ProjectedEntries {
    entries: Vec<crate::ListingEntry>,
    selected_entry: Option<crate::ListingEntry>,
}

pub(in crate::engine_backend) fn capture(
    engine: &AdmissionEngine,
    input: crate::ReadListing,
) -> Result<ListingRead, ApiError> {
    let slot_id = input.slot_id().to_owned();
    let path = input.path().to_owned();
    let preview = input
        .preview()
        .map(|(branch, tree)| (branch.to_owned(), tree.to_owned()));
    // Preserve cursor/query/snapshot error precedence before disk and path checks.
    let request = PageRequest::parse(engine, input)?;
    if disk(engine, &slot_id)?.level != 1 {
        if preview.is_some() {
            return Err(ApiError::invalid(
                "backup directories cannot preview primary branches",
            ));
        }
        return backup_namespace::BackupListingRead::capture(engine, &slot_id, &path, request)
            .map(Box::new)
            .map(ListingRead::Backup);
    }
    let directory = match resolve_read(engine, &slot_id, &path) {
        Ok((repository, within)) => Some(DirectoryCapture::capture(
            engine, repository, within, preview,
        )?),
        Err(error)
            if path.is_empty()
                && preview.is_none()
                && matches!(
                    error.status,
                    ::http::StatusCode::FORBIDDEN | ::http::StatusCode::NOT_FOUND
                ) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    Ok(ListingRead::Primary(Box::new(PrimaryListing {
        scope: ListingScope {
            view: ListingMetadata::capture(engine, &slot_id, path.is_empty())?,
            slot_id,
            path,
            directory,
        },
        request,
    })))
}

impl DirectoryCapture {
    fn capture(
        engine: &Engine,
        repository: Repository,
        within: &str,
        preview: Option<(String, String)>,
    ) -> Result<Self, ApiError> {
        let preview = preview
            .map(|(branch, tree)| {
                engine
                    .preview_directory_branch(&repository.repository_id, &branch, &tree)
                    .map_err(error)
            })
            .transpose()?;
        let namespace = engine
            .namespace_snapshot(&repository.main_branch_id)
            .map_err(error)?;
        let source_path = preview
            .as_ref()
            .map(|preview| preview.source_path(within))
            .transpose()
            .map_err(error)?;
        let within_key = name_policy::path_key(source_path.as_deref().unwrap_or(within));
        if !within.is_empty()
            && !preview.as_ref().map_or_else(
                || {
                    namespace.tree().get_key(&within_key).is_some_and(|entry| {
                        entry.kind == storage_contract::NamespaceEntryKind::Directory
                    })
                },
                |preview| {
                    within_key == name_policy::path_key(&preview.branch().path)
                        || preview.branch().entries.iter().any(|entry| {
                            entry.lookup_key() == within_key
                                && entry.kind == storage_contract::NamespaceEntryKind::Directory
                        })
                },
            )
        {
            return Err(ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::DirectoryNotFound,
                "directory not found",
            ));
        }
        Ok(Self {
            repository,
            within: within.into(),
            within_key,
            namespace,
            preview,
        })
    }
}

impl PrimaryListing {
    pub(in crate::engine_backend) fn select(self) -> Result<SelectedListing, ApiError> {
        #[cfg(test)]
        detached_tests::before_scan(&self.scope.view);
        let Self { scope, request } = self;
        let state = &scope.view;
        let mut page = PageSelector::new(request);
        let mut directory_branches_supported = true;
        if scope.path.is_empty() {
            for repo in state
                .repositories
                .values()
                .filter(|repo| repo.primary_slot_id == scope.slot_id && !repo.file_manager_root)
            {
                directory_branches_supported = false;
                page.consider(
                    SortKey::new(true, &repo.name, &repo.name, &repo.repository_id),
                    Candidate::Repository(repo.repository_id.clone()),
                );
            }
        }
        let mut directory_file_count = 0;
        let mut directory_branches = Vec::new();
        if let Some(directory) = &scope.directory {
            let repo = &directory.repository;
            if let Some(preview) = &directory.preview {
                directory_branches_supported = name_policy::path_key(preview.path())
                    == name_policy::path_key(&directory.within);
            }
            if directory_branches_supported {
                directory_branches = super::super::directory_scopes::matching_tables(
                    directory.namespace.tree(),
                    &state.directory_branches,
                    repo,
                    Some(&name_policy::path_key(&directory.within)),
                )
                .into_iter()
                .map(|branch| {
                    let current = state.directory_selections.get(&branch.directory_id)
                        == Some(&branch.directory_branch_id);
                    let entries = state
                        .directory_namespaces
                        .entries(&branch.directory_branch_id)
                        .map_err(error)?;
                    Ok(crate::DirectoryBranchChoice::capture_entries(
                        branch, current, entries,
                    ))
                })
                .collect::<Result<_, ApiError>>()?;
            }
            let prefix = if directory.within.is_empty() {
                String::new()
            } else {
                format!("{}/", directory.within_key)
            };
            // Scan the retained flat namespace, keeping at most limit + 1
            // candidates plus the independently requested selected entry.
            let entries = directory
                .preview
                .as_ref()
                .map_or(directory.namespace.entries(), |preview| {
                    preview.branch().entries.as_slice()
                });
            for (index, entry) in entries.iter().enumerate() {
                let Some(relative) = entry.lookup_key().strip_prefix(&prefix) else {
                    continue;
                };
                if entry.kind == storage_contract::NamespaceEntryKind::File {
                    directory_file_count += 1;
                }
                if relative.contains('/') {
                    continue;
                }
                let rebased = directory
                    .preview
                    .as_ref()
                    .map(|preview| preview.entry(index))
                    .transpose()
                    .map_err(error)?;
                let entry = rebased.as_ref().unwrap_or(entry);
                let display = if repo.file_manager_root {
                    entry.path().to_owned()
                } else {
                    format!("{}/{}", repo.name, entry.path)
                };
                page.consider(SortKey::for_entry(entry, display), Candidate::Entry(index));
            }
        }
        Ok(SelectedListing {
            page: page.finish()?,
            scope,
            directory_file_count,
            directory_branches,
            directory_branches_supported,
        })
    }
}

fn changed() -> ApiError {
    ApiError::conflict(
        storage_contract::ErrorCode::ListingChanged,
        "directory listing changed; restart from the first page",
    )
}

fn unchanged_repository(engine: &Engine, captured: &Repository) -> Result<(), ApiError> {
    if engine.state().repositories.get(&captured.repository_id) != Some(captured) {
        return Err(changed());
    }
    Ok(())
}

fn unchanged_namespace(
    engine: &Engine,
    captured: &storage_engine::NamespaceSnapshot,
) -> Result<(), ApiError> {
    if !engine.namespace_snapshot_matches(&captured.witness()) {
        return Err(changed());
    }
    Ok(())
}

impl SelectedListing {
    fn authorize_current(&self, engine: &AdmissionEngine) -> Result<(), ApiError> {
        if engine.listing_snapshot() != self.page.snapshot {
            return Err(changed());
        }
        disk(engine, &self.scope.slot_id)?;
        if let Some(directory) = &self.scope.directory {
            unchanged_repository(engine, &directory.repository)?;
            unchanged_namespace(engine, &directory.namespace)?;
            if let Some(preview) = &directory.preview {
                engine
                    .preview_directory_branch(
                        &directory.repository.repository_id,
                        &preview.branch().directory_branch_id,
                        &preview.branch().head_tree_id,
                    )
                    .map_err(error)?;
            }
        }
        for candidate in self.page.values.iter().chain(self.page.selected.iter()) {
            if let Candidate::Repository(id) = candidate {
                let state = &self.scope.view;
                let repo = &state.repositories[id];
                unchanged_repository(engine, repo)?;
                unchanged_namespace(engine, &state.namespaces[id])?;
            }
        }
        Ok(())
    }

    /// The caller has refreshed current identity under this same engine lock.
    pub(in crate::engine_backend) fn project(
        &self,
        engine: &AdmissionEngine,
    ) -> Result<ProjectedEntries, ApiError> {
        self.authorize_current(engine)?;
        let project = |candidate: &Candidate| match candidate {
            Candidate::Repository(id) => {
                let repo = &self.scope.view.repositories[id];
                Ok(directory(&repo.name, &repo.name, &repo.repository_id))
            }
            Candidate::Entry(index) => {
                let repo = &self
                    .scope
                    .directory
                    .as_ref()
                    .expect("invariant: entry candidates have a captured namespace")
                    .repository;
                let prefix = if repo.file_manager_root {
                    ""
                } else {
                    &repo.name
                };
                if let Some(preview) = &self
                    .scope
                    .directory
                    .as_ref()
                    .expect("invariant: candidate has directory")
                    .preview
                {
                    listed_preview_entry(engine, repo, preview, *index, prefix)
                } else {
                    listed_current_entry(engine, repo, *index, prefix)
                }
            }
        };
        let entries = self
            .page
            .values
            .iter()
            .map(project)
            .collect::<Result<Vec<_>, ApiError>>()?;
        let selected_entry = self.page.selected.as_ref().map(project).transpose()?;
        Ok(ProjectedEntries {
            entries,
            selected_entry,
        })
    }

    /// Consume summaries and release the retained generation after unlocking.
    pub(in crate::engine_backend) fn finish(
        self,
        page: ProjectedEntries,
    ) -> crate::ListingSnapshot {
        let context = self
            .scope
            .directory
            .as_ref()
            .map(|directory| crate::ListingContext {
                repository_id: directory.repository.repository_id.clone(),
                repository_path: directory.within.clone(),
                branch_id: directory.repository.main_branch_id.clone(),
                base_head: directory.namespace.head_commit_id().into(),
            });
        let default_branch_scope = self
            .scope
            .directory
            .as_ref()
            .is_some_and(|directory| directory.within.is_empty());
        let preview = self
            .scope
            .directory
            .as_ref()
            .is_some_and(|directory| directory.preview.is_some());
        crate::ListingSnapshot {
            slot_id: self.scope.slot_id,
            path: self.scope.path,
            backup: false,
            preview,
            entries: page.entries,
            selected_entry: page.selected_entry,
            context,
            directory_file_count: self.directory_file_count,
            directory_branches: self.directory_branches,
            directory_branches_supported: self.directory_branches_supported,
            default_branch_scope,
            total_entries: self.page.total,
            snapshot: self.page.snapshot,
            previous_cursor: self.page.previous,
            next_cursor: self.page.next,
        }
    }
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/listing_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/listing_detached_tests.rs"]
mod detached_tests;

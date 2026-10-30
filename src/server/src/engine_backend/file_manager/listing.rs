//! Admit primary-directory metadata under the writer lock, then enumerate one
//! immutable publication outside it. Backup path validation remains global.
use super::pagination::{PageRequest, PageSelector};
use super::*;
use crate::backend_listing::SortKey;
use storage_engine::PublishedReadView;

#[derive(Clone)]
enum Candidate<'a> {
    Repository(&'a Repository),
    Entry {
        repository: &'a Repository,
        entry: storage_contract::Entry,
    },
}

pub(in crate::engine_backend) enum ListingRead {
    Backup(Box<backup_namespace::BackupListing>),
    Primary(Box<PrimaryListing>),
}

/// Only request metadata is copied. The namespace and row contexts belong to
/// one shared publication, not a second engine or an authorization grant.
pub(in crate::engine_backend) struct PrimaryListing {
    view: PublishedReadView,
    request: PageRequest,
    slot_id: String,
    path: String,
    resolved: Option<ResolvedDirectory>,
    context: Option<crate::ListingContext>,
    directory_file_count: usize,
    directory_branches: Vec<crate::DirectoryBranchChoice>,
    directory_branches_supported: bool,
    default_branch_scope: bool,
}

struct ResolvedDirectory {
    repository: Repository,
    key: String,
}

pub(in crate::engine_backend) fn capture(
    engine: &AdmissionEngine,
    input: crate::ReadListing,
    control: &crate::HistoryReadControl,
) -> Result<ListingRead, ApiError> {
    activity::checkpoint(control)?;
    let slot_id = input.slot_id().to_owned();
    let path = input.path().to_owned();
    // The token and read view are captured while the same engine lock is held.
    let request = PageRequest::parse(engine, input)?;
    let (slot, value) = (slot_id.as_str(), path.as_str());
    if disk(engine, slot)?.level != 1 {
        return Ok(ListingRead::Backup(Box::new(
            backup_namespace::BackupListing::capture(
                engine.published_read_view(),
                request,
                slot_id,
                path,
                engine.config().max_entries,
            ),
        )));
    }
    let root = value.is_empty();
    let resolved = resolve_read(engine, slot, value);
    let mut context = None;
    let mut directory_file_count = 0;
    let mut directory_branches = Vec::new();
    let mut default_branch_scope = false;
    let directory_branches_supported = !(root && root_has_virtual_directories(engine, slot));
    let resolved = match resolved {
        Ok(resolved) => Some(resolved),
        Err(error)
            if root
                && matches!(
                    error.status,
                    ::http::StatusCode::FORBIDDEN | ::http::StatusCode::NOT_FOUND
                ) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    let resolved = if let Some((repo, within)) = resolved {
        default_branch_scope = within.is_empty();
        let namespace = engine.namespace_tree(&repo.main_branch_id).map_err(error)?;
        let within_key = name_policy::path_key(within);
        if !within.is_empty()
            && !namespace
                .get_key(&within_key)
                .map_err(error)?
                .is_some_and(|entry| entry.kind == storage_contract::NamespaceEntryKind::Directory)
        {
            return Err(ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::DirectoryNotFound,
                "directory not found",
            ));
        }
        if directory_branches_supported {
            directory_branches =
                super::super::directory_scopes::matching(engine, &repo, Some(&within_key))?
                    .into_iter()
                    .map(|branch| {
                        crate::DirectoryBranchChoice::capture(
                            branch,
                            engine
                                .state()
                                .directory_selections
                                .get(&branch.directory_id)
                                == Some(&branch.directory_branch_id),
                        )
                    })
                    .collect();
        }
        directory_file_count = namespace
            .directory_totals(&within_key)
            .map_err(error)?
            .expect("invariant: admitted directory exists in this immutable namespace")
            .1 as usize;
        let branch = &engine.state().branches[&repo.main_branch_id];
        context = Some(crate::ListingContext {
            repository_id: repo.repository_id.clone(),
            repository_path: within.into(),
            branch_id: repo.main_branch_id.clone(),
            base_head: branch.head_commit_id.clone(),
        });
        Some(ResolvedDirectory {
            repository: repo,
            key: within_key,
        })
    } else {
        None
    };
    activity::checkpoint(control)?;
    Ok(ListingRead::Primary(Box::new(PrimaryListing {
        view: engine.published_read_view(),
        request,
        slot_id,
        path,
        resolved,
        context,
        directory_file_count,
        directory_branches,
        directory_branches_supported,
        default_branch_scope,
    })))
}

impl ListingRead {
    pub(in crate::engine_backend) fn read(
        self,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::ListingSnapshot, ApiError> {
        let mut snapshot = match self {
            Self::Backup(plan) => plan.read(control)?,
            Self::Primary(plan) => plan.read(control)?,
        };
        activity::checkpoint(control)?;
        super::super::directory_scopes::sort_choices(&mut snapshot.directory_branches);
        Ok(snapshot)
    }
}

impl PrimaryListing {
    fn read(self, control: &crate::HistoryReadControl) -> Result<crate::ListingSnapshot, ApiError> {
        let Self {
            view,
            request,
            slot_id,
            path,
            resolved,
            context,
            directory_file_count,
            directory_branches,
            directory_branches_supported,
            default_branch_scope,
        } = self;
        activity::checkpoint(control)?;
        let mut page = PageSelector::new(request);
        if path.is_empty() {
            for repo in view.state().repositories.values() {
                activity::checkpoint(control)?;
                if repo.primary_slot_id == slot_id && !repo.file_manager_root {
                    page.consider(
                        SortKey::new(true, &repo.name, &repo.name, &repo.repository_id),
                        Candidate::Repository(repo),
                    );
                }
            }
        }
        let observe = || control.checkpoint().map_err(storage_engine::Error::from);
        if let Some(ResolvedDirectory {
            repository: repo,
            key: within_key,
        }) = &resolved
        {
            let head = &context
                .as_ref()
                .expect("invariant: resolved listing has context")
                .base_head;
            let namespace = view
                .commit_namespace(head, &observe)
                .expect("invariant: admitted listing head exists in the captured publication");
            // Sorting, filtering and totals still require O(directory width)
            // work. Only a source page and limit + 1 candidates are retained.
            let mut after = None;
            loop {
                activity::checkpoint(control)?;
                let children = namespace
                    .directory_page(
                        within_key,
                        after.as_deref(),
                        storage_contract::NAMESPACE_PAGE_ENTRIES,
                    )
                    .map_err(activity::read_error)?
                    .expect("invariant: admitted directory exists in this immutable namespace");
                let complete = children.len() < storage_contract::NAMESPACE_PAGE_ENTRIES;
                after = children.last().map(|entry| {
                    entry
                        .lookup_key()
                        .rsplit('/')
                        .next()
                        .expect("invariant: a portable path has a component")
                        .to_owned()
                });
                for entry in children {
                    activity::checkpoint(control)?;
                    let display = if repo.file_manager_root {
                        entry.path().to_owned()
                    } else {
                        format!("{}/{}", repo.name, entry.path)
                    };
                    page.consider(
                        SortKey::for_entry(&entry, &display),
                        Candidate::Entry {
                            repository: repo,
                            entry,
                        },
                    );
                }
                if complete {
                    break;
                }
            }
        }
        let page = page.finish()?;
        let project = |candidate: &Candidate<'_>| {
            activity::checkpoint(control)?;
            Ok(match candidate {
                Candidate::Repository(repo) => {
                    directory(&repo.name, &repo.name, &repo.repository_id)
                }
                Candidate::Entry {
                    repository: repo,
                    entry,
                } => {
                    let prefix = if repo.file_manager_root {
                        ""
                    } else {
                        &repo.name
                    };
                    listed_current_entry(&view, repo, entry, prefix)
                }
            })
        };
        let values = page
            .values
            .iter()
            .map(project)
            .collect::<Result<Vec<_>, ApiError>>()?;
        let selected_entry = page.selected.as_ref().map(project).transpose()?;
        Ok(crate::ListingSnapshot {
            slot_id,
            path,
            backup: false,
            entries: values,
            selected_entry,
            context,
            directory_file_count,
            directory_branches,
            directory_branches_supported,
            default_branch_scope,
            total_entries: page.total,
            snapshot: page.snapshot,
            previous_cursor: page.previous,
            next_cursor: page.next,
        })
    }
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/listing_tests.rs"]
mod tests;

//! Current primary-disk directory enumeration and bounded wire projection.
use super::pagination::{PageRequest, PageSelector};
use super::*;
use crate::backend_listing::SortKey;

#[derive(Clone)]
enum Candidate<'a> {
    Repository(&'a Repository),
    Entry {
        repository: &'a Repository,
        index: usize,
    },
}

pub(in crate::engine_backend) fn entries(
    engine: &AdmissionEngine,
    input: crate::ReadListing,
) -> Result<crate::ListingSnapshot, ApiError> {
    let slot_id = input.slot_id().to_owned();
    let path = input.path().to_owned();
    let request = PageRequest::parse(engine, input)?;
    let (slot, value) = (slot_id.as_str(), path.as_str());
    if disk(engine, slot)?.level != 1 {
        return backup_namespace::entries(engine, slot, value, request);
    }
    let mut page = PageSelector::new(request);
    let root = value.is_empty();
    if root {
        for repo in engine
            .state()
            .repositories
            .values()
            .filter(|repo| repo.primary_slot_id == slot && !repo.file_manager_root)
        {
            page.consider(
                SortKey::new(true, &repo.name, &repo.name, &repo.repository_id),
                Candidate::Repository(repo),
            );
        }
    }
    let resolved = resolve_read(engine, slot, value);
    let mut context = None;
    let mut directory_file_count = 0;
    let mut directory_branches = Vec::new();
    let mut default_branch_scope = false;
    let directory_branches_supported = !(root && root_has_virtual_directories(engine, slot));
    // Keep the resolved repository alive while candidates borrow its metadata.
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
    if let Some((repo, within)) = &resolved {
        default_branch_scope = within.is_empty();
        let namespace = engine.namespace_tree(&repo.main_branch_id).map_err(error)?;
        let tree = namespace.entries();
        let within_key = name_policy::path_key(within);
        if !within.is_empty()
            && !namespace
                .get_key(&within_key)
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
                super::super::directory_scopes::matching(engine.state(), repo, Some(&within_key))
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
        let prefix = if within.is_empty() {
            String::new()
        } else {
            format!("{within_key}/")
        };
        // The persisted namespace is currently flat. Scan once, retaining only
        // direct children for page selection; descendants contribute only to
        // the existing recursive file count, never to a full JSON projection.
        for (index, entry) in tree.iter().enumerate() {
            let Some(relative) = entry.lookup_key().strip_prefix(&prefix) else {
                continue;
            };
            if entry.kind == storage_contract::NamespaceEntryKind::File {
                directory_file_count += 1;
            }
            if relative.contains('/') {
                continue;
            }
            let display = if repo.file_manager_root {
                entry.path().to_owned()
            } else {
                format!("{}/{}", repo.name, entry.path)
            };
            page.consider(
                SortKey::for_entry(entry, &display),
                Candidate::Entry {
                    repository: repo,
                    index,
                },
            );
        }
        let branch = &engine.state().branches[&repo.main_branch_id];
        context = Some(crate::ListingContext {
            repository_id: repo.repository_id.clone(),
            repository_path: (*within).into(),
            branch_id: repo.main_branch_id.clone(),
            base_head: branch.head_commit_id.clone(),
        });
    }
    let page = page.finish()?;
    let project = |candidate: &Candidate<'_>| match candidate {
        Candidate::Repository(repo) => Ok(directory(&repo.name, &repo.name, &repo.repository_id)),
        Candidate::Entry {
            repository: repo,
            index,
        } => {
            let prefix = if repo.file_manager_root {
                ""
            } else {
                &repo.name
            };
            listed_current_entry(engine, repo, *index, prefix)
        }
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

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/listing_tests.rs"]
mod tests;

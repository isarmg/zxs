//! Current primary-disk directory enumeration and bounded wire projection.
use super::pagination::{PageRequest, PageSelector, SortKey};
use super::*;

#[derive(Clone)]
enum Candidate<'a> {
    Repository(&'a Repository),
    Entry {
        repository: &'a Repository,
        index: usize,
    },
}

pub(super) fn entries(engine: &Engine, input: &Value) -> Result<Value, ApiError> {
    let slot = field(input, "slot_id")?;
    let value = lookup_path(input)?;
    let request = PageRequest::parse(engine, input, slot, value)?;
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
    let mut context = Value::Null;
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
                .is_some_and(|entry| entry.kind == "directory")
        {
            return Err(ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::DirectoryNotFound,
                "directory not found",
            ));
        }
        if directory_branches_supported {
            directory_branches =
                scoped_directory_branches(engine, &repo.repository_id, Some(within))?
                    .into_iter()
                    .map(|branch| {
                        branch_summary(
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
            let Some(relative) = entry.lookup_key.strip_prefix(&prefix) else {
                continue;
            };
            if entry.kind == "file" {
                directory_file_count += 1;
            }
            if relative.contains('/') {
                continue;
            }
            let display = if repo.file_manager_root {
                entry.path.clone()
            } else {
                format!("{}/{}", repo.name, entry.path)
            };
            page.consider(
                SortKey::new(
                    entry.kind == "directory",
                    &entry.name,
                    &display,
                    &entry.entry_id,
                ),
                Candidate::Entry {
                    repository: repo,
                    index,
                },
            );
        }
        let branch = &engine.state().branches[&repo.main_branch_id];
        context = json!({"repository_id":repo.repository_id,"repository_path":within,"branch_id":repo.main_branch_id,"base_head":branch.head_commit_id});
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
            let mut value = listed_current_entry(engine, repo, *index, prefix)?;
            value["note"] = engine
                .state()
                .file_notes
                .get(value["file_id"].as_str().unwrap_or_default())
                .cloned()
                .unwrap_or_default()
                .into();
            Ok(value)
        }
    };
    let values = page
        .values
        .iter()
        .map(project)
        .collect::<Result<Vec<_>, ApiError>>()?;
    let selected_entry = page.selected.as_ref().map(project).transpose()?;
    let mut response = json!({"slot_id":slot,"path":value,"writable":true,"entries":values,"selected_entry":selected_entry,"context":context,"directory_file_count":directory_file_count.to_string(),"directory_branches":directory_branches,"directory_branches_supported":directory_branches_supported,"default_branch_scope":default_branch_scope});
    page.metadata(&mut response);
    Ok(response)
}

/// Compact branch choices never serialize descendant entries or context maps.
/// Branch count itself is not paginated by the file-entry cursor.
pub(super) fn branch_summary(branch: &storage_contract::DirectoryBranch, current: bool) -> Value {
    json!({"directory_branch_id":branch.directory_branch_id,"repository_id":branch.repository_id,
        "directory_id":branch.directory_id,"name":branch.name,"parent_branch_id":branch.parent_branch_id,
        "head_tree_id":branch.head_tree_id,"commit_id":branch.commit_id,"path":branch.path,
        "materialized":branch.materialized,"current":current,"sequence":branch.sequence.to_string(),
        "file_count":branch.entries.iter().filter(|entry|entry.kind=="file").count().to_string()})
}
#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/listing_tests.rs"]
mod tests;

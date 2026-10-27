//! Bounded, validated selection facts are separate from later mutation admission.
use crate::{ApiError, DirectoryBranchChoice};
use std::collections::BTreeSet;

pub struct DirectorySelectionSourceInput {
    pub directory_branch_id: String,
    pub expected_tree_id: String,
}
pub struct DirectorySelectionEntryInput {
    pub entry_id: String,
    pub path: String,
}
pub struct ReadDirectorySelection {
    repository_id: String,
    scope: Option<name_policy::LookupPath>,
    sources: Vec<DirectorySelectionSourceInput>,
    entries: Vec<SelectedDirectoryEntry>,
}
pub struct SelectedDirectoryEntry {
    pub(crate) entry_id: String,
    pub(crate) path: name_policy::LookupPath,
}
impl SelectedDirectoryEntry {
    pub fn entry_id(&self) -> &str {
        &self.entry_id
    }
    pub fn path(&self) -> &name_policy::LookupPath {
        &self.path
    }
}
fn nonempty(value: &str, field: &str) -> Result<(), ApiError> {
    if value.is_empty() {
        return Err(ApiError::invalid(format!("missing string field {field}")));
    }
    Ok(())
}
impl ReadDirectorySelection {
    /// Parse an external selection once, bounding work and proving child/uniqueness rules.
    /// This captures no current authorization or immutable-tree baseline proof.
    pub fn parse(
        repository_id: String,
        path: &str,
        sources: Vec<DirectorySelectionSourceInput>,
        entries: Vec<DirectorySelectionEntryInput>,
    ) -> Result<Self, ApiError> {
        nonempty(&repository_id, "repository_id")?;
        if sources.len() > 500 || entries.len() > 500 || sources.len() * entries.len() > 10_000 {
            return Err(ApiError::new(
                http::StatusCode::TOO_MANY_REQUESTS,
                storage_contract::ErrorCode::ResourceLimitExceeded,
                "scope query exceeds the bounded selection limit",
            ));
        }
        let scope = if path.is_empty() {
            None
        } else {
            Some(name_policy::LookupPath::parse(path).map_err(ApiError::invalid_source)?)
        };
        let scope_key = scope.as_ref().map_or("", name_policy::LookupPath::key);
        let entries = selected_entries(entries, scope_key)?;
        let mut seen = BTreeSet::new();
        for source in &sources {
            nonempty(&source.directory_branch_id, "directory_branch_id")?;
            nonempty(&source.expected_tree_id, "expected_tree_id")?;
            if !seen.insert(&source.directory_branch_id) {
                return Err(ApiError::invalid("duplicate source scope"));
            }
        }
        Ok(Self {
            repository_id,
            scope,
            sources,
            entries,
        })
    }
    pub fn repository_id(&self) -> &str {
        &self.repository_id
    }
    pub fn scope(&self) -> Option<&name_policy::LookupPath> {
        self.scope.as_ref()
    }
    pub fn sources(&self) -> &[DirectorySelectionSourceInput] {
        &self.sources
    }
    pub fn entries(&self) -> &[SelectedDirectoryEntry] {
        &self.entries
    }
}
fn selected_entries(
    entries: Vec<DirectorySelectionEntryInput>,
    scope_key: &str,
) -> Result<Vec<SelectedDirectoryEntry>, ApiError> {
    let mut seen = BTreeSet::new();
    let mut selected = Vec::with_capacity(entries.len());
    for DirectorySelectionEntryInput { entry_id, path } in entries {
        nonempty(&entry_id, "entry_id")?;
        nonempty(&path, "path")?;
        if entry_id.len() > 256 {
            return Err(ApiError::invalid("entry identity is too long"));
        }
        let path = name_policy::LookupPath::parse(&path).map_err(ApiError::invalid_source)?;
        let parent = path.key().rsplit_once('/').map_or("", |(parent, _)| parent);
        if parent != scope_key {
            return Err(ApiError::invalid(
                "selected entries must be direct children of the requested scope",
            ));
        }
        if !seen.insert((entry_id.clone(), path.key().to_owned())) {
            return Err(ApiError::invalid("duplicate selected entry"));
        }
        selected.push(SelectedDirectoryEntry { entry_id, path });
    }
    selected.sort_by(|left, right| {
        (&left.entry_id, left.path.as_str()).cmp(&(&right.entry_id, right.path.as_str()))
    });
    Ok(selected)
}
pub struct DirectorySelectionSnapshot {
    pub repository_id: String,
    pub branches: Vec<DirectorySelectionBranch>,
}
pub struct DirectorySelectionBranch {
    pub summary: DirectoryBranchChoice,
    pub entries: Vec<SelectedEntryFact>,
}
pub struct SelectedEntryFact {
    pub entry_id: String,
    pub path: String,
    pub selected_path: String,
}
#[derive(data_serialization::Serialize)]
pub(crate) struct DirectorySelectionResponse {
    repository_id: String,
    directory_branches: Vec<DirectorySelectionRecord>,
}
#[derive(data_serialization::Serialize)]
struct DirectorySelectionRecord {
    #[serialization(flatten)]
    summary: crate::backend_directory_scopes::DirectoryChoiceRecord,
    entries: Vec<SelectedEntryRecord>,
}
#[derive(data_serialization::Serialize)]
struct SelectedEntryRecord {
    entry_id: String,
    path: String,
    selected_path: String,
}
impl From<DirectorySelectionSnapshot> for DirectorySelectionResponse {
    fn from(snapshot: DirectorySelectionSnapshot) -> Self {
        let DirectorySelectionSnapshot {
            repository_id,
            branches,
        } = snapshot;
        let directory_branches = branches
            .into_iter()
            .map(|branch| {
                let DirectorySelectionBranch { summary, entries } = branch;
                let entries = entries
                    .into_iter()
                    .map(|entry| {
                        let SelectedEntryFact {
                            entry_id,
                            path,
                            selected_path,
                        } = entry;
                        SelectedEntryRecord {
                            entry_id,
                            path,
                            selected_path,
                        }
                    })
                    .collect();
                DirectorySelectionRecord {
                    summary: summary.into(),
                    entries,
                }
            })
            .collect();
        Self {
            repository_id,
            directory_branches,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_directory_selection.rs"]
mod tests;

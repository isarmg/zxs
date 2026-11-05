//! Read-only browsing of a frozen directory composition. No publication, IDs,
//! operation receipt or persistent selection is created by this projection.
use super::*;

pub struct DirectoryBranchPreview {
    branch: DirectoryBranch,
    path: String,
}
impl DirectoryBranchPreview {
    pub fn branch(&self) -> &DirectoryBranch {
        &self.branch
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn source_path(&self, path: &str) -> Result<String> {
        let key = name_policy::path_key(path);
        let scope = name_policy::path_key(&self.path);
        if key == scope {
            return Ok(self.branch().path.clone());
        }
        let suffix = if scope.is_empty() {
            path
        } else {
            key.strip_prefix(&format!("{scope}/")).ok_or_else(|| {
                Error::new(
                    ErrorCode::DirectoryBranchScope,
                    "preview path is outside its directory",
                )
            })?;
            path.splitn(self.path.split('/').count() + 1, '/')
                .last()
                .expect("invariant: admitted descendant has a suffix")
        };
        Ok(if self.branch().path.is_empty() {
            suffix.into()
        } else {
            format!("{}/{suffix}", self.branch().path)
        })
    }
    pub fn entry(&self, index: usize) -> Result<Entry> {
        let entry = &self.branch().entries[index];
        let path = super::directory::rebase_path(entry.path(), &self.branch().path, &self.path)?;
        let mut entry = entry.clone();
        entry.path = name_policy::PortablePath::parse(&path)?;
        Ok(entry)
    }
    pub fn file_context(&self, index: usize) -> Result<Option<&FileContext>> {
        let entry = &self.branch().entries[index];
        if entry.kind == NamespaceEntryKind::Directory {
            return Ok(None);
        }
        self.branch()
            .file_contexts
            .get(entry.lookup_key())
            .map(Some)
            .ok_or_else(|| Error::new(ErrorCode::StateInvalid, "preview file context missing"))
    }
}
impl Engine {
    pub fn preview_directory_branch(
        &self,
        repository: &str,
        branch_id: &str,
        expected_tree: &str,
    ) -> Result<DirectoryBranchPreview> {
        let repo = self
            .state
            .repositories
            .get(repository)
            .ok_or_else(|| Error::new(ErrorCode::RepositoryNotFound, "repository missing"))?;
        let branch = self
            .state
            .directory_branches
            .get(branch_id)
            .filter(|branch| branch.repository_id == repository)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DirectoryBranchNotFound,
                    "directory branch missing",
                )
            })?;
        if let Some(tree) = self
            .state
            .directory_trees
            .get(expected_tree)
            .filter(|tree| {
                storage_contract::directory_staging::directory_tree_is_committed(&self.state, tree)
            })
        {
            let owner = &self.state.directory_branches[&tree.directory_branch_id];
            if owner.repository_id != repository || owner.directory_id != branch.directory_id {
                return Err(Error::new(
                    ErrorCode::DirectoryBranchScope,
                    "history snapshot belongs to another directory",
                ));
            }
            let mut frozen = branch.clone();
            frozen.head_tree_id = tree.tree_id.clone();
            frozen.commit_id = tree.commit_id.clone();
            frozen.path = tree.path.clone();
            frozen.entries = tree.entries.clone();
            frozen.file_contexts = tree.file_contexts.clone();
            frozen.file_branches = tree.file_branches.clone();
            frozen.inherited_files = tree.inherited_files.clone();
            frozen.directory_refs = tree.directory_refs.clone();
            frozen.sequence = tree.sequence;
            let path = if branch.directory_id == repository {
                String::new()
            } else {
                self.list_tree_ref(&repo.main_branch_id)?
                    .iter()
                    .find(|entry| {
                        entry.kind == NamespaceEntryKind::Directory
                            && entry.entry_id == branch.directory_id
                    })
                    .map(|entry| entry.path().to_owned())
                    .unwrap_or_else(|| tree.path.clone())
            };
            return Ok(DirectoryBranchPreview {
                branch: frozen,
                path,
            });
        }
        if branch.head_tree_id != expected_tree {
            return Err(Error::new(
                ErrorCode::ListingChanged,
                "directory branch changed; refresh before browsing",
            ));
        }
        let projection = self
            .admitted_current_namespaces()?
            .map(|view| view.directory_projection(&self.state, branch_id))
            .transpose()?;
        let path = if let Some(projection) = &projection {
            projection.path().to_owned()
        } else if branch.directory_id == repository {
            String::new()
        } else {
            self.list_tree_ref(&repo.main_branch_id)?
                .iter()
                .find(|entry| {
                    entry.kind == NamespaceEntryKind::Directory
                        && entry.entry_id == branch.directory_id
                })
                .ok_or_else(|| {
                    Error::new(ErrorCode::DirectoryNotFound, "directory no longer exists")
                })?
                .path()
                .to_owned()
        };
        Ok(DirectoryBranchPreview {
            branch: projection
                .as_ref()
                .map_or_else(|| branch.clone(), |projection| projection.branch().clone()),
            path,
        })
    }
}

/// A default publication names both the exact viewed composition and the
/// namespace that will be replaced. Exact operation replay is resolved first.
pub struct DirectoryDefaultSelection<'a> {
    pub repository_id: &'a str,
    pub directory_branch_id: &'a str,
    pub expected_tree_id: &'a str,
    pub expected_head: &'a str,
    pub principal: &'a str,
    pub operation_id: OperationId,
}
impl Engine {
    pub fn set_directory_default(
        &mut self,
        request: DirectoryDefaultSelection<'_>,
    ) -> Result<OperationResult> {
        self.directory_git_checkout(json_codec::json!({"repository_id":request.repository_id,"directory_branch_id":request.directory_branch_id,"expected_tree_id":request.expected_tree_id,"expected_head":request.expected_head}),request.principal,request.operation_id)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/directory_preview_tests.rs"]
mod tests;

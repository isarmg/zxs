//! Directory-local compositions retain identities but remove the exterior mount.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{
    DirectoryBranch, DirectoryTree, NamespaceEntryKind, directory_git::DirectoryGitRepository,
};

pub(super) use super::super::local_database::MAX_RECORD_BYTES;

#[derive(Serialize)]
struct LocalBranch<'a> {
    directory_branch_id: &'a str,
    name: &'a str,
    parent_branch_id: &'a Option<String>,
    head_tree_id: &'a str,
    commit_id: &'a str,
    materialized: bool,
}

fn local(path: &str, scope: &str) -> Result<String> {
    if scope.is_empty() {
        return Ok(path.to_owned());
    }
    let parsed = name_policy::PortablePath::parse(path)?;
    let scope_key = name_policy::path_key(scope);
    if !parsed
        .key()
        .strip_prefix(&scope_key)
        .is_some_and(|tail| tail.starts_with('/'))
    {
        return Err(invalid(
            "directory database entry is outside its snapshot root",
        ));
    }
    Ok(parsed
        .strip_components(scope.split('/').count())
        .expect("invariant: descendant has a component after the scope")
        .into_string())
}

fn key_local(path: &str, scope: &str) -> Result<String> {
    local(path, &name_policy::path_key(scope))
}

fn tree(state: &State, original: &DirectoryTree) -> Result<DirectoryTree> {
    let mut tree = original.clone();
    // Exact snapshot references identify child repositories. Do not serialize
    // their cached descendant compositions or follow a child's mutable HEAD.
    let boundaries = tree
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == NamespaceEntryKind::Directory
                && tree.directory_refs.contains_key(&entry.entry_id)
                && state
                    .directory_git_repositories
                    .get(&entry.entry_id)
                    .is_some_and(|git| git.initialized_sequence <= original.sequence)
        })
        .map(|entry| entry.lookup_key().to_owned())
        .collect::<BTreeSet<_>>();
    let beneath = |path: &str| {
        let mut path = path;
        while let Some((parent, _)) = path.rsplit_once('/') {
            if boundaries.contains(parent) {
                return true;
            }
            path = parent;
        }
        false
    };
    tree.entries.retain(|entry| !beneath(entry.lookup_key()));
    let files = tree
        .entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .map(|entry| entry.file_id.clone())
        .collect::<BTreeSet<_>>();
    let directories = tree
        .entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
        .map(|entry| entry.entry_id.clone())
        .collect::<BTreeSet<_>>();
    tree.file_branches.retain(|file, _| files.contains(file));
    tree.inherited_files.retain(|file, _| files.contains(file));
    tree.directory_refs.retain(|id, _| directories.contains(id));
    tree.file_contexts = tree
        .file_contexts
        .into_iter()
        .filter(|(path, _)| !beneath(path))
        .map(|(path, context)| Ok((key_local(&path, &original.path)?, context)))
        .collect::<Result<_>>()?;
    for entry in &mut tree.entries {
        entry.relocate(name_policy::PortablePath::parse(&local(
            entry.path(),
            &original.path,
        )?)?);
    }
    tree.path.clear();
    Ok(tree)
}

pub(super) fn records(state: &State, directory: &str) -> Result<Vec<Record>> {
    let git = &state.directory_git_repositories[directory];
    let mut records = SCHEMA.records();
    let mut local_git: DirectoryGitRepository = git.clone();
    for node in local_git.deletions.values_mut() {
        let scope = if node.owner_directory_id == node.repository_id {
            ""
        } else {
            state.commits[&node.namespace_head]
                .entries
                .iter()
                .find(|entry| {
                    entry.entry_id == node.owner_directory_id
                        && entry.kind == NamespaceEntryKind::Directory
                })
                .ok_or_else(|| {
                    invalid("directory deletion owner is missing from its native namespace")
                })?
                .path()
        };
        node.path = name_policy::PortablePath::parse(&local(node.path.as_str(), scope)?)?;
    }
    for branch in local_git.branches.values_mut() {
        let scope = &state.directory_trees[&branch.index_tree_id].path;
        for path in branch.index_boundaries.values_mut() {
            *path = local(path, scope)?;
        }
    }
    records.add("repository", directory, &local_git)?;
    let branches: BTreeMap<_, &DirectoryBranch> = state
        .directory_branches
        .iter()
        .filter(|(_, branch)| {
            branch.directory_id == directory && branch.repository_id == git.repository_id
        })
        .collect();
    for (id, branch) in &branches {
        records.add(
            "branch",
            id,
            &LocalBranch {
                directory_branch_id: id,
                name: &branch.name,
                parent_branch_id: &branch.parent_branch_id,
                head_tree_id: &branch.head_tree_id,
                commit_id: &branch.commit_id,
                materialized: branch.materialized,
            },
        )?;
    }
    for original in state
        .directory_trees
        .values()
        .filter(|tree| branches.contains_key(&tree.directory_branch_id))
    {
        let projected = tree(state, original)?;
        records.add("tree", &original.tree_id, &projected)?;
        let commit = &state.commits[&original.commit_id];
        if commit.kind == "directory_commit"
            && commit.provenance.as_deref() == Some(original.tree_id.as_str())
        {
            let mut commit = commit.clone();
            let own_versions = projected
                .entries
                .iter()
                .filter(|entry| entry.kind == NamespaceEntryKind::File)
                .map(|entry| (entry.file_id.as_str(), entry.version_id.as_str()))
                .collect::<BTreeSet<_>>();
            commit
                .pending_file_versions
                .retain(|file, version| own_versions.contains(&(file.as_str(), version.as_str())));
            commit.entries = projected.entries;
            records.add("commit", &commit.commit_id, &commit)?;
        }
    }
    Ok(records.finish())
}

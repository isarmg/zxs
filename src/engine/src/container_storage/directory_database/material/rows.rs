//! Typed validation of the path-free header/branch rows and object-relative
//! trees. This is local structural closure, not publication authentication.
use super::*;

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct LocalBranch {
    directory_branch_id: String,
    name: String,
    parent_branch_id: Option<String>,
    head_tree_id: String,
    commit_id: String,
    materialized: bool,
}

pub(super) fn validate_rows(
    records: &[Record],
    repository: &str,
    directory: &str,
    through: u64,
) -> Result<()> {
    let mut git = None;
    let mut branches = BTreeMap::<String, LocalBranch>::new();
    let mut trees = BTreeMap::<String, DirectoryTree>::new();
    let mut commits = BTreeMap::<String, Commit>::new();
    for record in records {
        match record.kind {
            "repository" => {
                let row: DirectoryGitRepository = decode(&record.body)?;
                if record.id != directory
                    || row.directory_id != directory
                    || row.repository_id != repository
                    || git.replace(row).is_some()
                {
                    return Err(invalid(
                        "directory material has another or repeated repository scope",
                    ));
                }
            }
            "branch" => {
                let row: LocalBranch = decode(&record.body)?;
                storage_contract::validate_id(&record.id, "dbr")?;
                storage_contract::validate_branch_name(&row.name)?;
                if record.id != row.directory_branch_id {
                    return Err(invalid("directory branch key differs"));
                }
                branches.insert(record.id.clone(), row);
            }
            "tree" => {
                let row: DirectoryTree = decode(&record.body)?;
                storage_contract::validate_id(&record.id, "dtree")?;
                storage_contract::validate_id(&row.commit_id, "commit")?;
                if record.id != row.tree_id || !row.path.is_empty() {
                    return Err(invalid(
                        "directory material tree key or exterior root path differs",
                    ));
                }
                sequence(row.sequence, through)?;
                tree_rows(&row)?;
                trees.insert(record.id.clone(), row);
            }
            "commit" => {
                let row: Commit = decode(&record.body)?;
                storage_contract::validate_id(&record.id, "commit")?;
                storage_contract::validate_id(&row.history_tree_id, "history")?;
                storage_contract::validate_id(&row.tree_id, "tree")?;
                if record.id != row.commit_id
                    || row.kind != "directory_commit"
                    || row.namespace_deletions.is_some()
                {
                    return Err(invalid(
                        "directory material contains a nonlocal commit envelope",
                    ));
                }
                sequence(row.sequence, through)?;
                entries(&row.entries)?;
                for (file, version) in &row.pending_file_versions {
                    if !row.entries.iter().any(|entry| {
                        entry.kind == NamespaceEntryKind::File
                            && &entry.file_id == file
                            && &entry.version_id == version
                    }) {
                        return Err(invalid(
                            "directory local commit retains an outside pending version",
                        ));
                    }
                }
                commits.insert(record.id.clone(), row);
            }
            _ => return Err(invalid("unsupported directory material row kind")),
        }
    }
    let git = git.ok_or_else(|| invalid("directory material has no repository record"))?;
    sequence(git.initialized_sequence, through)?;
    if git.branches.is_empty()
        || !git.branches.contains_key(&git.main_branch_id)
        || !git.branches.contains_key(&git.current_branch_id)
    {
        return Err(invalid(
            "directory material has no exact main/current branch",
        ));
    }
    parents_closed(
        branches
            .iter()
            .map(|(id, row)| (id.as_str(), row.parent_branch_id.as_deref()))
            .collect(),
    )?;
    parents_closed(
        trees
            .iter()
            .map(|(id, row)| (id.as_str(), row.parent_tree_id.as_deref()))
            .collect(),
    )?;
    for (id, branch) in &branches {
        let head = trees
            .get(&branch.head_tree_id)
            .ok_or_else(|| invalid("directory local branch HEAD is missing"))?;
        if head.commit_id != branch.commit_id
            || branch.materialized && head.directory_branch_id != *id
        {
            return Err(invalid(
                "directory local branch differs from its exact HEAD",
            ));
        }
    }
    for tree in trees.values() {
        if !branches.contains_key(&tree.directory_branch_id) {
            return Err(invalid("directory local tree owner is missing"));
        }
        if tree
            .inherited_files
            .values()
            .any(|origin| !branches.contains_key(origin))
            || tree
                .file_contexts
                .values()
                .filter_map(|context| context.inherited_from.as_ref())
                .any(|origin| !branches.contains_key(origin))
        {
            return Err(invalid(
                "directory inherited write intent escapes its local history",
            ));
        }
    }
    for (branch, index) in &git.branches {
        if !branches.contains_key(branch)
            || index.index_generation == 0
            || !trees.contains_key(&index.index_tree_id)
        {
            return Err(invalid(
                "directory local Git branch or exact index is missing",
            ));
        }
        if let Some(head) = &index.head_tree_id {
            let tree = trees
                .get(head)
                .ok_or_else(|| invalid("directory committed HEAD is missing"))?;
            if !commits
                .get(&tree.commit_id)
                .is_some_and(|commit| commit.provenance.as_ref() == Some(head))
            {
                return Err(invalid("directory HEAD lacks its local user-commit marker"));
            }
        }
        for (child, path) in &index.index_boundaries {
            storage_contract::validate_id(child, "entry")?;
            name_policy::PortablePath::parse(path)?;
            if child == directory {
                return Err(invalid("directory boundary refers to itself"));
            }
        }
        for (child, reference) in &index.index_children {
            storage_contract::validate_id(child, "entry")?;
            storage_contract::validate_id(&reference.directory_branch_id, "dbr")?;
            if let Some(tree) = &reference.tree_id {
                storage_contract::validate_id(tree, "dtree")?;
            }
            if child == directory {
                return Err(invalid("directory child reference refers to itself"));
            }
        }
    }
    for commit in commits.values() {
        let tree = commit
            .provenance
            .as_ref()
            .and_then(|id| trees.get(id))
            .ok_or_else(|| invalid("directory local commit has no exact tree marker"))?;
        if tree.commit_id != commit.commit_id
            || tree.sequence != commit.sequence
            || tree.entries != commit.entries
        {
            return Err(invalid(
                "directory local commit differs from its complete tree",
            ));
        }
        for parent in &commit.parent_commit_ids {
            let parent = commits
                .get(parent)
                .ok_or_else(|| invalid("directory user-commit parent is missing"))?;
            if parent.sequence >= commit.sequence
                || parent.history_tree_id != commit.history_tree_id
            {
                return Err(invalid("directory local commit parent is later or foreign"));
            }
        }
    }
    Ok(())
}

fn tree_rows(tree: &DirectoryTree) -> Result<()> {
    entries(&tree.entries)?;
    let paths: BTreeSet<_> = tree
        .entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .map(|entry| entry.lookup_key())
        .collect();
    let files: BTreeSet<_> = tree
        .entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .map(|entry| entry.file_id.as_str())
        .collect();
    let directories: BTreeSet<_> = tree
        .entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
        .map(|entry| entry.entry_id.as_str())
        .collect();
    if tree.file_branches.len() != files.len() || tree.file_contexts.len() != paths.len() {
        return Err(invalid(
            "directory local file branch/context coverage differs",
        ));
    }
    for (file, branch) in &tree.file_branches {
        if !files.contains(file.as_str()) {
            return Err(invalid("directory tree has an outside file branch"));
        }
        storage_contract::validate_id(branch, "br")?;
    }
    for (file, origin) in &tree.inherited_files {
        if !files.contains(file.as_str()) {
            return Err(invalid("directory tree has an outside inherited file"));
        }
        storage_contract::validate_id(origin, "dbr")?;
    }
    for (path, context) in &tree.file_contexts {
        let parsed = name_policy::PortablePath::parse(path)?;
        if parsed.key() != path || !paths.contains(path.as_str()) {
            return Err(invalid(
                "directory tree context is not an exact local path key",
            ));
        }
        storage_contract::validate_id(&context.branch_id, "br")?;
        if let Some(origin) = &context.inherited_from {
            storage_contract::validate_id(origin, "dbr")?;
        }
        if let Some(node) = &context.node_id {
            storage_contract::history_window::HistoryNodeId::parse(node)?;
        }
    }
    for (directory, reference) in &tree.directory_refs {
        if !directories.contains(directory.as_str()) {
            return Err(invalid("directory tree has an outside child reference"));
        }
        storage_contract::validate_id(&reference.directory_branch_id, "dbr")?;
        storage_contract::validate_id(&reference.tree_id, "dtree")?;
    }
    Ok(())
}

//! Keep directory working compositions, indexes and committed pins distinct.
use super::graph::Graph;
use super::*;
use storage_contract::{DirectoryReference, directory_git::DirectoryGitChildReference};

fn contexts(
    graph: &Graph,
    input: &BTreeMap<String, FileContext>,
) -> Result<BTreeMap<String, FileContext>> {
    input
        .iter()
        .map(|(key, context)| Ok((key.clone(), graph.context(context)?)))
        .collect()
}
fn file_branches(
    graph: &Graph,
    input: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    input
        .iter()
        .map(|(file, branch)| Ok((graph.id(file)?, graph.id(branch)?)))
        .collect()
}
fn inherited_files(
    graph: &Graph,
    input: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    // An external directory's write origin does not move with this unit.
    // Its frozen bytes are already retained through the pending-version graph;
    // only an included directory origin remains a lazy branch-write intent.
    input
        .iter()
        .filter_map(|(file, origin)| {
            graph
                .ids
                .get(origin)
                .filter(|origin| origin.starts_with("dbr_"))
                .map(|origin| graph.id(file).map(|file| (file, origin.clone())))
        })
        .collect()
}
fn references(
    graph: &Graph,
    input: &BTreeMap<String, DirectoryReference>,
) -> Result<BTreeMap<String, DirectoryReference>> {
    input
        .iter()
        .map(|(directory, reference)| {
            Ok((
                graph.id(directory)?,
                DirectoryReference {
                    directory_branch_id: graph.id(&reference.directory_branch_id)?,
                    tree_id: graph.id(&reference.tree_id)?,
                },
            ))
        })
        .collect()
}

pub(super) fn copy(
    previous: &State,
    next: &mut State,
    unit: &MoveUnit,
    target: &Repository,
    graph: &Graph,
    event: &MoveEvent<'_>,
) -> Result<Vec<storage_contract::directory_git::DirectoryDeletionNode>> {
    let mut imported = Vec::new();
    for (directory, member) in &unit.directories {
        for id in &member.tree_ids {
            let mut tree = previous.directory_trees[id].clone();
            tree.tree_id = graph.id(id)?;
            tree.directory_branch_id = graph.id(&tree.directory_branch_id)?;
            tree.parent_tree_id = tree
                .parent_tree_id
                .as_deref()
                .map(|id| graph.id(id))
                .transpose()?;
            tree.commit_id = graph.id(&tree.commit_id)?;
            tree.entries = tree
                .entries
                .iter()
                .map(|entry| graph.entry(entry))
                .collect::<Result<_>>()?;
            tree.file_branches = file_branches(graph, &tree.file_branches)?;
            tree.file_contexts = contexts(graph, &tree.file_contexts)?;
            tree.inherited_files = inherited_files(graph, &tree.inherited_files)?;
            tree.directory_refs = references(graph, &tree.directory_refs)?;
            next.directory_trees.insert(tree.tree_id.clone(), tree);
        }
        for id in &member.branch_ids {
            let mut branch = previous.directory_branches[id].clone();
            branch.directory_branch_id = graph.id(id)?;
            branch.repository_id = target.repository_id.clone();
            branch.directory_id = graph.id(&branch.directory_id)?;
            branch.parent_branch_id = branch
                .parent_branch_id
                .as_deref()
                .map(|id| graph.id(id))
                .transpose()?;
            branch.head_tree_id = graph.id(&branch.head_tree_id)?;
            branch.commit_id = graph.id(&branch.commit_id)?;
            branch.entries = branch
                .entries
                .iter()
                .map(|entry| graph.entry(entry))
                .collect::<Result<_>>()?;
            branch.file_branches = file_branches(graph, &branch.file_branches)?;
            branch.file_contexts = contexts(graph, &branch.file_contexts)?;
            branch.inherited_files = inherited_files(graph, &branch.inherited_files)?;
            branch.directory_refs = references(graph, &branch.directory_refs)?;
            next.directory_branches
                .insert(branch.directory_branch_id.clone(), branch);
        }
        if let Some(mut git) = member.repository.clone() {
            git.repository_id = target.repository_id.clone();
            git.directory_id = graph.id(&git.directory_id)?;
            git.main_branch_id = graph.id(&git.main_branch_id)?;
            git.current_branch_id = graph.id(&git.current_branch_id)?;
            git.branches = git
                .branches
                .into_iter()
                .map(|(id, mut branch)| {
                    branch.head_tree_id = branch
                        .head_tree_id
                        .as_deref()
                        .map(|id| graph.id(id))
                        .transpose()?;
                    branch.index_tree_id = graph.id(&branch.index_tree_id)?;
                    branch.index_boundaries = branch
                        .index_boundaries
                        .into_iter()
                        .map(|(id, path)| Ok((graph.id(&id)?, path)))
                        .collect::<Result<_>>()?;
                    branch.index_children = branch
                        .index_children
                        .into_iter()
                        .map(|(id, reference)| {
                            Ok((
                                graph.id(&id)?,
                                DirectoryGitChildReference {
                                    directory_branch_id: graph.id(&reference.directory_branch_id)?,
                                    tree_id: reference
                                        .tree_id
                                        .as_deref()
                                        .map(|id| graph.id(id))
                                        .transpose()?,
                                },
                            ))
                        })
                        .collect::<Result<_>>()?;
                    Ok((graph.id(&id)?, branch))
                })
                .collect::<Result<_>>()?;
            // Deleted children remain represented by mapped retained markers. The
            // importing operation owns these copies; original audit remains in the
            // per-file retained prefix instead of fabricating old operations.
            git.deletions = git
                .deletions
                .into_iter()
                .map(|(id, mut node)| {
                    node.node_id = graph.id(&node.node_id)?;
                    node.repository_id = target.repository_id.clone();
                    node.history_tree_id = target.history_tree_id.clone();
                    node.owner_directory_id = graph.id(&node.owner_directory_id)?;
                    node.directory_id = graph.id(&node.directory_id)?;
                    node.namespace_head = graph.id(&node.namespace_head)?;
                    node.directory_branch_id = graph.id(&node.directory_branch_id)?;
                    node.source_tree_id = graph.id(&node.source_tree_id)?;
                    node.operation_id = event.operation;
                    node.author = event.principal.into();
                    node.sequence = next.sequence;
                    node.created_at = event.created_at;
                    node.files = node
                        .files
                        .into_iter()
                        .map(|(file, mut member)| {
                            member.deletion_node_id = graph.id(&member.deletion_node_id)?;
                            Ok((graph.id(&file)?, member))
                        })
                        .collect::<Result<_>>()?;
                    imported.push(node.clone());
                    Ok((graph.id(&id)?, node))
                })
                .collect::<Result<_>>()?;
            next.directory_git_repositories
                .insert(git.directory_id.clone(), git);
        }
        let id = graph.id(directory)?;
        if let Some(branch) = &member.selected_branch {
            next.directory_selections
                .insert(id.clone(), graph.id(branch)?);
        }
        if let Some(tree) = &member.selected_tree {
            next.directory_tree_selections
                .insert(id.clone(), graph.id(tree)?);
        }
        if let Some(generation) = member.selection_generation {
            next.directory_selection_generations.insert(id, generation);
        }
    }
    Ok(imported)
}

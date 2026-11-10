//! Reconstruct the current D table from retained create/undo evidence. This is
//! cold State admission, not authentication of a rewritten publication root.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn is_lifecycle(kind: &str) -> bool {
    matches!(
        kind,
        "file.delete"
            | "file.delete.undo"
            | crate::business::FILE_HISTORY_MOVE_KIND
            | storage_contract::file_relocation::OBJECT_MOVE_KIND
            | crate::file_deletion::batch::KIND
            | crate::file_deletion::directory::DELETE
            | crate::file_deletion::directory::UNDO
    )
}

pub(crate) fn admit_current(state: &State) -> Result<()> {
    let mut receipts = BTreeMap::new();
    let mut operations = BTreeSet::new();
    for (id, receipt) in state.operations.iter() {
        if !is_lifecycle(&receipt.kind) {
            continue;
        }
        if receipt.operation_id != *id
            || receipts.insert(receipt.sequence, (*id, receipt)).is_some()
        {
            return Err(invalid());
        }
        operations.insert(*id);
    }
    let mut witnessed = BTreeSet::new();
    for event in state.events.iter() {
        if let Some((id, receipt)) = receipts.get(&event.sequence) {
            let owns_object = if matches!(receipt.kind.as_str(),crate::business::FILE_HISTORY_MOVE_KIND | storage_contract::file_relocation::OBJECT_MOVE_KIND) {
                receipt.commit_id.as_deref() == Some(event.object_id.as_str())
            } else {
                event.object_id == id.to_string()
            };
            if event.operation_id != *id
                || event.kind != receipt.kind
                || !owns_object
                || !witnessed.insert(*id)
            {
                return Err(invalid());
            }
        } else if operations.contains(&event.operation_id) || is_lifecycle(&event.kind) {
            return Err(invalid());
        }
    }
    if witnessed.len() != receipts.len() {
        return Err(invalid());
    }
    let mut active = BTreeMap::new();
    let mut created = BTreeSet::new();
    let mut directories =
        BTreeMap::<String, storage_contract::directory_git::DirectoryDeletionNode>::new();
    let mut created_directories = BTreeSet::new();
    for (_, receipt) in receipts.values() {
        if receipt.kind==storage_contract::file_relocation::OBJECT_MOVE_KIND {
            let result=crate::business::object_move::admit_result(state,receipt)?;
            for node in result.source_deletions.into_iter().chain(result.displaced_deletions).chain(result.imported_deletions) {
                if !created.insert(node.node_id.clone()) || active.insert(node.file_id.clone(),node).is_some() {
                    return Err(invalid());
                }
            }
            for node in result.imported_directory_deletions {
                crate::file_deletion::directory::admit_original(state,receipt,&node)?;
                if !created_directories.insert(node.node_id.clone()) || directories.contains_key(&node.directory_id) {
                    return Err(invalid());
                }
                for (file,member) in &node.files {
                    if active.get(file).map(|row:&FileDeletionNode|&row.node_id)!=Some(&member.deletion_node_id) {
                        return Err(invalid());
                    }
                }
                directories.insert(node.directory_id.clone(),node);
            }
            continue;
        }
        if crate::file_deletion::directory::is_kind(&receipt.kind) {
            let group = crate::file_deletion::directory::admit(state, receipt)?;
            if group.undo {
                if directories.remove(&group.node.directory_id).as_ref() != Some(&group.node) {
                    return Err(invalid());
                }
                for (file, member) in &group.node.files {
                    if active
                        .get(file)
                        .map(|node: &FileDeletionNode| &node.node_id)
                        != Some(&member.deletion_node_id)
                    {
                        return Err(invalid());
                    }
                }
                for node in group.files {
                    if directories
                        .values()
                        .any(|directory| directory.files.contains_key(&node.file_id))
                        || active.remove(&node.file_id).as_ref() != Some(&node)
                    {
                        return Err(invalid());
                    }
                }
            } else {
                if !created_directories.insert(group.node.node_id.clone())
                    || directories.contains_key(&group.node.directory_id)
                {
                    return Err(invalid());
                }
                for node in group.files {
                    if !created.insert(node.node_id.clone())
                        || active.insert(node.file_id.clone(), node).is_some()
                    {
                        return Err(invalid());
                    }
                }
                for (file, member) in &group.node.files {
                    if active
                        .get(file)
                        .map(|node: &FileDeletionNode| &node.node_id)
                        != Some(&member.deletion_node_id)
                    {
                        return Err(invalid());
                    }
                }
                directories.insert(group.node.directory_id.clone(), group.node);
            }
            continue;
        }
        if receipt.kind == crate::file_deletion::batch::KIND {
            for node in crate::file_deletion::batch::admit(state, receipt)?.nodes {
                if !created.insert(node.node_id.clone())
                    || active.insert(node.file_id.clone(), node).is_some()
                {
                    return Err(invalid());
                }
            }
            continue;
        }
        if receipt.kind == crate::business::FILE_HISTORY_MOVE_KIND {
            for node in crate::business::admit_file_history_move_deletions(state, receipt)? {
                if !created.insert(node.node_id.clone())
                    || active.insert(node.file_id.clone(), node).is_some()
                {
                    return Err(invalid());
                }
            }
            continue;
        }
        let undo = receipt.kind == "file.delete.undo";
        let node = admit_receipt(state, receipt, undo)?;
        if undo {
            if directories
                .values()
                .any(|directory| directory.files.contains_key(&node.file_id))
                || active.remove(&node.file_id).as_ref() != Some(&node)
            {
                return Err(invalid());
            }
        } else if !created.insert(node.node_id.clone())
            || active.insert(node.file_id.clone(), node).is_some()
        {
            return Err(invalid());
        }
    }
    let mut retained = BTreeMap::new();
    for git in state.directory_git_repositories.values() {
        for (id, node) in &git.deletions {
            if id != &node.directory_id
                || node.owner_directory_id != git.directory_id
                || node.repository_id != git.repository_id
                || retained.insert(id, node).is_some()
            {
                return Err(invalid());
            }
        }
    }
    if directories.iter().ne(retained) || active.iter().ne(state.file_deletions.iter()) {
        return Err(invalid());
    }
    Ok(())
}


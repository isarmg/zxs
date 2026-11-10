//! Admit bodyless current deletions against the exact retained branch frontier.
use super::HistoryManifest;
use crate::{BranchKind, Error, ErrorCode, NamespaceEntryKind, Result};
use std::collections::{BTreeMap, BTreeSet};

fn invalid() -> Error {
    Error::new(
        ErrorCode::HistoryPackageInvalid,
        "file deletion does not join the complete retained file frontier",
    )
}

impl HistoryManifest {
    pub(super) fn validate_file_deletions(&self) -> Result<()> {
        for (file, node) in &self.file_deletions {
            storage_contract::validate_id(&node.node_id, "fdel")?;
            storage_contract::validate_id(file, "file")?;
            if node.file_id != *file
                || node.repository_id != self.repository.repository_id
                || node.history_tree_id != self.history.history_tree_id
                || node.author.is_empty()
            {
                return Err(invalid());
            }
            let mut heads = BTreeMap::new();
            for branch in self
                .branches
                .values()
                .filter(|branch| branch.file_id.as_ref() == Some(file))
            {
                if branch.branch_kind != BranchKind::File {
                    return Err(invalid());
                }
                let head = if let Some(head) = &branch.file_node_id {
                    head.clone()
                } else {
                    self.commits
                        .get(&branch.head_commit_id)
                        .and_then(|commit| {
                            commit.entries.iter().find(|entry| {
                                entry.kind == NamespaceEntryKind::File && entry.file_id == *file
                            })
                        })
                        .map(|entry| entry.version_id.clone())
                        .ok_or_else(invalid)?
                };
                let owner = self
                    .versions
                    .get(&head)
                    .map(|row| row.file_id.as_str())
                    .or_else(|| {
                        self.file_path_nodes
                            .get(&head)
                            .map(|row| row.file_id.as_str())
                    })
                    .or_else(|| {
                        self.file_metadata_nodes
                            .get(&head)
                            .map(|row| row.file_id.as_str())
                    });
                if owner != Some(file.as_str()) {
                    return Err(invalid());
                }
                heads.insert(branch.branch_id.clone(), head);
            }
            let parents: Vec<_> = heads
                .values()
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            if heads.is_empty() || heads != node.branch_heads || parents != node.parent_node_ids {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

//! The bounded output of the real FILE and ancestor namespace publishers.
use crate::{Result, State};
use data_serialization::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use storage_contract::{BranchId, CommitId, DirectoryBranchId, DirectoryTreeId, TreeId};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serialization(tag = "kind", content = "id", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Allocation {
    Branch(BranchId),
    Commit(CommitId),
    Tree(TreeId),
    DirectoryBranch(DirectoryBranchId),
    DirectoryTree(DirectoryTreeId),
}

impl Allocation {
    pub(super) fn generate(prefix: &str) -> Result<Self> {
        match prefix {
            "br" => Ok(Self::Branch(BranchId::generate()?)),
            "commit" => Ok(Self::Commit(CommitId::generate()?)),
            "tree" => Ok(Self::Tree(TreeId::generate()?)),
            "dbr" => Ok(Self::DirectoryBranch(DirectoryBranchId::generate()?)),
            "dtree" => Ok(Self::DirectoryTree(DirectoryTreeId::generate()?)),
            _ => Err(super::invalid()),
        }
    }

    pub(super) fn text(self, prefix: &str) -> Result<String> {
        let (kind, text) = match self {
            Self::Branch(id) => ("br", id.to_string()),
            Self::Commit(id) => ("commit", id.to_string()),
            Self::Tree(id) => ("tree", id.to_string()),
            Self::DirectoryBranch(id) => ("dbr", id.to_string()),
            Self::DirectoryTree(id) => ("dtree", id.to_string()),
        };
        if kind != prefix {
            return Err(super::invalid());
        }
        Ok(text)
    }
}

pub(super) struct Replay {
    pending: VecDeque<Allocation>,
}
impl Replay {
    pub(super) fn new(previous: &State, values: &[Allocation]) -> Result<Self> {
        let mut ids = BTreeSet::new();
        let mut trees = BTreeSet::new();
        for id in values {
            let fresh = match id {
                Allocation::Branch(id) => !previous.branches.contains_key(&id.to_string()),
                Allocation::Commit(id) => !previous.commits.contains_key(&id.to_string()),
                Allocation::Tree(id) => trees.insert(*id),
                Allocation::DirectoryBranch(id) => {
                    !previous.directory_branches.contains_key(&id.to_string())
                }
                Allocation::DirectoryTree(id) => {
                    !previous.directory_trees.contains_key(&id.to_string())
                }
            };
            if !fresh || !ids.insert(*id) {
                return Err(super::invalid());
            }
        }
        if !trees.is_empty() {
            for commit in previous.commits.values() {
                if trees.contains(&commit.tree_id.parse::<TreeId>()?) {
                    return Err(super::invalid());
                }
            }
        }
        Ok(Self { pending: values.iter().copied().collect() })
    }

    pub(super) fn next(&mut self, prefix: &str) -> Result<String> {
        self.pending.pop_front().ok_or_else(super::invalid)?.text(prefix)
    }

    pub(super) fn finish(self) -> Result<()> {
        if !self.pending.is_empty() {
            return Err(super::invalid());
        }
        Ok(())
    }
}

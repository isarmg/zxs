//! The bounded output of the one real root-level FILE publisher.
use crate::{Result, State};
use data_serialization::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use storage_contract::{BranchId, CommitId, TreeId, file_draft::DraftLocation};

/// Each command has one native FILE and one ordinary namespace root. Check the
/// target's actual root before accepting any allocation vector from storage.
pub(crate) enum PublicationGraph {
    Revised,
    First,
}

impl PublicationGraph {
    pub(crate) fn limit(self, state: &State, target: &DraftLocation) -> Result<usize> {
        let repo = state.repositories.get(&target.repository.to_string()).ok_or_else(super::invalid)?;
        let main = state.branches.get(&repo.main_branch_id).ok_or_else(super::invalid)?;
        let head = state.commits.get(&main.head_commit_id).ok_or_else(super::invalid)?;
        if main.history_tree_id != repo.history_tree_id || head.history_tree_id != main.history_tree_id {
            return Err(super::invalid());
        }
        Ok(match self {
            // The two Commit/Tree pairs advance the existing FILE and the owning namespace.
            Self::Revised => 4,
            // Creation additionally allocates this FILE's own independent branch.
            Self::First => 5,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serialization(tag = "kind", content = "id", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Allocation {
    Branch(BranchId),
    Commit(CommitId),
    Tree(TreeId),
}

impl Allocation {
    pub(super) fn generate(prefix: &str) -> Result<Self> {
        match prefix {
            "br" => Ok(Self::Branch(BranchId::generate()?)),
            "commit" => Ok(Self::Commit(CommitId::generate()?)),
            "tree" => Ok(Self::Tree(TreeId::generate()?)),
            _ => Err(super::invalid()),
        }
    }

    pub(super) fn text(self, prefix: &str) -> Result<String> {
        let (kind, text) = match self {
            Self::Branch(id) => ("br", id.to_string()),
            Self::Commit(id) => ("commit", id.to_string()),
            Self::Tree(id) => ("tree", id.to_string()),
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

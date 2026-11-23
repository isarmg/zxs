//! Selected container metadata, branches, and immutable complete nodes.
use crate::display;
use std::{collections::BTreeMap, fmt, io};

pub(crate) type Id = [u8; 16];
pub(crate) type NodeId = Id;
pub(crate) const MAIN: &str = "main";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Mutation {
    pub(crate) container: Id,
    pub(crate) request: Id,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Target {
    Branch {
        name: String,
        expected_head: Option<NodeId>,
    },
    HistoricalMain {
        new_branch: String,
        original: NodeId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Body {
    pub(crate) length: u64,
    pub(crate) sha256: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Change {
    Save {
        target: Target,
        node: NodeId,
        body: Body,
        initial_name: Option<String>,
    },
    Branch {
        name: String,
        from: NodeId,
    },
    Rename {
        expected_sequence: u64,
        name: String,
    },
    Tags {
        expected_sequence: u64,
        tags: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Receipt {
    pub(crate) mutation: Mutation,
    pub(crate) sequence: u64,
    pub(crate) change: Change,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Node {
    pub(crate) prior: Option<NodeId>,
    pub(crate) original_branch: String,
    pub(crate) body: Body,
    pub(crate) sequence: u64,
    pub(crate) request: Id,
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) name: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) branches: BTreeMap<String, Option<NodeId>>,
    pub(crate) nodes: BTreeMap<NodeId, Node>,
    pub(crate) receipts: BTreeMap<Id, Receipt>,
    pub(crate) sequence: u64,
}

pub(crate) enum Conflict {
    AlreadyExists,
    MissingContainer,
    MissingBranch,
    MissingNode,
    HeadChanged(Option<NodeId>),
    SequenceChanged(u64),
    NotHistoricalMain,
    RequestReused,
    Invalid(io::Error),
}

// History qualification uses this diagnostic. Keep the observed head and
// original error typed, but never render the ID or the underlying message.
impl fmt::Debug for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyExists => f.write_str("AlreadyExists"),
            Self::MissingContainer => f.write_str("MissingContainer"),
            Self::MissingBranch => f.write_str("MissingBranch"),
            Self::MissingNode => f.write_str("MissingNode"),
            Self::HeadChanged(observed) => f
                .debug_struct("HeadChanged")
                .field("head_exists", &observed.is_some())
                .finish(),
            Self::SequenceChanged(observed) => {
                f.debug_tuple("SequenceChanged").field(observed).finish()
            }
            Self::NotHistoricalMain => f.write_str("NotHistoricalMain"),
            Self::RequestReused => f.write_str("RequestReused"),
            Self::Invalid(error) => f.debug_tuple("Invalid").field(&error.kind()).finish(),
        }
    }
}

impl From<io::Error> for Conflict {
    fn from(value: io::Error) -> Self {
        Self::Invalid(value)
    }
}

fn label(value: &str) -> Result<(), Conflict> {
    display::component(value.as_bytes(), display::MAX)
        .map(|_| ())
        .map_err(Into::into)
}
fn identity(value: Id) -> Result<(), Conflict> {
    if value == [0; 16] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "zero mutation or node identity",
        )
        .into());
    }
    Ok(())
}

impl State {
    /// Returns the original result on exact replay, never another node.
    pub(crate) fn replay(
        &self,
        mutation: Mutation,
        change: &Change,
    ) -> Result<Option<&Receipt>, Conflict> {
        identity(mutation.container)?;
        identity(mutation.request)?;
        match self.receipts.get(&mutation.request) {
            None => Ok(None),
            Some(receipt) if receipt.mutation == mutation && receipt.change == *change => {
                Ok(Some(receipt))
            }
            Some(_) => Err(Conflict::RequestReused),
        }
    }

    pub(crate) fn next(&self, mutation: Mutation, change: Change) -> Result<Receipt, Conflict> {
        if let Some(original) = self.replay(mutation, &change)? {
            return Ok(original.clone());
        }
        let receipt = Receipt {
            mutation,
            sequence: self
                .sequence
                .checked_add(1)
                .ok_or_else(|| io::Error::other("container sequence exhausted"))?,
            change,
        };
        self.validate(&receipt)?;
        Ok(receipt)
    }

    fn ancestor_of_main(&self, target: NodeId) -> Result<bool, Conflict> {
        let mut current = self.branches.get(MAIN).copied().flatten();
        // Traversal is bounded by the actual immutable history. Each node
        // has an earlier position, so no arbitrary depth limit is added.
        let mut before = self.sequence.saturating_add(1);
        while let Some(id) = current {
            let node = self.nodes.get(&id).ok_or(Conflict::MissingNode)?;
            if node.sequence >= before {
                return Err(io::Error::other("unordered original node history").into());
            }
            if id == target {
                return Ok(true);
            }
            before = node.sequence;
            current = node.prior;
        }
        Ok(false)
    }

    fn validate(&self, receipt: &Receipt) -> Result<(), Conflict> {
        identity(receipt.mutation.container)?;
        identity(receipt.mutation.request)?;
        if self.sequence.checked_add(1) != Some(receipt.sequence)
            || self.receipts.contains_key(&receipt.mutation.request)
        {
            return Err(Conflict::RequestReused);
        }
        if !matches!(receipt.change, Change::Save { .. }) && self.name.is_none() {
            return Err(Conflict::MissingContainer);
        }
        match &receipt.change {
            Change::Rename {
                expected_sequence,
                name,
            } => {
                label(name)?;
                if *expected_sequence != self.sequence {
                    return Err(Conflict::SequenceChanged(self.sequence));
                }
            }
            Change::Tags {
                expected_sequence,
                tags,
            } => {
                if *expected_sequence != self.sequence {
                    return Err(Conflict::SequenceChanged(self.sequence));
                }
                super::encode_tags(tags)?;
            }
            Change::Branch { name, from } => {
                label(name)?;
                if self.branches.contains_key(name) {
                    return Err(Conflict::AlreadyExists);
                }
                if !self.nodes.contains_key(from) {
                    return Err(Conflict::MissingNode);
                }
            }
            Change::Save {
                target,
                node,
                initial_name,
                ..
            } => {
                identity(*node)?;
                if self.nodes.contains_key(node) {
                    return Err(Conflict::AlreadyExists);
                }
                if self.name.is_none() {
                    if let (
                        Some(display),
                        Target::Branch {
                            name,
                            expected_head: None,
                        },
                    ) = (initial_name, target)
                        && name == MAIN
                    {
                        return label(display);
                    }
                    return Err(Conflict::MissingContainer);
                }
                if initial_name.is_some() {
                    return Err(Conflict::AlreadyExists);
                }
                match target {
                    Target::Branch {
                        name,
                        expected_head,
                    } => {
                        let current = self.branches.get(name).ok_or(Conflict::MissingBranch)?;
                        if current != expected_head {
                            return Err(Conflict::HeadChanged(*current));
                        }
                    }
                    Target::HistoricalMain {
                        new_branch,
                        original,
                    } => {
                        label(new_branch)?;
                        if self.branches.contains_key(new_branch) {
                            return Err(Conflict::AlreadyExists);
                        }
                        if self.branches.get(MAIN).copied().flatten() == Some(*original)
                            || !self.ancestor_of_main(*original)?
                        {
                            return Err(Conflict::NotHistoricalMain);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Used after durable commit and while reconstructing an exact ledger.
    pub(crate) fn select(&mut self, receipt: Receipt) -> Result<(), Conflict> {
        self.validate(&receipt)?;
        match &receipt.change {
            Change::Rename { name, .. } => self.name = Some(name.clone()),
            Change::Tags { tags, .. } => self.tags.clone_from(tags),
            Change::Branch { name, from } => {
                self.branches.insert(name.clone(), Some(*from));
            }
            Change::Save {
                target,
                node,
                body,
                initial_name,
            } => {
                if let Some(name) = initial_name {
                    self.name = Some(name.clone());
                }
                let (branch, prior) = match target {
                    Target::Branch {
                        name,
                        expected_head,
                    } => (name, *expected_head),
                    Target::HistoricalMain {
                        new_branch,
                        original,
                    } => (new_branch, Some(*original)),
                };
                self.nodes.insert(
                    *node,
                    Node {
                        prior,
                        original_branch: branch.clone(),
                        body: body.clone(),
                        sequence: receipt.sequence,
                        request: receipt.mutation.request,
                    },
                );
                self.branches.insert(branch.clone(), Some(*node));
            }
        }
        self.sequence = receipt.sequence;
        self.receipts.insert(receipt.mutation.request, receipt);
        Ok(())
    }
}

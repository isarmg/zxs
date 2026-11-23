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
    FirstMain {
        name: String,
    },
    Branch {
        name: String,
        expected_head: NodeId,
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
    pub(crate) branches: BTreeMap<String, NodeId>,
    pub(crate) nodes: BTreeMap<NodeId, Node>,
    pub(crate) receipts: BTreeMap<Id, Receipt>,
    pub(crate) sequence: u64,
}

pub(crate) enum Conflict {
    AlreadyExists,
    MissingContainer,
    MissingBranch,
    MissingNode,
    HeadChanged(NodeId),
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
                .field("head_exists", &(*observed != [0; 16]))
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

/// A new receipt is bound to the exact mutable selection that admitted it.
/// Dropping it before durable publication leaves that selection untouched.
pub(super) struct Pending<'a> {
    state: &'a mut State,
    receipt: Receipt,
}

pub(super) enum Decision<'a> {
    Replay(Receipt),
    New(Pending<'a>),
}

impl Pending<'_> {
    pub(super) fn receipt(&self) -> &Receipt {
        &self.receipt
    }

    /// The caller holds the container lock through publication and parent sync.
    pub(super) fn commit(self) -> Receipt {
        self.state.apply(self.receipt.clone());
        self.receipt
    }
}

impl State {
    pub(super) fn plan(
        &mut self,
        mutation: Mutation,
        change: Change,
    ) -> Result<Decision<'_>, Conflict> {
        if let Some(prior) = self.receipts.get(&mutation.request) {
            return if prior.mutation == mutation && prior.change == change {
                Ok(Decision::Replay(prior.clone()))
            } else {
                Err(Conflict::RequestReused)
            };
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
        Ok(Decision::New(Pending {
            state: self,
            receipt,
        }))
    }

    /// First-main creates the only root. Main can only append at its current
    /// head, and no command rewrites a parent or moves an existing branch.
    /// Thus a node born on main, except its current head, is precisely a
    /// historical main node. Forks share ancestors, not the birth branch.
    fn historical_main(&self, id: NodeId) -> bool {
        self.branches.get(MAIN).copied() != Some(id)
            && self
                .nodes
                .get(&id)
                .is_some_and(|node| node.original_branch == MAIN)
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
            Change::Save { target, node, .. } => {
                identity(*node)?;
                if self.nodes.contains_key(node) {
                    return Err(Conflict::AlreadyExists);
                }
                if self.name.is_none() {
                    return match target {
                        Target::FirstMain { name } => label(name),
                        _ => Err(Conflict::MissingContainer),
                    };
                }
                match target {
                    Target::FirstMain { .. } => return Err(Conflict::AlreadyExists),
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
                        if !self.historical_main(*original) {
                            return Err(Conflict::NotHistoricalMain);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// A record read from disk has no live plan: qualify it at this boundary.
    pub(super) fn recover(&mut self, receipt: Receipt) -> Result<(), Conflict> {
        self.validate(&receipt)?;
        self.apply(receipt);
        Ok(())
    }

    fn apply(&mut self, receipt: Receipt) {
        match &receipt.change {
            Change::Rename { name, .. } => self.name = Some(name.clone()),
            Change::Tags { tags, .. } => self.tags.clone_from(tags),
            Change::Branch { name, from } => {
                self.branches.insert(name.clone(), *from);
            }
            Change::Save { target, node, body } => {
                let (branch, prior) = match target {
                    Target::FirstMain { name } => {
                        self.name = Some(name.clone());
                        (MAIN, None)
                    }
                    Target::Branch {
                        name,
                        expected_head,
                    } => (name.as_str(), Some(*expected_head)),
                    Target::HistoricalMain {
                        new_branch,
                        original,
                    } => (new_branch.as_str(), Some(*original)),
                };
                self.nodes.insert(
                    *node,
                    Node {
                        prior,
                        original_branch: branch.to_owned(),
                        body: body.clone(),
                        sequence: receipt.sequence,
                        request: receipt.mutation.request,
                    },
                );
                self.branches.insert(branch.to_owned(), *node);
            }
        }
        self.sequence = receipt.sequence;
        self.receipts.insert(receipt.mutation.request, receipt);
    }
}

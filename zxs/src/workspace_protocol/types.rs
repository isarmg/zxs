use super::hex;
use std::io::{Read, Take};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Id(pub(super) [u8; 16]);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Branch {
    Main,
    Named(Id),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Rejected {
    Route,
    Header,
    Value,
}

impl Id {
    pub(crate) fn bytes(self) -> [u8; 16] {
        self.0
    }
    pub(super) fn parse(s: &str) -> Result<Self, Rejected> {
        let value = hex(s)?;
        if value == [0; 16] {
            return Err(Rejected::Value);
        }
        Ok(Self(value))
    }
}
impl Branch {
    pub(super) fn parse(s: &str) -> Result<Self, Rejected> {
        if s == "main" {
            return Ok(Self::Main);
        }
        Id::parse(s).map(Self::Named)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Target {
    FirstMain { name: String },
    Branch { branch: Branch, expected_head: Id },
    HistoricalMain { new_branch: Id, original: Id },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SaveNode {
    pub(crate) container: Id,
    pub(crate) operation: Id,
    pub(crate) node: Id,
    pub(crate) target: Target,
    pub(crate) length: u64,
    pub(crate) sha256: [u8; 32],
}
impl SaveNode {
    // The store must read the entire bounded body, checking exact length
    // and hash BEFORE publishing a new node. No resume offsets or deltas.
    pub(crate) fn body<R: Read>(&self, source: R) -> Take<R> {
        source.take(self.length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MetadataMutation {
    pub(crate) container: Id,
    pub(crate) operation: Id,
    pub(crate) revision: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JsonMutation {
    Rename,
    ReplaceTags,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Request {
    ListCompact {
        after: Option<Id>,
    },
    ContainerDetail {
        container: Id,
        sequence: Option<u64>,
        after: Option<Branch>,
    },
    Tags(Id),
    NodeData {
        container: Id,
        node: Id,
    },
    BranchHistory {
        container: Id,
        branch: Branch,
        head: Id,
        next: Option<Id>,
    },
    Receipt {
        container: Id,
        operation: Id,
    },
    SaveNode(SaveNode),
    Delete(MetadataMutation),
    CreateBranch {
        container: Id,
        operation: Id,
        branch: Id,
        length: u64,
    },
    Json {
        scope: MetadataMutation,
        mutation: JsonMutation,
        length: u64,
    },
}

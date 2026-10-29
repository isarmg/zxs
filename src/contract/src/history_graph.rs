//! Bounded inspection of published history metadata. These are wire views,
//! not replacements for the authoritative history records.
use crate::{BranchKind, BranchState, CommitNamespace, HistoryState, OperationId};
use data_serialization::{Deserialize, Serialize};

/// Maximum number of records in one graph or auxiliary list response.
pub const MAX_GRAPH_RECORDS: usize = 128;
pub const MAX_GRAPH_TEXT_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageLimit(usize);
impl PageLimit {
    pub fn new(value: usize) -> crate::Result<Self> {
        if !(1..=MAX_GRAPH_RECORDS).contains(&value) {
            return Err(crate::Error::new(
                crate::ErrorCode::InvalidRequest,
                "graph page limit must be between 1 and 128",
            ));
        }
        Ok(Self(value))
    }
    pub fn get(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextLimit(usize);
impl TextLimit {
    pub fn new(value: usize) -> crate::Result<Self> {
        if !(4..=MAX_GRAPH_TEXT_BYTES).contains(&value) {
            return Err(crate::Error::new(
                crate::ErrorCode::InvalidRequest,
                "graph text limit must be between 4 and 16384 bytes",
            ));
        }
        Ok(Self(value))
    }
    pub fn get(self) -> usize {
        self.0
    }
}

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
        #[serialization(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }
        }
        impl std::str::FromStr for $name {
            type Err = crate::Error;
            fn from_str(value: &str) -> crate::Result<Self> {
                match value {
                    $($wire => Ok(Self::$variant),)+
                    _ => Err(crate::Error::new(crate::ErrorCode::InvalidRequest, concat!("invalid ", stringify!($name)))),
                }
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: data_serialization::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                String::deserialize(deserializer)?.parse().map_err(data_serialization::de::Error::custom)
            }
        }
    };
}
string_enum!(GraphCollection {
    Commits => "commits", Branches => "branches", MergeLinks => "merge_links", BoundaryMarks => "boundary_marks"
});
string_enum!(HistoryField { Provenance => "provenance" });
string_enum!(CommitField {
    Kind => "kind", TreeId => "tree_id", Author => "author", Message => "message", Provenance => "provenance"
});
string_enum!(MergeLinkField { Author => "author" });
string_enum!(BoundaryField { Relation => "relation", SourceMergeLinkId => "source_merge_link_id" });

/// Continuation keys have collection-specific meaning and are admitted by the
/// request boundary against the exact history root, never as authorization.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphKey {
    Commit {
        #[serialization(with = "crate::u64_string")]
        sequence: u64,
        commit_id: String,
    },
    Id {
        id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TextSummary {
    Inline {
        text: String,
    },
    Deferred {
        #[serialization(with = "crate::u64_string")]
        bytes: u64,
    },
}
impl TextSummary {
    pub fn from_text(text: &str) -> Self {
        if text.len() <= crate::client_protocol::MAX_INLINE_TEXT_BYTES {
            Self::Inline {
                text: text.to_owned(),
            }
        } else {
            Self::Deferred {
                bytes: text.len() as u64,
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistorySummary {
    pub history_tree_id: String,
    pub repository_id: String,
    pub root_commit_id: String,
    #[serialization(deserialize_with = "required_nullable")]
    pub main_branch_id: Option<String>,
    #[serialization(with = "crate::u64_string")]
    pub generation: u64,
    pub state: HistoryState,
    #[serialization(deserialize_with = "required_nullable")]
    pub provenance: Option<TextSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CommitSummary {
    pub commit_id: String,
    pub history_tree_id: String,
    pub kind: TextSummary,
    pub parent_commit_ids: Vec<String>,
    pub tree_id: TextSummary,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    pub operation_id: OperationId,
    pub author: TextSummary,
    pub message: TextSummary,
    pub namespace: CommitNamespace,
    #[serialization(deserialize_with = "required_nullable")]
    pub namespace_deletions: Option<DeletionSummary>,
    #[serialization(with = "crate::u64_string")]
    pub pending_file_versions: u64,
    #[serialization(deserialize_with = "required_nullable")]
    pub provenance: Option<TextSummary>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DeletionSummary {
    #[serialization(with = "crate::u64_string")]
    pub timestamp: u64,
    #[serialization(with = "crate::u64_string")]
    pub paths: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BranchSummary {
    pub branch_id: String,
    pub history_tree_id: String,
    pub name: String,
    pub head_commit_id: String,
    pub fork_commit_id: String,
    pub branch_kind: BranchKind,
    #[serialization(deserialize_with = "required_nullable")]
    pub file_id: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    pub file_node_id: Option<String>,
    #[serialization(with = "crate::u64_string")]
    pub pending_file_versions: u64,
    #[serialization(with = "crate::u64_string")]
    pub generation: u64,
    pub state: BranchState,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct MergeLinkSummary {
    pub merge_link_id: String,
    pub history_tree_id: String,
    pub source_branch_id: String,
    pub source_commit_id: String,
    pub target_branch_id: String,
    pub target_commit_id: String,
    pub author: TextSummary,
    pub operation_id: OperationId,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BoundarySummary {
    pub boundary_id: String,
    pub history_tree_id: String,
    pub local_commit_id: String,
    #[serialization(deserialize_with = "required_nullable")]
    pub local_branch_id: Option<String>,
    pub relation: TextSummary,
    pub external_history_tree_id: String,
    pub external_commit_id: String,
    #[serialization(deserialize_with = "required_nullable")]
    pub source_merge_link_id: Option<TextSummary>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(
    tag = "collection",
    content = "records",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum GraphItems {
    Commits(Vec<CommitSummary>),
    Branches(Vec<BranchSummary>),
    MergeLinks(Vec<MergeLinkSummary>),
    BoundaryMarks(Vec<BoundarySummary>),
}
impl GraphItems {
    pub fn collection(&self) -> GraphCollection {
        match self {
            Self::Commits(_) => GraphCollection::Commits,
            Self::Branches(_) => GraphCollection::Branches,
            Self::MergeLinks(_) => GraphCollection::MergeLinks,
            Self::BoundaryMarks(_) => GraphCollection::BoundaryMarks,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct GraphPage {
    pub history: HistorySummary,
    pub history_root: String,
    pub items: GraphItems,
    #[serialization(deserialize_with = "required_nullable")]
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct PendingRecord {
    pub file_id: String,
    pub version_id: String,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/history_graph.rs"]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TextTarget {
    History { field: HistoryField },
    Commit { id: String, field: CommitField },
    MergeLink { id: String, field: MergeLinkField },
    Boundary { id: String, field: BoundaryField },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PendingTarget {
    Commit { id: String },
    Branch { id: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TextPage {
    pub history_tree_id: String,
    pub history_root: String,
    pub target: TextTarget,
    #[serialization(with = "crate::u64_string")]
    pub total_bytes: u64,
    #[serialization(with = "crate::u64_string")]
    pub offset: u64,
    #[serialization(deserialize_with = "required_nullable")]
    pub text: Option<String>,
    #[serialization(with = "crate::optional_u64_string")]
    pub next_offset: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct PendingPage {
    pub history_tree_id: String,
    pub history_root: String,
    pub target: PendingTarget,
    pub records: Vec<PendingRecord>,
    #[serialization(deserialize_with = "required_nullable")]
    pub next_after: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DeletionsPage {
    pub history_tree_id: String,
    pub history_root: String,
    pub commit_id: String,
    #[serialization(with = "crate::optional_u64_string")]
    pub timestamp: Option<u64>,
    #[serialization(with = "crate::u64_string")]
    pub total: u64,
    #[serialization(with = "crate::u64_string")]
    pub offset: u64,
    pub records: Vec<crate::DeletedPath>,
    #[serialization(with = "crate::optional_u64_string")]
    pub next_offset: Option<u64>,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: data_serialization::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

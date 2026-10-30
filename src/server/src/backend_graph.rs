//! Typed, root-bound requests and bounded responses for history inspection.
use crate::{
    ApiError,
    backend_file_history::{HistoryRootText, QueryFields, RequestId, required_root},
};
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeMap;
use storage_contract::history_graph::{
    BoundaryField, CommitField, DeletionsPage, GraphCollection, GraphKey, GraphPage, HistoryField,
    MergeLinkField, PageLimit, PendingPage, PendingTarget, TextLimit, TextPage, TextTarget,
};

pub struct ReadHistoryGraph {
    history: RequestId,
    root: Option<HistoryRootText>,
    selection: GraphRead,
}
pub enum GraphRead {
    Page {
        collection: GraphCollection,
        after: Option<GraphKey>,
        limit: PageLimit,
    },
    Text {
        target: TextTarget,
        offset: usize,
        limit: TextLimit,
    },
    Pending {
        target: PendingTarget,
        after: Option<String>,
        limit: PageLimit,
    },
    Deletions {
        commit_id: String,
        offset: usize,
        limit: PageLimit,
    },
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct GraphCursor {
    v: u8,
    history_tree_id: String,
    history_root: String,
    collection: GraphCollection,
    limit: usize,
    key: GraphKey,
}

fn enum_field<T: std::str::FromStr<Err = storage_contract::Error>>(
    text: String,
) -> Result<T, ApiError> {
    text.parse().map_err(ApiError::contract_input)
}
fn bounded_id(name: &'static str, value: String) -> Result<String, ApiError> {
    Ok(RequestId::parse(name, value)?.into_string())
}
fn limit_value(query: &mut QueryFields, default: usize) -> Result<usize, ApiError> {
    match query.take("limit") {
        Some(value) => storage_contract::parse_u64_decimal(&value)
            .map_err(ApiError::contract_input)
            .and_then(|value| usize::try_from(value).map_err(ApiError::invalid_source)),
        None => Ok(default),
    }
}

impl ReadHistoryGraph {
    pub fn history_id(&self) -> &str {
        self.history.as_str()
    }
    pub fn root(&self) -> Option<&str> {
        self.root.as_ref().map(HistoryRootText::as_str)
    }
    pub fn selection(&self) -> &GraphRead {
        &self.selection
    }
    #[cfg(feature = "core")]
    pub(crate) fn into_parts(self) -> (String, Option<HistoryRootText>, GraphRead) {
        (self.history.into_string(), self.root, self.selection)
    }
    pub fn page(history: String, query: BTreeMap<String, String>) -> Result<Self, ApiError> {
        let history = RequestId::parse("history", history)?;
        let mut query = QueryFields::strict(query, &["collection", "limit", "cursor"])?;
        let collection = enum_field(
            query
                .take("collection")
                .ok_or_else(|| ApiError::invalid("collection is required"))?,
        )?;
        let limit =
            PageLimit::new(limit_value(&mut query, 50)?).map_err(ApiError::contract_input)?;
        let cursor = query
            .take("cursor")
            .map(|value| crate::backend_file_history::cursors::decode::<GraphCursor>(&value))
            .transpose()?;
        let (root, after) = match cursor {
            Some(cursor) => {
                if cursor.v != 1
                    || cursor.history_tree_id != history.as_str()
                    || cursor.collection != collection
                    || cursor.limit != limit.get()
                {
                    return Err(ApiError::invalid(
                        "cursor does not match the history graph query",
                    ));
                }
                let key = match (collection, cursor.key) {
                    (
                        GraphCollection::Commits,
                        GraphKey::Commit {
                            sequence,
                            commit_id,
                        },
                    ) => GraphKey::Commit {
                        sequence,
                        commit_id: bounded_id("commit_id", commit_id)?,
                    },
                    (
                        GraphCollection::Branches
                        | GraphCollection::MergeLinks
                        | GraphCollection::BoundaryMarks,
                        GraphKey::Id { id },
                    ) => GraphKey::Id {
                        id: bounded_id("id", id)?,
                    },
                    (GraphCollection::Commits, GraphKey::Id { .. })
                    | (
                        GraphCollection::Branches
                        | GraphCollection::MergeLinks
                        | GraphCollection::BoundaryMarks,
                        GraphKey::Commit { .. },
                    ) => {
                        return Err(ApiError::invalid(
                            "cursor key does not match its collection",
                        ));
                    }
                };
                (
                    Some(HistoryRootText::parse(&cursor.history_root)?),
                    Some(key),
                )
            }
            None => (None, None),
        };
        Ok(Self {
            history,
            root,
            selection: GraphRead::Page {
                collection,
                after,
                limit,
            },
        })
    }
    fn text(
        history: String,
        target: TextTarget,
        query: BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        let mut query = QueryFields::strict(query, &["history_root", "offset", "limit"])?;
        Ok(Self {
            history: RequestId::parse("history", history)?,
            root: Some(required_root(query.root()?)?),
            selection: GraphRead::Text {
                target,
                offset: query.offset()?,
                limit: TextLimit::new(limit_value(&mut query, 16_384)?)
                    .map_err(ApiError::contract_input)?,
            },
        })
    }
    pub fn history_field(
        history: String,
        field: String,
        query: BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        Self::text(
            history,
            TextTarget::History {
                field: enum_field::<HistoryField>(field)?,
            },
            query,
        )
    }
    pub fn commit_field(
        history: String,
        id: String,
        field: String,
        query: BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        Self::text(
            history,
            TextTarget::Commit {
                id: bounded_id("commit_id", id)?,
                field: enum_field::<CommitField>(field)?,
            },
            query,
        )
    }
    pub fn merge_link_field(
        history: String,
        id: String,
        field: String,
        query: BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        Self::text(
            history,
            TextTarget::MergeLink {
                id: bounded_id("merge_link_id", id)?,
                field: enum_field::<MergeLinkField>(field)?,
            },
            query,
        )
    }
    pub fn boundary_field(
        history: String,
        id: String,
        field: String,
        query: BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        Self::text(
            history,
            TextTarget::Boundary {
                id: bounded_id("boundary_id", id)?,
                field: enum_field::<BoundaryField>(field)?,
            },
            query,
        )
    }
    fn pending(
        history: String,
        target: PendingTarget,
        query: BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        let mut query = QueryFields::strict(query, &["history_root", "after", "limit"])?;
        Ok(Self {
            history: RequestId::parse("history", history)?,
            root: Some(required_root(query.root()?)?),
            selection: GraphRead::Pending {
                target,
                after: query
                    .take("after")
                    .map(|value| bounded_id("file_id", value))
                    .transpose()?,
                limit: PageLimit::new(limit_value(&mut query, 128)?)
                    .map_err(ApiError::contract_input)?,
            },
        })
    }
    pub fn commit_pending(
        history: String,
        id: String,
        query: BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        Self::pending(
            history,
            PendingTarget::Commit {
                id: bounded_id("commit_id", id)?,
            },
            query,
        )
    }
    pub fn branch_pending(
        history: String,
        id: String,
        query: BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        Self::pending(
            history,
            PendingTarget::Branch {
                id: bounded_id("branch_id", id)?,
            },
            query,
        )
    }
    pub fn deletions(
        history: String,
        id: String,
        query: BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        let mut query = QueryFields::strict(query, &["history_root", "offset", "limit"])?;
        Ok(Self {
            history: RequestId::parse("history", history)?,
            root: Some(required_root(query.root()?)?),
            selection: GraphRead::Deletions {
                commit_id: bounded_id("commit_id", id)?,
                offset: query.offset()?,
                limit: PageLimit::new(limit_value(&mut query, 128)?)
                    .map_err(ApiError::contract_input)?,
            },
        })
    }
}

#[derive(Debug, Serialize)]
#[serialization(untagged)]
pub enum HistoryGraphResponse {
    Graph(GraphPage),
    Text(TextPage),
    Pending(PendingPage),
    Deletions(DeletionsPage),
}
impl HistoryGraphResponse {
    pub(crate) fn encode(&self) -> Result<crate::backend_file_history::EncodedPage, ApiError> {
        crate::backend_file_history::encode_bounded(self)
    }
}
#[cfg(any(feature = "core", test))]
pub(crate) fn cursor(
    history: &str,
    root: &str,
    collection: GraphCollection,
    limit: PageLimit,
    key: GraphKey,
) -> Result<String, ApiError> {
    crate::backend_file_history::cursors::encode(&GraphCursor {
        v: 1,
        history_tree_id: history.to_owned(),
        history_root: root.to_owned(),
        collection,
        limit: limit.get(),
        key,
    })
}

// The mutation endpoint retains its complete, unchanged merge-link result.
#[derive(Serialize)]
pub(crate) struct MergeLinkRecord {
    merge_link_id: String,
    history_tree_id: String,
    source_branch_id: String,
    source_commit_id: String,
    target_branch_id: String,
    target_commit_id: String,
    author: String,
    operation_id: storage_contract::OperationId,
}
impl From<storage_contract::MergeLink> for MergeLinkRecord {
    fn from(record: storage_contract::MergeLink) -> Self {
        let storage_contract::MergeLink {
            merge_link_id,
            history_tree_id,
            source_branch_id,
            source_commit_id,
            target_branch_id,
            target_commit_id,
            author,
            operation_id,
        } = record;
        Self {
            merge_link_id,
            history_tree_id,
            source_branch_id,
            source_commit_id,
            target_branch_id,
            target_commit_id,
            author,
            operation_id,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_graph.rs"]
mod tests;

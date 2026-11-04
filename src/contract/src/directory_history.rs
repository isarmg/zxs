//! Bounded, current-root directory composition history. Reads confer no write authority.
use crate::{
    Error, ErrorCode, Result,
    history_window::{HistoryEndpoint, HistoryRoot},
};
use data_serialization::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_DIRECTORY_HISTORY_LIMIT: u32 = 128;
pub const MAX_DIRECTORY_HISTORY_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_CURSOR_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum DirectoryHistoryCollection {
    Nodes,
    Branches,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum DirectoryHistoryDirection {
    Before,
    After,
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
fn invalid_cursor(message: &str) -> Error {
    Error::new(ErrorCode::InvalidCursor, message)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct CursorPayload {
    repository_id: String,
    directory_id: String,
    collection: DirectoryHistoryCollection,
    history_root: HistoryRoot,
    limit: u32,
    direction: DirectoryHistoryDirection,
    #[serialization(with = "crate::u64_string")]
    sequence: u64,
    id: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryHistoryCursor {
    encoded: String,
    payload: CursorPayload,
}
impl DirectoryHistoryCursor {
    fn parse(encoded: &str) -> Result<Self> {
        if encoded.is_empty()
            || encoded.len() > MAX_CURSOR_BYTES
            || !encoded.len().is_multiple_of(2)
        {
            return Err(invalid_cursor("invalid directory history cursor size"));
        }
        let digit = |byte| match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err(invalid_cursor("invalid directory history cursor encoding")),
        };
        let bytes = encoded
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| Ok(digit(pair[0])? * 16 + digit(pair[1])?))
            .collect::<Result<Vec<u8>>>()?;
        let payload: CursorPayload = crate::decode_strict(&bytes)?;
        crate::validate_id(
            &payload.id,
            match payload.collection {
                DirectoryHistoryCollection::Nodes => "dtree",
                DirectoryHistoryCollection::Branches => "dbr",
            },
        )?;
        Ok(Self {
            encoded: encoded.into(),
            payload,
        })
    }
    pub fn sequence(&self) -> u64 {
        self.payload.sequence
    }
    pub fn id(&self) -> &str {
        &self.payload.id
    }
    pub fn direction(&self) -> DirectoryHistoryDirection {
        self.payload.direction
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryHistoryRequest {
    repository_id: String,
    directory_id: String,
    collection: DirectoryHistoryCollection,
    limit: u32,
    history_root: Option<HistoryRoot>,
    cursor: Option<DirectoryHistoryCursor>,
    anchor_tree_id: Option<String>,
}
impl DirectoryHistoryRequest {
    pub fn parse(
        repository: &str,
        directory: &str,
        collection: DirectoryHistoryCollection,
        mut query: BTreeMap<String, String>,
    ) -> Result<Self> {
        crate::validate_id(repository, "repo")?;
        if directory != repository {
            crate::validate_id(directory, "entry")?;
        }
        if query.keys().any(|key| {
            !["limit", "cursor", "history_root", "anchor_tree_id"].contains(&key.as_str())
        }) {
            return Err(invalid("unknown directory history query field"));
        }
        let limit = match query.remove("limit") {
            None => 64,
            Some(value) => {
                let parsed = crate::parse_u64_decimal(&value)?;
                u32::try_from(parsed)
                    .map_err(|_| invalid("directory history limit exceeds its budget"))?
            }
        };
        if limit == 0 || limit > MAX_DIRECTORY_HISTORY_LIMIT {
            return Err(invalid("directory history limit must be between 1 and 128"));
        }
        let history_root = query
            .remove("history_root")
            .map(|value| HistoryRoot::parse(&value))
            .transpose()?;
        let cursor = query
            .remove("cursor")
            .map(|value| DirectoryHistoryCursor::parse(&value))
            .transpose()?;
        let anchor_tree_id = query.remove("anchor_tree_id");
        if let Some(anchor) = &anchor_tree_id {
            crate::validate_id(anchor, "dtree")?;
            if collection != DirectoryHistoryCollection::Nodes || cursor.is_some() {
                return Err(invalid(
                    "directory node anchor cannot accompany a cursor or a branch query",
                ));
            }
        }
        if let Some(cursor) = &cursor {
            let p = &cursor.payload;
            if p.repository_id != repository
                || p.directory_id != directory
                || p.collection != collection
                || p.limit != limit
                || history_root.is_some_and(|root| root != p.history_root)
            {
                return Err(invalid_cursor(
                    "directory history cursor belongs to another scope or query",
                ));
            }
        }
        Ok(Self {
            repository_id: repository.into(),
            directory_id: directory.into(),
            collection,
            limit,
            history_root,
            cursor,
            anchor_tree_id,
        })
    }
    pub fn repository_id(&self) -> &str {
        &self.repository_id
    }
    pub fn directory_id(&self) -> &str {
        &self.directory_id
    }
    pub fn collection(&self) -> DirectoryHistoryCollection {
        self.collection
    }
    pub fn limit(&self) -> u32 {
        self.limit
    }
    pub fn expected_root(&self) -> Option<HistoryRoot> {
        self.history_root.or_else(|| {
            self.cursor
                .as_ref()
                .map(|cursor| cursor.payload.history_root)
        })
    }
    pub fn cursor(&self) -> Option<&DirectoryHistoryCursor> {
        self.cursor.as_ref()
    }
    pub fn anchor_tree_id(&self) -> Option<&str> {
        self.anchor_tree_id.as_deref()
    }
    pub fn route(&self) -> String {
        format!(
            "/api/repositories/{}/directories/{}/history{}",
            self.repository_id,
            self.directory_id,
            if self.collection == DirectoryHistoryCollection::Branches {
                "/branches"
            } else {
                ""
            }
        )
    }
    pub fn query_fields(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::from([("limit".into(), self.limit.to_string())]);
        if let Some(root) = self.history_root {
            fields.insert("history_root".into(), root.to_string());
        }
        if let Some(cursor) = &self.cursor {
            fields.insert("cursor".into(), cursor.encoded.clone());
        }
        if let Some(anchor) = &self.anchor_tree_id {
            fields.insert("anchor_tree_id".into(), anchor.clone());
        }
        fields
    }
    pub fn page_cursor(
        &self,
        root: HistoryRoot,
        sequence: u64,
        id: &str,
        direction: DirectoryHistoryDirection,
    ) -> Result<String> {
        let payload = CursorPayload {
            repository_id: self.repository_id.clone(),
            directory_id: self.directory_id.clone(),
            collection: self.collection,
            history_root: root,
            limit: self.limit,
            direction,
            sequence,
            id: id.into(),
        };
        let bytes = json_codec::to_vec(&payload)?;
        if bytes.len() > MAX_CURSOR_BYTES / 2 {
            return Err(invalid_cursor(
                "directory history cursor exceeds its budget",
            ));
        }
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write;
            write!(&mut encoded, "{byte:02x}").expect("invariant: writing to String succeeds");
        }
        Ok(encoded)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryHistoryIdentity {
    pub repository_id: String,
    pub directory_id: String,
    pub history_tree_id: String,
    pub history_root: HistoryRoot,
    pub collection: DirectoryHistoryCollection,
    pub limit: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryHistoryCurrent {
    pub directory_branch_id: Option<String>,
    pub tree_id: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryHistoryNode {
    pub tree_id: String,
    pub directory_branch_id: String,
    pub parent_tree_id: Option<String>,
    pub parent_endpoint: Option<HistoryEndpoint>,
    pub commit_id: String,
    pub path: String,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryHistoryBranch {
    pub directory_branch_id: String,
    pub name: String,
    pub parent_branch_id: Option<String>,
    pub head_tree_id: String,
    pub materialized: bool,
    pub current: bool,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryHistoryPagination {
    pub previous_cursor: Option<String>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryHistoryPage {
    pub identity: DirectoryHistoryIdentity,
    pub current: DirectoryHistoryCurrent,
    pub nodes: Vec<DirectoryHistoryNode>,
    pub branches: Vec<DirectoryHistoryBranch>,
    pub page: DirectoryHistoryPagination,
}
impl DirectoryHistoryPage {
    /// Shared response admission for HTTP, native clients and browser consumers.
    pub fn validate(&self, request: &DirectoryHistoryRequest) -> Result<()> {
        let i = &self.identity;
        if i.repository_id != request.repository_id
            || i.directory_id != request.directory_id
            || i.collection != request.collection
            || i.limit != request.limit
            || request
                .expected_root()
                .is_some_and(|root| root != i.history_root)
        {
            return Err(invalid("directory history response scope or root differs"));
        }
        crate::validate_id(&i.history_tree_id, "history")?;
        if self.nodes.len() > i.limit as usize
            || self.branches.len() > i.limit as usize
            || (i.collection == DirectoryHistoryCollection::Nodes && !self.branches.is_empty())
            || (i.collection == DirectoryHistoryCollection::Branches && !self.nodes.is_empty())
        {
            return Err(invalid(
                "directory history response exceeds its collection budget",
            ));
        }
        match (&self.current.directory_branch_id, &self.current.tree_id) {
            (Some(branch), Some(tree)) => {
                crate::validate_id(branch, "dbr")?;
                crate::validate_id(tree, "dtree")?;
            }
            (None, None) => {}
            _ => return Err(invalid("directory history current selection is incomplete")),
        }
        let mut ids = BTreeSet::new();
        let mut previous = None;
        for node in &self.nodes {
            crate::validate_id(&node.tree_id, "dtree")?;
            crate::validate_id(&node.directory_branch_id, "dbr")?;
            crate::validate_id(&node.commit_id, "commit")?;
            if !node.path.is_empty() {
                name_policy::validate_path(&node.path)?;
            }
            let key = (std::cmp::Reverse(node.sequence), node.tree_id.as_str());
            if !ids.insert(node.tree_id.as_str()) || previous.is_some_and(|old| old >= key) {
                return Err(invalid(
                    "directory history nodes repeat or are out of order",
                ));
            }
            previous = Some(key);
        }
        for node in &self.nodes {
            match (&node.parent_tree_id, node.parent_endpoint) {
                (None, None) => {}
                (Some(parent), Some(endpoint)) => {
                    crate::validate_id(parent, "dtree")?;
                    if parent == &node.tree_id
                        || (endpoint == HistoryEndpoint::InWindow) != ids.contains(parent.as_str())
                    {
                        return Err(invalid("directory history parent boundary differs"));
                    }
                }
                _ => return Err(invalid("directory history parent endpoint is incomplete")),
            }
        }
        ids.clear();
        previous = None;
        for branch in &self.branches {
            crate::validate_id(&branch.directory_branch_id, "dbr")?;
            crate::validate_id(&branch.head_tree_id, "dtree")?;
            crate::validate_branch_name(&branch.name)?;
            if let Some(parent) = &branch.parent_branch_id {
                crate::validate_id(parent, "dbr")?;
            }
            let key = (
                std::cmp::Reverse(branch.sequence),
                branch.directory_branch_id.as_str(),
            );
            if !ids.insert(branch.directory_branch_id.as_str())
                || previous.is_some_and(|old| old >= key)
                || branch.current
                    != (self.current.directory_branch_id.as_deref()
                        == Some(branch.directory_branch_id.as_str()))
            {
                return Err(invalid("directory history branch summary differs"));
            }
            previous = Some(key);
        }
        let rows = match i.collection {
            DirectoryHistoryCollection::Nodes => self
                .nodes
                .iter()
                .map(|n| (n.sequence, n.tree_id.as_str()))
                .collect::<Vec<_>>(),
            DirectoryHistoryCollection::Branches => self
                .branches
                .iter()
                .map(|b| (b.sequence, b.directory_branch_id.as_str()))
                .collect::<Vec<_>>(),
        };
        if let Some(cursor) = request.cursor() {
            let boundary = (std::cmp::Reverse(cursor.sequence()), cursor.id());
            for &(sequence, id) in &rows {
                let key = (std::cmp::Reverse(sequence), id);
                let progressing = match cursor.direction() {
                    DirectoryHistoryDirection::Before => key < boundary,
                    DirectoryHistoryDirection::After => key > boundary,
                };
                if !progressing {
                    return Err(invalid_cursor(
                        "directory history page does not progress past its cursor",
                    ));
                }
            }
        }
        if let Some(anchor) = request.anchor_tree_id()
            && rows.first().map(|(_, id)| *id) != Some(anchor)
        {
            return Err(invalid(
                "directory history page does not start at its anchor",
            ));
        }
        for (value, direction) in [
            (
                &self.page.previous_cursor,
                DirectoryHistoryDirection::Before,
            ),
            (&self.page.next_cursor, DirectoryHistoryDirection::After),
        ] {
            if let Some(value) = value {
                let cursor = DirectoryHistoryCursor::parse(value)?;
                let p = &cursor.payload;
                if p.repository_id != i.repository_id
                    || p.directory_id != i.directory_id
                    || p.collection != i.collection
                    || p.history_root != i.history_root
                    || p.limit != i.limit
                    || p.direction != direction
                {
                    return Err(invalid_cursor("directory history response cursor differs"));
                }
                let endpoint = match i.collection {
                    DirectoryHistoryCollection::Nodes => {
                        let n = if direction == DirectoryHistoryDirection::Before {
                            self.nodes.first()
                        } else {
                            self.nodes.last()
                        };
                        n.map(|n| (n.sequence, n.tree_id.as_str()))
                    }
                    DirectoryHistoryCollection::Branches => {
                        let b = if direction == DirectoryHistoryDirection::Before {
                            self.branches.first()
                        } else {
                            self.branches.last()
                        };
                        b.map(|b| (b.sequence, b.directory_branch_id.as_str()))
                    }
                };
                if endpoint != Some((p.sequence, p.id.as_str())) {
                    return Err(invalid_cursor(
                        "directory history cursor is not a page boundary",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/directory_history.rs"]
mod tests;

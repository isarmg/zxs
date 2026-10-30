//! The action owns only selected branch facts, never a candidate page.
use data_serialization::Serialize;
use json_codec::Value;
use std::io::{self, Write};
use crate::files::history::{FileBranch, FileNode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::files) struct SelectedBranch {
    pub(in crate::files) id: String,
    pub(in crate::files) name: String,
    pub(in crate::files) expected: String,
}
impl SelectedBranch {
    pub(in crate::files) fn capture(branch: &FileBranch) -> Option<Self> {
        if !branch.writable() {
            return None;
        }
        branch.head().map(|head| Self {
            id: branch.id().to_owned(),
            name: branch.name().to_owned(),
            expected: head.to_owned(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::files) struct ChosenNode {
    pub(in crate::files) id: String,
    pub(in crate::files) version: String,
}
impl ChosenNode {
    pub(in crate::files) fn capture(node: &FileNode) -> Option<Self> {
        node.version_id().map(|version| Self {
            id: node.id().to_owned(),
            version: version.to_owned(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReviewInventory {
    pub(crate) node_ids: Vec<String>,
    pub(crate) branch_ids: Vec<String>,
}

/// This is the existing JSON write-body limit, including the operation identity
/// inserted by `review::submit`. It does not restrict permanent history.
pub(in crate::files) const MAX_ACTION_REQUEST_BYTES: usize = 1024 * 1024;
const RESERVED_OPERATION_ID: &str = "op_00000000000000000000000000000000";

#[derive(Debug, error_definition::Error)]
pub(in crate::files) enum LimitError {
    #[error("This action can contain at most {maximum} entries.")]
    Entries { maximum: usize },
    #[error("The complete action request exceeds the 1 MiB JSON limit.")]
    Bytes {
        #[source]
        source: json_codec::Error,
    },
}

#[derive(Serialize)]
struct FinalAction<'a> {
    #[serialization(flatten)]
    body: &'a Value,
    operation_id: &'static str,
}

struct RequestBytes(usize);
impl Write for RequestBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_ACTION_REQUEST_BYTES - self.0 {
            return Err(io::Error::other("action request byte limit exceeded"));
        }
        self.0 += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Count the complete encoded body without allocating another serialized copy.
/// The frozen action path and every other fixed field are part of `body`.
pub(in crate::files) fn check_request(
    body: &Value,
    entries: usize,
    max_entries: usize,
) -> Result<(), LimitError> {
    if entries > max_entries {
        return Err(LimitError::Entries {
            maximum: max_entries,
        });
    }
    json_codec::to_writer(
        RequestBytes(0),
        &FinalAction {
            body,
            // The generated identity has exactly this canonical encoded length.
            operation_id: RESERVED_OPERATION_ID,
        },
    )
    .map_err(|source| LimitError::Bytes { source })
}

/// Reserve the fixed envelope and still-unassigned references before accepting
/// another selection. IDs have the current canonical prefix plus 32 hex digits;
/// naming admits no JSON-escaped path characters and caps paths at 768 bytes.
pub(in crate::files) fn check_selection(
    body: &Value,
    entries: usize,
    max_entries: usize,
) -> Result<(), LimitError> {
    let mut reserved = body.clone();
    reserved["path"] = "p".repeat(768).into();
    if reserved.get("chosen_node_id").is_some_and(|id| id == "") {
        reserved["chosen_node_id"] = "fnode_00000000000000000000000000000000".into();
    }
    if reserved.get("surviving_branch_id").is_some_and(|id| id == "") {
        reserved["surviving_branch_id"] = "br_00000000000000000000000000000000".into();
    }
    if let Some(targets) = reserved.get_mut("targets").and_then(Value::as_array_mut) {
        for target in targets {
            if target["branch_id"] == "" {
                target["branch_id"] = "br_00000000000000000000000000000000".into();
                target["expected_node_id"] = "fnode_00000000000000000000000000000000".into();
            }
        }
    }
    check_request(&reserved, entries, max_entries)
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/merge/selection_tests.rs"]
mod tests;

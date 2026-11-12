//! Typed permanent facts for namespace changes, independent of content ancestry.
use crate::{Error, OperationResult, PathChangeError, Result};
use data_serialization::{Deserialize, Serialize};
use json_codec::Value;
use storage_contract::file_lifecycle::PathLocation;
use storage_contract::{
    BranchId, EntryId, FileId, FilePathEventId, HistoryTreeId, OperationId, Principal,
    Sequence, Timestamp, VersionId,
};

const MAX_FACTS: usize = 65_536;
const MAX_BYTES: usize = 12 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct PathChanged {
    pub(crate) event_id: FilePathEventId,
    pub(crate) operation_id: OperationId,
    pub(crate) file_id: FileId,
    pub(crate) entry_id: EntryId,
    pub(crate) history_tree_id: HistoryTreeId,
    pub(crate) branch_id: BranchId,
    pub(crate) before_version_id: VersionId,
    pub(crate) after_version_id: VersionId,
    pub(crate) before: PathLocation,
    pub(crate) after: PathLocation,
    pub(crate) sequence: Sequence,
    pub(crate) occurred_at: Timestamp,
    pub(crate) author: Principal,
}

/// Every producer of these receipt kinds writes the required fact vector.
pub(crate) fn is_kind(kind: &str) -> bool {
    matches!(kind, "workspace.commit" | "file.namespace" | "directory.namespace.batch")
        || kind == storage_contract::file_relocation::OBJECT_MOVE_KIND
        || crate::engine::CurrentNamespaces::recursive_namespace_kind(kind)
}

pub(crate) fn invalid() -> Error {
    Error::PathChange(PathChangeError::Publication)
}

fn codec(source: json_codec::Error) -> Error {
    Error::PathChange(PathChangeError::Encoding(source))
}

/// Identity is assigned by the writer, not synthesized while paging history.
pub(crate) fn identity(
    operation: OperationId,
    file: FileId,
    entry: EntryId,
    before: &PathLocation,
    after: &PathLocation,
) -> Result<FilePathEventId> {
    let bytes = storage_contract::canonical_json(&(
        "zxs.container-path-change",
        operation,
        file,
        entry,
        before,
        after,
    ))?;
    let hash = blake3::hash(&bytes);
    let mut identity = [0; 16];
    identity.copy_from_slice(&hash.as_bytes()[..16]);
    Ok(FilePathEventId::from_random(identity))
}

pub(crate) fn validate_fact(fact: &PathChanged) -> Result<()> {
    if fact.before == fact.after
        || fact.sequence.get() == 0
        || fact.event_id != identity(
            fact.operation_id, fact.file_id, fact.entry_id, &fact.before, &fact.after,
        )?
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate(facts: &[PathChanged]) -> Result<()> {
    if facts.len() > MAX_FACTS {
        return Err(Error::PathChange(PathChangeError::Limit));
    }
    let mut previous = None;
    for fact in facts {
        validate_fact(fact)?;
        if previous.is_some_and(|id| id >= fact.event_id) {
            return Err(invalid());
        }
        previous = Some(fact.event_id);
    }
    if storage_contract::canonical_json(facts)?.len() > MAX_BYTES {
        return Err(Error::PathChange(PathChangeError::Limit));
    }
    Ok(())
}

/// Call only for a path-fact-bearing receipt kind. Missing facts are not legacy empty facts.
pub(crate) fn read(receipt: &OperationResult) -> Result<Vec<PathChanged>> {
    let value = receipt.result.get("path_changes").ok_or_else(invalid)?;
    let array = value.as_array().ok_or_else(invalid)?;
    if array.len() > MAX_FACTS || storage_contract::canonical_json_value(value)?.len() > MAX_BYTES {
        return Err(Error::PathChange(PathChangeError::Limit));
    }
    let facts: Vec<PathChanged> = json_codec::from_value(value.clone()).map_err(codec)?;
    validate(&facts)?;
    for fact in &facts {
        if fact.operation_id != receipt.operation_id || fact.sequence.get() != receipt.sequence {
            return Err(invalid());
        }
    }
    Ok(facts)
}

pub(crate) fn attach(mut result: Value, mut facts: Vec<PathChanged>) -> Result<Value> {
    facts.sort_by_key(|fact| fact.event_id);
    validate(&facts)?;
    let object = result.as_object_mut().ok_or_else(invalid)?;
    if object.contains_key("path_changes") {
        return Err(invalid());
    }
    object.insert("path_changes".into(), json_codec::to_value(facts).map_err(codec)?);
    Ok(result)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/path_changes_tests.rs"]
mod tests;

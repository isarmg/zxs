//! Typed permanent facts for namespace changes, independent of content ancestry.
use crate::{Error, OperationResult, PathChangeError, Result};
use data_serialization::{Deserialize, Serialize};
use json_codec::Value;
use storage_contract::file_lifecycle::PathLocation;
use storage_contract::{
    BranchId, EntryId, FileId, FilePathEventId, HistoryTreeId, OperationId, Principal,
    Sequence, Timestamp, VersionId,
};

const MAX_FACTS: usize = storage_contract::file_lifecycle::MAX_AUDIT_EVENTS;
const MAX_BYTES: usize = storage_contract::file_lifecycle::MAX_AUDIT_BYTES;

mod ledger;
mod references;
pub(crate) use ledger::{admit as admit_ledger, visit as visit_ledger};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct BatchTransfer {
    pub(crate) source: storage_contract::DirectoryNamespaceSource,
    pub(crate) target: Option<storage_contract::DirectoryNamespaceTarget>,
    pub(crate) file_id: Option<FileId>,
    pub(crate) version_id: Option<VersionId>,
    #[serialization(rename = "physical_path")]
    _physical_path: Option<name_policy::PortablePath>,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct BatchRequest {
    repository_id: storage_contract::RepositoryId,
    request: storage_contract::DirectoryNamespaceBatch,
}

pub(crate) struct BatchTransfers {
    pub(crate) repository: storage_contract::RepositoryId,
    pub(crate) kind: storage_contract::DirectoryBatchKind,
    pub(crate) items: Vec<BatchTransfer>,
}

pub(crate) fn batch_transfers(
    receipt: &OperationResult,
    principal: &Principal,
) -> Result<Option<BatchTransfers>> {
    if !matches!(
        receipt.kind.as_str(),
        "directory.namespace.batch"
            | "directory.namespace.batch.recursive"
            | crate::protection::BATCH_FOLLOW_KIND
            | crate::protection::BATCH_ROUTE_KIND
    ) {
        return Ok(None);
    }
    let raw = receipt.result.get("transfers").ok_or_else(invalid)?;
    let rows = raw.as_array().ok_or_else(invalid)?;
    if rows.len() > MAX_FACTS || storage_contract::canonical_json_value(raw)?.len() > MAX_BYTES {
        return Err(Error::PathChange(PathChangeError::Limit));
    }
    for row in rows {
        if ["target", "physical_path", "file_id", "version_id"]
            .iter().any(|field| row.get(*field).is_none())
        {
            return Err(invalid());
        }
    }
    let input = receipt.result.get("namespace_request").ok_or_else(invalid)?;
    if crate::Engine::native_request_digest(&receipt.kind, principal.as_str(), input)?
        != receipt.request_digest
    {
        return Err(invalid());
    }
    let request: BatchRequest = json_codec::from_value(input.clone()).map_err(codec)?;
    let action: storage_contract::DirectoryBatchKind = json_codec::from_value(
        receipt.result.get("action").ok_or_else(invalid)?.clone(),
    ).map_err(codec)?;
    let count: Sequence = json_codec::from_value(
        receipt.result.get("count").ok_or_else(invalid)?.clone(),
    )
    .map_err(codec)?;
    let transfers: Vec<BatchTransfer> = json_codec::from_value(raw.clone()).map_err(codec)?;
    if transfers.is_empty()
        || transfers.len() != request.request.items.len()
        || count.get() != transfers.len() as u64
        || action != request.request.kind
        || action == storage_contract::DirectoryBatchKind::Delete
    {
        return Err(invalid());
    }
    for (item, transfer) in request.request.items.iter().zip(&transfers) {
        if storage_contract::canonical_json(&item.source)?
            != storage_contract::canonical_json(&transfer.source)?
            || storage_contract::canonical_json(&item.target)?
                != storage_contract::canonical_json(&transfer.target)?
        {
            return Err(invalid());
        }
    }
    Ok(Some(BatchTransfers {
        repository: request.repository_id,
        kind: request.request.kind,
        items: transfers,
    }))
}

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
    validate_receipt_facts(receipt, &facts)?;
    Ok(facts)
}

pub(super) fn validate_receipt_facts(
    receipt: &OperationResult,
    facts: &[PathChanged],
) -> Result<()> {
    validate(facts)?;
    for fact in facts {
        if fact.operation_id != receipt.operation_id || fact.sequence.get() != receipt.sequence {
            return Err(invalid());
        }
    }
    Ok(())
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

/// Shared immutable graph ownership closure for imported typed path projections.
/// This does not admit a source receipt as a local native operation.
pub(crate) fn retained_references(
    state: &crate::State,
    rows: &[crate::business::history_copy::retained::RetainedPathChange],
    published: Sequence,
) -> crate::Result<()> {
    let mut references = references::References::new(state);
    for row in rows {
        references.check(
            row.file_id, row.history_tree_id, row.branch_id,
            [row.before_version_id, row.after_version_id], published, &mut || Ok(()),
        )?;
    }
    Ok(())
}

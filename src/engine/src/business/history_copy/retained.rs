//! Complete audit prefix captured in this target's own receipt.
//! No source operation is forged, and imported D is never a live deletion.
use crate::business::history_copy::{
    CopyRoute, Destination, DIRECTORY_EXPORT_KIND, FileHistoryCopyRequest, KIND, RETAINED_KIND,
    identity, invalid, sources_for,
};
use crate::{Error, OperationId, OperationResult, Result, State};
use data_serialization::{Deserialize, Serialize};
use json_codec::{Value, json};
use storage_contract::{BranchId, FileId, HistoryTreeId, VersionId};
use std::collections::{BTreeMap, BTreeSet};
use crate::file_deletion::retained::LifecycleRow;

const FIELD: &str = "retained_lifecycle";
const MAX_BYTES: usize = storage_contract::file_lifecycle::MAX_AUDIT_BYTES;

pub(in crate::business) fn bounded<T: Serialize + ?Sized>(value: &T) -> Result<()> {
    bounded_bytes(value).map(|_| ())
}

fn bounded_bytes<T: Serialize + ?Sized>(value: &T) -> Result<usize> {
    // Count compact serialized bytes without allocating an output/Value copy.
    // Object key ordering does not change this encoded-length limit.
    struct Limit {
        remaining: usize,
        exceeded: bool,
    }
    impl std::io::Write for Limit {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.remaining {
                self.exceeded = true;
                return Err(std::io::Error::other(
                    "retained audit exceeds its byte limit",
                ));
            }
            self.remaining -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut limit = Limit {
        remaining: MAX_BYTES,
        exceeded: false,
    };
    match json_codec::to_writer(&mut limit, value) {
        Ok(()) => Ok(MAX_BYTES - limit.remaining),
        Err(_) if limit.exceeded => {
            Err(Error::PathChange(crate::PathChangeError::Limit))
        }
        Err(error) => Err(error.into()),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub(crate) struct CopiedLifecycle {
    pub source_file_id: String,
    pub source_history_tree_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub source_sequence: u64,
    pub source_records_hash: String,
    pub file_id: String,
    pub history_tree_id: String,
    pub events: Vec<LifecycleRow>,
    pub path_changes: Vec<RetainedPathChange>,
}

/// Original source evidence is immutable; these fields describe only the
/// receiving container's explicit identity projection of that evidence.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
pub(crate) struct RetainedPathChange {
    pub(crate) source: crate::persist::path_changes::PathChanged,
    pub(crate) file_id: FileId,
    pub(crate) history_tree_id: HistoryTreeId,
    pub(crate) branch_id: BranchId,
    pub(crate) before_version_id: VersionId,
    pub(crate) after_version_id: VersionId,
}

pub(crate) fn read(receipt: &OperationResult, file: &str) -> Result<CopiedLifecycle> {
    let raw = receipt.result.get(FIELD).ok_or_else(invalid)?;
    require_event_budget(raw)?;
    bounded(raw)?;
    let value: CopiedLifecycle = json_codec::from_value(raw.clone())?;
    validate(receipt, file, &value)?;
    Ok(value)
}

/// Fence the vector before either reader clones or decodes its envelope.
pub(in crate::business) fn require_event_budget(raw: &Value) -> Result<()> {
    let mut remaining = storage_contract::file_lifecycle::MAX_AUDIT_EVENTS;
    for field in ["events", "path_changes"] {
        let count = raw.get(field).and_then(Value::as_array).ok_or_else(invalid)?.len();
        if count > remaining {
            return Err(Error::PathChange(crate::PathChangeError::Limit));
        }
        remaining -= count;
    }
    Ok(())
}

/// The native move reader already decoded the same bounded result envelope.
/// Reuse this ownership rule without decoding or cloning its audit again.
pub(in crate::business) fn validate(
    receipt: &OperationResult,
    file: &str,
    value: &CopiedLifecycle,
) -> Result<()> {
    if !matches!(
        receipt.kind.as_str(),
        KIND | RETAINED_KIND | DIRECTORY_EXPORT_KIND | crate::business::FILE_HISTORY_MOVE_KIND
    ) || receipt.result["file_id"].as_str() != Some(file)
        || receipt.result["source_file_id"].as_str() != Some(value.source_file_id.as_str())
    {
        return Err(invalid());
    }
    validate_payload(receipt, file, value)
}

/// Shared retained-prefix identity rules; the containing operation validates
/// its own typed envelope rather than forging a single-file receipt.
pub(in crate::business) fn validate_payload(
    receipt: &OperationResult,
    file: &str,
    value: &CopiedLifecycle,
) -> Result<()> {
    if value.events.len() > storage_contract::file_lifecycle::MAX_AUDIT_EVENTS
        || value.path_changes.len()
            > storage_contract::file_lifecycle::MAX_AUDIT_EVENTS - value.events.len()
        || value.file_id != file
        || value.file_id != identity(receipt.operation_id, &value.source_file_id)?
        || receipt.history_tree_id.as_deref() != Some(value.history_tree_id.as_str())
        || value.source_sequence >= receipt.sequence
        || value.source_records_hash.len() != 64
        || !value
            .source_records_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || value.events.iter().any(|event| {
            event.sequence == 0
                || event.sequence > value.source_sequence
                || event.node.file_id != file
                || event.node.history_tree_id != value.history_tree_id
        })
    {
        return Err(invalid());
    }
    let mut seen = BTreeSet::new();
    let mut previous = None;
    for event in &value.path_changes {
        crate::persist::path_changes::validate_fact(&event.source)?;
        let key = (event.source.sequence, event.source.operation_id, event.source.event_id);
        if event.file_id.to_string() != file
            || event.history_tree_id.to_string() != value.history_tree_id
            || event.source.sequence.get() > value.source_sequence
            || !seen.insert(event.source.event_id)
            || previous.is_some_and(|previous| previous >= key)
        {
            return Err(crate::persist::path_changes::invalid());
        }
        previous = Some(key);
    }
    storage_contract::validate_id(&value.source_file_id, "file")?;
    storage_contract::validate_id(&value.source_history_tree_id, "history")?;
    storage_contract::validate_id(&value.history_tree_id, "history")?;
    Ok(())
}

pub(super) fn apply(
    previous: &State,
    next: &mut State,
    view: Option<&crate::engine::CurrentNamespaces>,
    request: &FileHistoryCopyRequest,
    route: &CopyRoute,
    principal: &str,
    operation: OperationId,
) -> Result<Value> {
    let RetainedProjection {
        mut result,
        lifecycle,
    } = project(
        previous,
        next,
        view,
        Destination::EmptyRepository(request),
        route,
        principal,
        operation,
    )?;
    result[FIELD] = json!(lifecycle);
    Ok(result)
}

/// The typed audit is shared by copy and the non-publishing move projection.
/// Neither field is a receipt or publication authority until its own operation
/// has passed the complete publication protocol.
pub(in crate::business) struct RetainedProjection {
    pub(in crate::business) result: Value,
    pub(in crate::business) lifecycle: CopiedLifecycle,
}

pub(in crate::business) fn project(
    previous: &State,
    next: &mut State,
    view: Option<&crate::engine::CurrentNamespaces>,
    destination: Destination<'_>,
    route: &CopyRoute,
    principal: &str,
    operation: OperationId,
) -> Result<RetainedProjection> {
    let source = sources_for(previous, view, destination)?;
    let request = destination.request();
    let source_history = &previous.repositories[&request.source_repository_id].history_tree_id;
    let result = super::apply_scoped(
        previous,
        next,
        view,
        destination,
        route,
        principal,
        operation,
    )?;
    let projection = crate::state_projection::ProjectionSnapshot::new(previous);
    let lifecycle = capture_mapped(
        projection.view(),
        &source.file,
        source_history,
        &previous.repositories[&request.target_repository_id].history_tree_id,
        operation,
    )?;
    Ok(RetainedProjection { result, lifecycle })
}

pub(in crate::business) fn capture_mapped(
    source: crate::state_projection::StateProjection<'_>,
    source_file: &str,
    source_history: &str,
    history: &str,
    operation: OperationId,
) -> Result<CopiedLifecycle> {
    let capture = crate::container_storage::file_database::capture_projected_copy(
        source,
        source_file,
        source_history,
    )?;
    let file = identity(operation, source_file)?;
    let source_file_id = storage_contract::history_window::HistoryFileId::parse(source_file)?;
    let mut path_changes = path_changes(source, &source_file_id)?;
    for event in &mut path_changes {
        event.file_id = file.parse()?;
        event.history_tree_id = history.parse()?;
        event.branch_id = identity(operation, &event.branch_id.to_string())?.parse()?;
        event.before_version_id = identity(operation, &event.before_version_id.to_string())?.parse()?;
        event.after_version_id = identity(operation, &event.after_version_id.to_string())?.parse()?;
    }
    let mut events = capture.events;
    for event in &mut events {
        let node = &mut event.node;
        node.node_id = identity(operation, &node.node_id)?;
        node.file_id = file.clone();
        node.history_tree_id = history.to_owned();
        node.branch_heads = node
            .branch_heads
            .iter()
            .map(|(branch, head)| Ok((identity(operation, branch)?, identity(operation, head)?)))
            .collect::<Result<_>>()?;
        node.parent_node_ids = node
            .parent_node_ids
            .iter()
            .map(|id| identity(operation, id))
            .collect::<Result<_>>()?;
        node.parent_node_ids.sort();
    }
    let copied = CopiedLifecycle {
        source_file_id: source_file.to_owned(),
        source_history_tree_id: source_history.to_owned(),
        source_sequence: source.state().sequence,
        source_records_hash: capture.records_hash,
        file_id: file,
        history_tree_id: history.to_owned(),
        events,
        path_changes,
    };
    if copied.events.len() > storage_contract::file_lifecycle::MAX_AUDIT_EVENTS
        || copied.path_changes.len()
            > storage_contract::file_lifecycle::MAX_AUDIT_EVENTS - copied.events.len()
    {
        return Err(Error::PathChange(crate::PathChangeError::Limit));
    }
    bounded(&copied)?;
    Ok(copied)
}

/// A complete source prefix from the same immutable projection used by FILE.
/// It never follows mutable source containers or recursively opens older copies.
pub(crate) fn path_changes(
    source: crate::state_projection::StateProjection<'_>,
    file: &storage_contract::history_window::HistoryFileId,
) -> Result<Vec<RetainedPathChange>> {
    let owners = source.owners();
    if owners.malformed_path_operations().next().is_some()
        || owners.malformed_history_copy_receipts()
    {
        return Err(crate::persist::path_changes::invalid());
    }
    let Some(rows) = owners.file_rows(file.as_str()) else {
        return Ok(Vec::new());
    };
    if rows.history_copies.len() > 1 {
        return Err(crate::persist::path_changes::invalid());
    }
    let state = source.state();
    let mut facts = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut bytes = 2usize;
    let mut insert = |fact: RetainedPathChange| -> Result<()> {
        crate::persist::path_changes::validate_fact(&fact.source)?;
        if fact.file_id.to_string() != file.as_str() || !seen.insert(fact.source.event_id) {
            return Err(crate::persist::path_changes::invalid());
        }
        if facts.len() == storage_contract::file_lifecycle::MAX_AUDIT_EVENTS {
            return Err(Error::PathChange(crate::PathChangeError::Limit));
        }
        let encoded = bounded_bytes(&fact)?;
        bytes = bytes.checked_add(encoded)
            .and_then(|count| count.checked_add(usize::from(!facts.is_empty())))
            .filter(|count| *count <= MAX_BYTES)
            .ok_or(Error::PathChange(crate::PathChangeError::Limit))?;
        let key = (fact.source.sequence, fact.source.operation_id, fact.source.event_id);
        if facts.insert(key, fact).is_some() {
            return Err(crate::persist::path_changes::invalid());
        }
        Ok(())
    };
    for id in rows.history_copies.keys() {
        let receipt = state.operations.get(id).ok_or_else(crate::persist::path_changes::invalid)?;
        if receipt.operation_id != *id {
            return Err(crate::persist::path_changes::invalid());
        }
        if receipt.kind == storage_contract::file_relocation::OBJECT_MOVE_KIND {
            let movement = source.lifecycle(file).object_move(*id)?;
            for event in &movement.retained(file.as_str())?.path_changes {
                insert(event.clone())?;
            }
        } else {
            let copied = read(receipt, file.as_str())?;
            for event in copied.path_changes {
                insert(event)?;
            }
        }
    }
    for id in rows.path_operations.keys() {
        let receipt = state.operations.get(id).ok_or_else(crate::persist::path_changes::invalid)?;
        let principal = state.operation_principals.get(id)
            .ok_or_else(crate::persist::path_changes::invalid)?;
        if receipt.operation_id != *id {
            return Err(crate::persist::path_changes::invalid());
        }
        for fact in crate::persist::path_changes::read(receipt)? {
            if fact.author.as_str() != principal.as_str() {
                return Err(crate::persist::path_changes::invalid());
            }
            if fact.file_id.to_string() == file.as_str() {
                insert(RetainedPathChange {
                    file_id: fact.file_id,
                    history_tree_id: fact.history_tree_id,
                    branch_id: fact.branch_id,
                    before_version_id: fact.before_version_id,
                    after_version_id: fact.after_version_id,
                    source: fact,
                })?;
            }
        }
    }
    let facts = facts.into_values().collect::<Vec<_>>();
    bounded(&facts)?;
    Ok(facts)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/retained_history_copy_tests.rs"]
mod tests;

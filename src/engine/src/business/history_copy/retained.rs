//! Complete path-free audit prefix captured in this target's own receipt.
//! No source operation is forged, and imported D is never a live deletion.
use super::*;
use crate::file_deletion::retained::LifecycleRow;

const FIELD: &str = "retained_lifecycle";
const MAX_BYTES: usize = 12 * 1024 * 1024;

pub(in crate::business) fn bounded<T: Serialize + ?Sized>(value: &T) -> Result<()> {
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
        Ok(()) => Ok(()),
        Err(_) if limit.exceeded => Err(Error::new(
            ErrorCode::ResourceLimit,
            "retained audit exceeds its encoded byte budget",
        )),
        Err(error) => Err(error.into()),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
}

pub(crate) fn read(receipt: &OperationResult, file: &str) -> Result<CopiedLifecycle> {
    let raw = receipt.result.get(FIELD).ok_or_else(invalid)?;
    // State is already bounded by publication admission; retain a stricter
    // per-file limit before decoding/copying the retained vector.
    if raw
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?
        .len()
        > 65_536
    {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "retained audit exceeds its event budget",
        ));
    }
    bounded(raw)?;
    let value: CopiedLifecycle = json_codec::from_value(raw.clone())?;
    if !matches!(receipt.kind.as_str(), RETAINED_KIND | crate::business::FILE_HISTORY_MOVE_KIND)
        || value.events.len() > 65_536
        || value.file_id != file
        || receipt.result["file_id"].as_str() != Some(file)
        || receipt.result["source_file_id"].as_str() != Some(value.source_file_id.as_str())
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
    storage_contract::validate_id(&value.source_file_id, "file")?;
    storage_contract::validate_id(&value.source_history_tree_id, "history")?;
    storage_contract::validate_id(&value.history_tree_id, "history")?;
    Ok(value)
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
    } = project(previous, next, view, request, route, principal, operation)?;
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
    request: &FileHistoryCopyRequest,
    route: &CopyRoute,
    principal: &str,
    operation: OperationId,
) -> Result<RetainedProjection> {
    let source = sources(previous, view, request)?;
    let source_history = &previous.repositories[&request.source_repository_id].history_tree_id;
    let capture = crate::container_storage::file_database::capture_copy(
        previous,
        &source.file,
        source_history,
    )?;
    let result = super::apply(previous, next, view, request, route, principal, operation)?;
    let file = identity(operation, &source.file)?;
    let history = &previous.repositories[&request.target_repository_id].history_tree_id;
    let mut events = capture.events;
    for event in &mut events {
        let node = &mut event.node;
        node.node_id = identity(operation, &node.node_id)?;
        node.file_id = file.clone();
        node.history_tree_id = history.clone();
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
        source_file_id: source.file,
        source_history_tree_id: source_history.clone(),
        source_sequence: previous.sequence,
        source_records_hash: capture.records_hash,
        file_id: file,
        history_tree_id: history.clone(),
        events,
    };
    bounded(&copied)?;
    Ok(RetainedProjection {
        result,
        lifecycle: copied,
    })
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/retained_history_copy_tests.rs"]
mod tests;

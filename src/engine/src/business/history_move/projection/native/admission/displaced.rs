//! The displaced target belongs to this one move receipt, never a second delete.
use super::*;

impl ReadBody {
    pub(super) fn require_replacement(
        &self,
        state: &State,
        receipt: &OperationResult,
        principal: &str,
    ) -> Result<()> {
        let (identity, proof) = match (&self.move_request.replacement, &self.replacement) {
            (None, None) => return Ok(()),
            (Some(identity), Some(proof)) => (identity, proof),
            _ => return Err(invalid()),
        };
        let request = &self.move_request.namespace;
        let node = &proof.deletion_node;
        let occurrence = &proof.occurrence;
        let target = state.repositories.get(&request.target_repository_id).ok_or_else(invalid)?;
        let context = &occurrence.displaced_context;
        let context_branch = state.branches.get(&context.branch_id).ok_or_else(invalid)?;
        crate::file_deletion::publication::admit_node_shape(node)?;
        digest(&proof.source_records_hash)?;
        let hash = object_hash(
            "file-history-move-replaced-deletion",
            &json!([receipt.operation_id, node.file_id]),
        )?;
        if !identity.matches(&occurrence.displaced)
            || occurrence.namespace_head != request.expected_target_head
            || occurrence.displaced.file_id != node.file_id
            || occurrence.displaced.path != node.path
            || occurrence.displaced.lookup_key()
                != name_policy::LookupPath::parse(&request.target_name)?.key()
            || node.file_id == self.source_file_id
            || node.file_id == self.file_id
            || node.node_id != format!("fdel_{}", &hash[..32])
            || node.repository_id != request.target_repository_id
            || node.history_tree_id != target.history_tree_id
            || node.operation_id != receipt.operation_id
            || node.author != principal
            || node.sequence != receipt.sequence
            || node.created_at != self.timestamp
            || context.inherited_from.is_some()
            || context_branch.file_id.as_deref() != Some(node.file_id.as_str())
            || context_branch.history_tree_id != node.history_tree_id
            || node.branch_heads.get(&context.branch_id)
                != Some(context.node_id.as_ref().unwrap_or(&occurrence.displaced.version_id))
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// One receipt may own the source D and one exact displaced target D.
pub(crate) fn admit_deletions(
    state: &State,
    receipt: &OperationResult,
) -> Result<Vec<FileDeletionNode>> {
    let source = admit_source_deletion(state, receipt)?;
    let mut nodes = vec![source];
    // The complete typed envelope was admitted above.
    if let Some(proof) = receipt.result.get("replacement") {
        nodes.push(json_codec::from_value(
            proof.get("deletion_node").ok_or_else(invalid)?.clone(),
        )?);
    }
    Ok(nodes)
}

/// FILE audit projection is also used for the private pre-publication draft.
/// Publication flags and complete State authority are checked by its consumer.
pub(crate) fn retained_deletion(
    receipt: &OperationResult,
    file: &str,
) -> Result<FileDeletionNode> {
    history_copy::retained::bounded(&receipt.result)?;
    let source = receipt.result.get("deletion_node").ok_or_else(invalid)?;
    let node = if source.get("file_id").and_then(json_codec::Value::as_str) == Some(file) {
        source
    } else {
        receipt.result.get("replacement")
            .and_then(|proof| proof.get("deletion_node"))
            .filter(|node| node.get("file_id").and_then(json_codec::Value::as_str) == Some(file))
            .ok_or_else(invalid)?
    };
    let node: FileDeletionNode = json_codec::from_value(node.clone())?;
    crate::file_deletion::publication::admit_node_shape(&node)?;
    Ok(node)
}

/// Recovery obtains displaced identity only from the original full move proof.
pub(crate) fn admit_displaced(
    state: &State,
    receipt: &OperationResult,
    node: &FileDeletionNode,
) -> Result<Option<crate::file_deletion::occurrence::DisplacedOccurrence>> {
    let body = admit_body(state, receipt)?;
    if body.deletion_node == *node {
        return Ok(None);
    }
    match body.replacement {
        Some(proof) if proof.deletion_node == *node => Ok(Some(proof.occurrence)),
        _ => Err(invalid()),
    }
}

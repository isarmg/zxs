//! Read one object's stored membership facts through the existing lifecycle API.
//! Publication authority remains the shared executor and exact State transition.
use super::*;
use storage_contract::file_relocation::OBJECT_MOVE_KIND;

fn invalid() -> Error {
    Error::new(ErrorCode::PublicationInvalid,"object movement lifecycle differs from its original members")
}

pub(crate) fn admit_result(state:&State,receipt:&OperationResult)->Result<ResultBody> {
    let result=read_result(receipt)?;
    if state.operations.get(&receipt.operation_id)!=Some(receipt)
        || state.operation_principals.get(&receipt.operation_id)!=Some(&result.movement.principal)
        || receipt.commit_state!=storage_contract::CommitState::Published
        || state.events.iter().filter(|event|event.operation_id==receipt.operation_id).count()!=1
        || !state.events.iter().any(|event|event.operation_id==receipt.operation_id
            && event.sequence==receipt.sequence && event.kind==OBJECT_MOVE_KIND
            && Some(event.object_id.as_str())==receipt.commit_id.as_deref()) {
        return Err(invalid());
    }
    for node in result.source_deletions.iter().chain(&result.displaced_deletions).chain(&result.imported_deletions) {
        crate::file_deletion::publication::admit_node_shape(node)?;
        if state.repositories.get(&node.repository_id).map(|repository|&repository.history_tree_id)!=Some(&node.history_tree_id) {
            return Err(invalid());
        }
        for (branch,head) in &node.branch_heads {
            if !state.branches.get(branch).is_some_and(|branch|branch.file_id.as_deref()==Some(node.file_id.as_str())
                && branch.history_tree_id==node.history_tree_id) {
                return Err(invalid());
            }
            let owner=state.versions.get(head).map(|row|(&row.file_id,&row.history_tree_id))
                .or_else(||state.file_path_nodes.get(head).map(|row|(&row.file_id,&row.history_tree_id)))
                .or_else(||state.file_metadata_nodes.get(head).map(|row|(&row.file_id,&row.history_tree_id)));
            if owner!=Some((&node.file_id,&node.history_tree_id)) {return Err(invalid());}
        }
    }
    Ok(result)
}

/// Removed source and replacement occurrences need exact restoration evidence.
/// Imported lifecycle nodes continue through the existing ordinary undo gate.
pub(crate) fn admit_ordinary_undo(state:&State,receipt:&OperationResult,node:&FileDeletionNode)->Result<()> {
    let result=admit_result(state,receipt)?;
    if result.source_deletions.iter().chain(&result.displaced_deletions).any(|original|original==node) {
        return Err(Error::new(ErrorCode::UnsupportedOperation,
            "object movement has no exact restoration evidence for this removed occurrence"));
    }
    if !result.imported_deletions.iter().any(|original|original==node) {
        return Err(invalid());
    }
    Ok(())
}

pub(crate) fn deletion_for(receipt:&OperationResult,file:&str)->Result<FileDeletionNode> {
    let result=read_result(receipt)?;
    result.source_deletions.into_iter().chain(result.displaced_deletions).chain(result.imported_deletions)
        .find(|node|node.file_id==file).ok_or_else(invalid)
}

pub(crate) fn retained_for(receipt:&OperationResult,file:&str)->Result<history_copy::retained::CopiedLifecycle> {
    read_result(receipt)?.retained_lifecycle.into_iter().find(|row|row.file_id==file).ok_or_else(invalid)
}

/// The deletion gate and Current gate replay this same planner. No independent
/// move-specific lifecycle plan, fake per-file receipt or relaxed write set.
pub(crate) fn verify_publication(previous:&State,next:&State,current:Option<&crate::engine::CurrentNamespaces>)->Result<bool> {
    let mut selected=None;
    let mut inspect=|id:&OperationId|->Result<()> {
        let before=previous.operations.get(id);let after=next.operations.get(id);
        if before==after || !before.into_iter().chain(after).any(|row|row.kind==OBJECT_MOVE_KIND) {return Ok(());}
        let receipt=after.ok_or_else(invalid)?;
        if before.is_some() || receipt.operation_id!=*id || selected.replace(receipt).is_some() {return Err(invalid());}
        Ok(())
    };
    match next.operations.changes_since(&previous.operations) {
        storage_contract::MapChanges::Keys(keys)=>{for id in keys.iter(){inspect(id)?;}},
        storage_contract::MapChanges::All=>{for id in previous.operations.keys().chain(next.operations.keys()).collect::<BTreeSet<_>>(){inspect(id)?;}},
    }
    let Some(receipt)=selected else{return Ok(false)};
    verify_transition(previous,next,current,receipt)?;
    Ok(true)
}

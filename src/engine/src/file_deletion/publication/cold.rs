//! Reconstruct the current D table from retained create/undo evidence. This is
//! cold State admission, not authentication of a rewritten publication root.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn admit_current(state: &State) -> Result<()> {
    let mut receipts = BTreeMap::new();
    let mut operations = BTreeSet::new();
    for (id, receipt) in state.operations.iter() {
        if !matches!(receipt.kind.as_str(), "file.delete" | "file.delete.undo") {
            continue;
        }
        if receipt.operation_id != *id
            || receipts.insert(receipt.sequence, (*id, receipt)).is_some()
        {
            return Err(invalid());
        }
        operations.insert(*id);
    }
    let mut witnessed = BTreeSet::new();
    for event in state.events.iter() {
        if let Some((id, receipt)) = receipts.get(&event.sequence) {
            if event.operation_id != *id
                || event.kind != receipt.kind
                || event.object_id != id.to_string()
                || !witnessed.insert(*id)
            {
                return Err(invalid());
            }
        } else if operations.contains(&event.operation_id)
            || matches!(event.kind.as_str(), "file.delete" | "file.delete.undo")
        {
            return Err(invalid());
        }
    }
    if witnessed.len() != receipts.len() {
        return Err(invalid());
    }
    let mut active = BTreeMap::new();
    let mut created = BTreeSet::new();
    for (_, receipt) in receipts.values() {
        let undo = receipt.kind == "file.delete.undo";
        let node = admit_receipt(state, receipt, undo)?;
        if undo {
            if active.remove(&node.file_id).as_ref() != Some(&node) {
                return Err(invalid());
            }
        } else if !created.insert(node.node_id.clone())
            || active.insert(node.file_id.clone(), node).is_some()
        {
            return Err(invalid());
        }
    }
    if active.iter().ne(state.file_deletions.iter()) {
        return Err(invalid());
    }
    Ok(())
}

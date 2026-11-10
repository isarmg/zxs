//! One exact write set, permanent receipts, and reverse closure on cold decode.
use super::*;
use crate::{CommitState, ProtectionAtPublish};

pub(crate) fn admit_current(state: &State) -> Result<()> {
    let mut retired = BTreeSet::new();
    let mut terminals = BTreeSet::new();
    let mut archived = BTreeSet::new();
    let mut receipts = BTreeMap::new();
    for receipt in state.operations.values().filter(|r| r.kind == KIND) {
        let decision = decision(receipt)?;
        decision.request.validate()?;
        decision.cleanup_routes.validate(&decision.request)?;
        decision.separation.validate(state, &decision.request)?;
        decision
            .file_terminations
            .validate(state, &decision.request)?;
        archived.extend(decision.archived_branch_ids());
        for file in &decision.file_terminations.files {
            if !terminals.insert(file.file_id.clone()) {
                return Err(invalid());
            }
        }
        let principal = state
            .operation_principals
            .get(&receipt.operation_id)
            .ok_or_else(invalid)?;
        Engine::require_actor(principal)?;
        if receipt.sequence == 0
            || receipt.sequence > state.sequence
            || receipt.request_digest
                != Engine::native_request_digest(
                    KIND,
                    principal,
                    &json_codec::to_value(&decision.request)?,
                )?
            || receipt.commit_id.is_some()
            || receipt.history_tree_id.is_some()
            || receipt.branch_id.is_some()
            || !matches!(
                receipt.commit_state,
                CommitState::Published | CommitState::PublishedDegraded
            )
            || receipt.protection_at_publish != ProtectionAtPublish::NotApplicable
            || receipt.result["version_protection"] != json!([])
            || !receipt.result["timestamp"]
                .as_str()
                .is_some_and(|v| v.parse::<u64>().is_ok())
            || !receipt.result["membership_epoch"]
                .as_str()
                .is_some_and(|v| v.parse::<u64>().is_ok())
            || receipt.result.as_object().is_none_or(|v| v.len() != 4)
            || archive_digest(state, &decision.request)? != decision.archive_digest
            || references::capture(state, &decision.request)? != decision.inbound_references
        {
            return Err(invalid());
        }
        for id in decision.request.version_ids {
            if !is_retired(state, &id) || !retired.insert(id) {
                return Err(invalid());
            }
        }
        receipts.insert(receipt.operation_id, (receipt.sequence, 0usize));
    }
    for event in state.events.iter() {
        if let Some((sequence, count)) = receipts.get_mut(&event.operation_id) {
            if event.kind != KIND
                || event.sequence != *sequence
                || event.object_id != event.operation_id.to_string()
            {
                return Err(invalid());
            }
            *count += 1;
        } else if event.kind == KIND {
            return Err(invalid());
        }
    }
    if receipts.values().any(|(_, count)| *count != 1)
        || state
            .content_states
            .iter()
            .any(|(id, status)| *status == ContentState::Retired && !retired.contains(id))
    {
        return Err(invalid());
    }
    terminal::require_complete(state, &terminals)?;
    if !retired.is_empty() {
        require_live_references(state, &terminals, &archived)?;
    }
    Ok(())
}

pub(crate) fn verify(previous: &State, next: &State) -> Result<()> {
    // Full-state decoding and every publication retain the same reverse closure.
    // This also blocks importing an old Available/Purged state over a retirement.
    admit_current(next)?;
    for (id, receipt) in previous.operations.iter().filter(|(_, r)| r.kind == KIND) {
        if next.operations.get(id) != Some(receipt)
            || next.operation_principals.get(id) != previous.operation_principals.get(id)
        {
            return Err(invalid());
        }
    }
    for (id, state) in &previous.content_states {
        if *state == ContentState::Retired && !is_retired(next, id) {
            return Err(invalid());
        }
    }
    // Existing immutable source references remain evidence; no new object may
    // use a retired version/event to create a new live history occurrence.
    for (id, version) in next.versions.iter() {
        if previous.versions.get(id) != Some(version)
            && version
                .source_version_ids
                .iter()
                .any(|v| is_retired(next, v))
        {
            return Err(pinned());
        }
    }
    for (id, commit) in next.commits.iter() {
        if previous.commits.get(id) != Some(commit)
            && (commit
                .entries
                .iter()
                .any(|e| is_retired(next, &e.version_id))
                || commit
                    .pending_file_versions
                    .values()
                    .any(|id| is_retired(next, id)))
        {
            return Err(pinned());
        }
    }
    for (id, node) in &next.file_path_nodes {
        if previous.file_path_nodes.get(id) != Some(node)
            && (is_retired(next, &node.version_id)
                || node
                    .parent_node_ids
                    .iter()
                    .any(|parent| node_is_retired(next, parent)))
        {
            return Err(pinned());
        }
    }
    for (id, node) in &next.file_metadata_nodes {
        if previous.file_metadata_nodes.get(id) != Some(node)
            && (is_retired(next, &node.version_id)
                || node
                    .parent_node_ids
                    .iter()
                    .any(|parent| node_is_retired(next, parent)))
        {
            return Err(pinned());
        }
    }
    for (id, parents) in &next.file_node_parents {
        if previous.file_node_parents.get(id) != Some(parents)
            && parents.iter().any(|parent| node_is_retired(next, parent))
        {
            return Err(pinned());
        }
    }
    let additions = next
        .operations
        .iter()
        .filter(|(id, r)| r.kind == KIND && !previous.operations.contains_key(id))
        .map(|(_, r)| r)
        .collect::<Vec<_>>();
    if additions.is_empty() {
        if next
            .content_states
            .iter()
            .any(|(id, state)| *state == ContentState::Retired && !is_retired(previous, id))
        {
            return Err(invalid());
        }
        return Ok(());
    }
    if additions.len() != 1 {
        return Err(invalid());
    }
    let receipt = additions[0];
    let stored = decision(receipt)?;
    if decide(previous, stored.request.clone())? != stored || receipt.sequence != next.sequence {
        return Err(invalid());
    }
    let selected = stored.request.version_ids.iter().collect::<BTreeSet<_>>();
    for id in previous
        .content_states
        .keys()
        .chain(next.content_states.keys())
    {
        if selected.contains(id) {
            if !is_retired(next, id) {
                return Err(invalid());
            }
        } else if previous.content_states.get(id) != next.content_states.get(id) {
            return Err(invalid());
        }
    }
    crate::publication_shape::verify(
        previous,
        next,
        receipt,
        crate::publication_shape::ChangedTable::ContentStates,
        invalid,
    )
}

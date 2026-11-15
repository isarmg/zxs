//! The shared metadata plan closes its exact permanent receipt and complete State.
use super::{CurrentNamespaces, publication_delta};
use crate::business::file_metadata::{Plan, invalid, plan};
use crate::persist::file_metadata as codec;
use crate::{Error, FileMetadataError, OperationResult, Result, State};
use storage_contract::{CommitState, CurrentHealth, Event, FileId, Generation, ProtectionAtPublish};

pub(in crate::engine) fn verify(
    previous: &State,
    next: &State,
    current: Option<&CurrentNamespaces>,
    supplied: Option<&OperationResult>,
) -> Result<()> {
    let receipt = publication_delta::receipt(previous, next, codec::recognizes, invalid)?;
    publication_delta::events(
        previous, next, receipt.map(|receipt| receipt.operation_id), codec::is_kind, invalid,
    )?;
    let Some(receipt) = receipt else {
        if supplied.is_some() { return Err(invalid()); }
        return preserve_existing(previous, next);
    };
    if supplied.is_some_and(|supplied| supplied != receipt) { return Err(invalid()); }
    let principal = next.operation_principals.get(&receipt.operation_id).ok_or_else(invalid)?;
    if principal.is_empty() || next.sequence <= previous.sequence { return Err(invalid()); }
    let decoded = codec::decode(receipt)?;
    let replacement = plan(previous, current, decoded.request())?;
    let (history, branch) = match &replacement {
        Plan::Note { .. } => (None, None),
        Plan::Rename { repository, branch, .. } => {
            (Some(repository.history_tree_id.clone()), Some(branch.to_string()))
        }
    };
    let expected_receipt = OperationResult {
        operation_id: receipt.operation_id,
        kind: replacement.request().kind().into(),
        commit_id: None,
        history_tree_id: history,
        branch_id: branch,
        commit_state: CommitState::Published,
        protection_at_publish: ProtectionAtPublish::NotApplicable,
        current_health: CurrentHealth::Healthy,
        request_digest: codec::digest(replacement.request(), principal)?,
        sequence: next.sequence,
        result: decoded.encoded(&replacement)?,
    };
    if receipt != &expected_receipt { return Err(invalid()); }
    let mut expected = previous.clone();
    expected.begin_tracking();
    crate::container_storage::capture_names(&mut expected);
    expected.sequence = next.sequence;
    expected.business_generation = Generation::new(previous.business_generation).next()
        .map_err(|source| Error::FileMetadata(FileMetadataError::PublicationGeneration(source)))?
        .get();
    replacement.apply(&mut expected);
    crate::container_storage::capture_changed_names(previous, &mut expected);
    let object_id = expected_receipt.history_tree_id.clone()
        .unwrap_or_else(|| receipt.operation_id.to_string());
    expected.operations.insert(receipt.operation_id, expected_receipt);
    expected.operation_principals.insert(receipt.operation_id, principal.clone());
    expected.events.push(Event {
        sequence: next.sequence,
        operation_id: receipt.operation_id,
        kind: replacement.request().kind().into(),
        object_id,
    });
    if !crate::publication_shape::same_transition(previous, &expected, next) {
        return Err(invalid());
    }
    Ok(())
}

// Restore/copy can add labels to new identities, and retirement can remove them.
// Neither may rewrite labels of an identity that survives its original operation.
fn preserve_existing(previous: &State, next: &State) -> Result<()> {
    let file_note = |file: &String| -> Result<()> {
        let id: FileId = file.parse()?;
        if previous.container_tags.contains_key(&id) && next.container_tags.contains_key(&id)
            && previous.file_notes.get(file) != next.file_notes.get(file)
        {
            return Err(invalid());
        }
        Ok(())
    };
    match next.file_notes.changes_since(&previous.file_notes) {
        storage_contract::MapChanges::Keys(keys) => keys.iter().try_for_each(file_note)?,
        storage_contract::MapChanges::All => {
            for key in previous.file_notes.keys().chain(next.file_notes.keys()) { file_note(key)?; }
        }
    }
    let version_note = |version: &String| -> Result<()> {
        if previous.versions.contains_key(version) && next.versions.contains_key(version)
            && previous.version_notes.get(version) != next.version_notes.get(version)
        {
            return Err(invalid());
        }
        Ok(())
    };
    match next.version_notes.changes_since(&previous.version_notes) {
        storage_contract::MapChanges::Keys(keys) => keys.iter().try_for_each(version_note)?,
        storage_contract::MapChanges::All => {
            for key in previous.version_notes.keys().chain(next.version_notes.keys()) {
                version_note(key)?;
            }
        }
    }
    let branch_name = |id: &String| -> Result<()> {
        if let (Some(before), Some(after)) = (previous.branches.get(id), next.branches.get(id))
            && before.file_id.is_some() && before.name != after.name
        {
            return Err(invalid());
        }
        Ok(())
    };
    match next.branches.changes_since(&previous.branches) {
        storage_contract::MapChanges::Keys(keys) => keys.iter().try_for_each(branch_name)?,
        storage_contract::MapChanges::All => previous.branches.keys().try_for_each(branch_name)?,
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_file_metadata_tests.rs"]
mod tests;

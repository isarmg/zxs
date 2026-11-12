//! Exact container-label publication proof for live planning and cold replay.
use crate::business::file_tags::{apply, invalid, plan};
use crate::engine::CurrentNamespaces;
use crate::{OperationResult, Result, State};
use storage_contract::file_container_tags::KIND;
use storage_contract::{CommitState, CurrentHealth, Event, Generation, ProtectionAtPublish};

impl CurrentNamespaces {
    pub(in crate::engine) fn verify_new_file_tags(
        &self,
        previous: &State,
        next: &State,
        result: &OperationResult,
    ) -> Result<()> {
        verify(previous, next, Some(self), Some(result))
    }

    pub(super) fn verify_file_tag_publications(
        &self,
        previous: &State,
        next: &State,
    ) -> Result<()> {
        verify(previous, next, Some(self), None)
    }
}

pub(in crate::engine) fn verify(
    previous: &State,
    next: &State,
    current: Option<&CurrentNamespaces>,
    supplied: Option<&OperationResult>,
) -> Result<()> {
    let receipt = crate::engine::disk_registry::current::publication_delta::receipt(
        previous,
        next,
        |receipt| receipt.kind == KIND,
        invalid,
    )?;
    crate::engine::disk_registry::current::publication_delta::events(
        previous,
        next,
        receipt.map(|receipt| receipt.operation_id),
        |kind| kind == KIND,
        invalid,
    )?;
    let Some(receipt) = receipt else {
        if supplied.is_some() {
            return Err(invalid());
        }
        return crate::business::file_tags::preserve_existing(previous, next);
    };
    if supplied.is_some_and(|supplied| supplied != receipt) {
        return Err(invalid());
    }
    let principal = next
        .operation_principals
        .get(&receipt.operation_id)
        .ok_or_else(invalid)?;
    if principal.is_empty() || next.sequence <= previous.sequence {
        return Err(invalid());
    }
    let (actual, timestamp, epoch) = crate::persist::container_tags::decode(receipt)?;
    let replacement = plan(previous, current, &actual.request)?;
    if replacement != actual {
        return Err(invalid());
    }
    let expected_receipt = OperationResult {
        operation_id: receipt.operation_id,
        kind: KIND.into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: CommitState::Published,
        protection_at_publish: ProtectionAtPublish::NotApplicable,
        current_health: CurrentHealth::Healthy,
        request_digest: crate::persist::container_tags::digest(&actual.request, principal)?,
        sequence: next.sequence,
        result: crate::persist::container_tags::encoded(replacement.clone(), timestamp, epoch)?,
    };
    if receipt != &expected_receipt {
        return Err(invalid());
    }
    let mut expected = previous.clone();
    expected.begin_tracking();
    expected.sequence = next.sequence;
    expected.business_generation = Generation::new(previous.business_generation)
        .next()
        .map_err(|source| {
            crate::Error::ContainerTags(crate::ContainerTagsError::PublicationGeneration(source))
        })?
        .get();
    apply(&mut expected, &replacement);
    expected
        .operations
        .insert(receipt.operation_id, expected_receipt);
    expected
        .operation_principals
        .insert(receipt.operation_id, principal.clone());
    expected.events.push(Event {
        sequence: next.sequence,
        operation_id: receipt.operation_id,
        kind: KIND.into(),
        object_id: receipt.operation_id.to_string(),
    });
    if !crate::publication_shape::same_transition(previous, &expected, next) {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_file_tags_tests.rs"]
mod tests;

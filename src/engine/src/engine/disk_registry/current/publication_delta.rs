//! Shared audit rules for Current's permanent operation receipts and events.
use crate::{Error, Event, OperationId, OperationResult, Result, State};
use storage_contract::{MapChanges, SequenceChanges, TrackedOperations};

/// Select at most one new receipt of this family. Existing receipts and their
/// principals remain immutable even when only the principal map changed.
pub(super) fn receipt<'a>(
    previous: &State,
    next: &'a State,
    accepts: impl Fn(&OperationResult) -> bool,
    invalid: fn() -> Error,
) -> Result<Option<&'a OperationResult>> {
    let mut receipt = None;
    let mut inspect = |id: &OperationId| -> Result<()> {
        let before = previous.operations.get(id);
        let after = next.operations.get(id);
        if before.is_some_and(&accepts) {
            if before != after {
                return Err(invalid());
            }
        } else if let Some(after) = after.filter(|after| accepts(after))
            && (before.is_some() || after.operation_id != *id || receipt.replace(after).is_some())
        {
            return Err(invalid());
        }
        Ok(())
    };
    match next.operations.changes_since(&previous.operations) {
        MapChanges::Keys(ids) => ids.iter().try_for_each(&mut inspect)?,
        MapChanges::All => {
            for id in previous.operations.keys() {
                inspect(id)?;
            }
            for id in next
                .operations
                .keys()
                .filter(|id| !previous.operations.contains_key(*id))
            {
                inspect(id)?;
            }
        }
    }
    let preserved_owner = |id: &OperationId| {
        previous.operations.get(id).is_none_or(|old| {
            !accepts(old)
                || previous.operation_principals.get(id) == next.operation_principals.get(id)
        })
    };
    let owners_unchanged = match next
        .operation_principals
        .changes_since(&previous.operation_principals)
    {
        MapChanges::Keys(ids) => ids.iter().all(preserved_owner),
        MapChanges::All => previous.operations.keys().all(preserved_owner),
    };
    if !owners_unchanged {
        return Err(invalid());
    }
    Ok(receipt)
}

/// Existing events of this family cannot change; new events must belong to the
/// selected receipt. Cold states compare the full array, tracked states its suffix.
pub(super) fn events(
    previous: &State,
    next: &State,
    operation: Option<OperationId>,
    accepts: impl Fn(&str) -> bool,
    invalid: fn() -> Error,
) -> Result<()> {
    let start = match next.events.changes_since(&previous.events) {
        SequenceChanges::Append { start } => start,
        SequenceChanges::All => 0,
    };
    for index in start..previous.events.len().max(next.events.len()) {
        let before = previous.events.get(index);
        let after = next.events.get(index);
        if before != after
            && (before.is_some_and(|event| accepts(&event.kind))
                || after.is_some_and(|event| {
                    accepts(&event.kind)
                        && (before.is_some() || Some(event.operation_id) != operation)
                }))
        {
            return Err(invalid());
        }
    }
    Ok(())
}

pub(super) fn only_added<T: PartialEq>(
    before: &TrackedOperations<T>,
    after: &TrackedOperations<T>,
    key: OperationId,
    expected: &T,
) -> bool {
    if before.contains_key(&key)
        || before.len().checked_add(1) != Some(after.len())
        || after.get(&key) != Some(expected)
    {
        return false;
    }
    let equal = |id: &OperationId| *id == key || after.get(id) == before.get(id);
    match after.changes_since(before) {
        MapChanges::Keys(keys) => keys.iter().all(equal),
        MapChanges::All => before
            .iter()
            .all(|(id, value)| after.get(id) == Some(value)),
    }
}

pub(super) fn one_event(previous: &State, next: &State, expected: &Event) -> bool {
    previous.events.len().checked_add(1) == Some(next.events.len())
        && next.events.last() == Some(expected)
        && match next.events.changes_since(&previous.events) {
            SequenceChanges::Append { start } => start == previous.events.len(),
            SequenceChanges::All => next.events[..previous.events.len()] == previous.events[..],
        }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_publication_delta_tests.rs"]
mod tests;

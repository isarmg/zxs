//! Cancellation publishes one non-executable tombstone, never plan execution.
//! Local Ready files are checked by Engine::plan before the native mutation.
//! A cold publication may therefore add a Cancelled record with no published
//! Ready predecessor; it gains no namespace, payload or cleanup authority.
use super::*;
use crate::publication_shape::Table;
use crate::{
    CommitState, CurrentHealth, Event, OperationResult, Plan, PlanState, ProtectionAtPublish,
};
use json_codec::{Value, json};
use storage_contract::{MapChanges, TrackedMap};

const KIND: &str = "plan.cancel";

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "plan cancellation differs from its native request or closed metadata write set",
    )
}

impl CurrentNamespaces {
    pub(in crate::engine) fn verify_new_plan_cancellation(
        &self,
        previous: &State,
        next: &State,
        result: &OperationResult,
    ) -> Result<()> {
        let receipt =
            publication_delta::receipt(previous, next, |receipt| receipt.kind == KIND, invalid)?
                .ok_or_else(invalid)?;
        if receipt != result {
            return Err(invalid());
        }
        self.verify_plan_cancellation(previous, next, receipt)
    }

    pub(super) fn verify_plan_cancellations(&self, previous: &State, next: &State) -> Result<()> {
        match publication_delta::receipt(previous, next, |receipt| receipt.kind == KIND, invalid)? {
            Some(receipt) => self.verify_plan_cancellation(previous, next, receipt),
            None if plan_changed(&previous.plans, &next.plans) => Err(invalid()),
            None => Ok(()),
        }
    }

    fn verify_plan_cancellation(
        &self,
        previous: &State,
        next: &State,
        receipt: &OperationResult,
    ) -> Result<()> {
        let id = receipt.result["plan_id"].as_str().ok_or_else(invalid)?;
        let cancelled = next.plans.get(id).ok_or_else(invalid)?;
        let principal = next
            .operation_principals
            .get(&receipt.operation_id)
            .ok_or_else(invalid)?;
        verify_cancelled_plan(&previous.plans, id, principal, cancelled)?;
        let digest = Engine::native_request_digest(KIND, principal, &json!({"plan_id": id}))?;
        let expected = OperationResult {
            operation_id: receipt.operation_id,
            kind: KIND.into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: CommitState::Published,
            protection_at_publish: ProtectionAtPublish::NotApplicable,
            current_health: CurrentHealth::Healthy,
            request_digest: digest,
            sequence: next.sequence,
            result: json!({
                "plan_id": id,
                "state": "cancelled",
                "cancelled_plan_hash": crate::object_hash("plan-cancellation", cancelled)?,
                "timestamp": decimal_field(&receipt.result, "timestamp")?.to_string(),
                "membership_epoch": decimal_field(&receipt.result, "membership_epoch")?.to_string(),
                "version_protection": []
            }),
        };
        // An exhaustive State pattern forces new fields to choose their cancellation
        // semantics. In particular physical file_names/file_locations stay exact, even
        // when a restored State lost mutation tracking; neither is repaired from HEAD.
        if receipt != &expected
            || next.sequence <= previous.sequence
            || previous.business_generation.checked_add(1) != Some(next.business_generation)
            || !one_cancelled_plan(&previous.plans, &next.plans, id, cancelled)
            || !publication_delta::only_added(
                &previous.operations,
                &next.operations,
                receipt.operation_id,
                receipt,
            )
            || !publication_delta::only_added(
                &previous.operation_principals,
                &next.operation_principals,
                receipt.operation_id,
                principal,
            )
            || !one_cancellation_event(previous, next, receipt)
            || !crate::publication_shape::unchanged_except(previous, next, &[Table::Plans])
            || next.topology_generation != previous.topology_generation
            || next.control_auth != previous.control_auth
        {
            return Err(invalid());
        }
        Ok(())
    }
}

fn verify_cancelled_plan(
    previous: &TrackedMap<Plan>,
    id: &str,
    principal: &str,
    cancelled: &Plan,
) -> Result<()> {
    Engine::require_actor(principal)?;
    storage_contract::validate_id(id, "plan")?;
    if cancelled.plan_id != id
        || cancelled.principal != principal
        || cancelled.state != PlanState::Cancelled
        || crate::object_hash_value("plan", &cancelled.input)? != cancelled.digest
    {
        return Err(invalid());
    }
    if let Some(source) = previous.get(id) {
        let mut expected = source.clone();
        expected.state = PlanState::Cancelled;
        if source.state != PlanState::Ready || &expected != cancelled {
            return Err(invalid());
        }
    }
    // No predecessor is normal for a local-only preparation. The native entry
    // already checked its safe file read, identity, input digest, owner and Ready
    // state. The authenticated publication now retains only its cancellation;
    // no receipt field is promoted into a Ready plan or execution authority.
    Ok(())
}

fn decimal_field(value: &Value, key: &str) -> Result<u64> {
    let value = value[key].as_str().ok_or_else(invalid)?;
    Ok(storage_contract::parse_u64_decimal(value)?)
}

// No other currently admitted operation publishes a Plan transition. Checking
// every changed plan also rejects a forged kind paired with a Ready/Completed
// record, rather than letting it evade detection by dropping the Cancelled tag.
fn plan_changed(before: &TrackedMap<Plan>, after: &TrackedMap<Plan>) -> bool {
    let changed = |id: &String| before.get(id) != after.get(id);
    match after.changes_since(before) {
        MapChanges::Keys(keys) => keys.iter().any(changed),
        MapChanges::All => {
            before.keys().any(changed)
                || after
                    .keys()
                    .filter(|id| !before.contains_key(*id))
                    .any(changed)
        }
    }
}

fn one_cancelled_plan(
    before: &TrackedMap<Plan>,
    after: &TrackedMap<Plan>,
    id: &str,
    cancelled: &Plan,
) -> bool {
    let expected_len = if before.contains_key(id) {
        Some(before.len())
    } else {
        before.len().checked_add(1)
    };
    if expected_len != Some(after.len()) || after.get(id) != Some(cancelled) {
        return false;
    }
    match after.changes_since(before) {
        MapChanges::Keys(keys) => keys
            .iter()
            .all(|key| key == id || after.get(key) == before.get(key)),
        MapChanges::All => before
            .iter()
            .all(|(key, plan)| key == id || after.get(key) == Some(plan)),
    }
}

fn one_cancellation_event(previous: &State, next: &State, receipt: &OperationResult) -> bool {
    let expected = Event {
        sequence: next.sequence,
        operation_id: receipt.operation_id,
        kind: KIND.into(),
        object_id: receipt.operation_id.to_string(),
    };
    publication_delta::one_event(previous, next, &expected)
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_plan_cancellation_tests.rs"]
mod tests;

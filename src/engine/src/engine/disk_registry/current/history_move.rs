//! Native move advances both namespace admission and its single source D.
//! Replaying an authenticated snapshot never treats a receipt map as authority.
use super::*;
use crate::business::{FILE_HISTORY_MOVE_KIND, verify_current_move_transition};
use crate::{OperationId, OperationResult};

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "Current native move differs from its original complete transition",
    )
}

fn receipt<'a>(previous: &State, next: &'a State) -> Result<Option<&'a OperationResult>> {
    let mut found = None;
    let mut inspect = |id: &OperationId| -> Result<()> {
        let before = previous.operations.get(id);
        let after = next.operations.get(id);
        if before == after {
            return Ok(());
        }
        if before.into_iter().chain(after).any(|op| {
            op.kind == FILE_HISTORY_MOVE_KIND || op.result.get("move_request").is_some()
        }) {
            let current = after.ok_or_else(invalid)?;
            if before.is_some()
                || current.operation_id != *id
                || current.kind != FILE_HISTORY_MOVE_KIND
                || found.replace(current).is_some()
            {
                return Err(invalid());
            }
        }
        Ok(())
    };
    match next.operations.changes_since(&previous.operations) {
        storage_contract::MapChanges::Keys(keys) => {
            for id in keys.iter() {
                inspect(id)?;
            }
        }
        storage_contract::MapChanges::All => {
            for id in previous
                .operations
                .keys()
                .chain(next.operations.keys())
                .collect::<BTreeSet<_>>()
            {
                inspect(id)?;
            }
        }
    }
    Ok(found)
}

impl CurrentNamespaces {
    pub(crate) fn reject_degraded_history_move(
        candidate: &crate::journal::Candidate,
        next: &State,
    ) -> Result<()> {
        if next
            .operations
            .get(&candidate.operation_id)
            .is_some_and(|op| op.kind == FILE_HISTORY_MOVE_KIND)
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "native move must finish its original FULL and three-scope closure; degraded publication cannot hide its source",
            ));
        }
        Ok(())
    }

    pub(super) fn advance_history_move(
        &self,
        previous: &State,
        next: &State,
    ) -> Result<Option<Self>> {
        let Some(op) = receipt(previous, next)? else {
            return Ok(None);
        };
        // Activation has already been reconstructed from its native authority.
        // Retain that epoch, not a number supplied by the move's own receipt.
        let mut activations = previous
            .operations
            .values()
            .filter(|original| original.sequence == self.activation_sequence);
        let activation = activations.next().ok_or_else(invalid)?;
        if activations.next().is_some() {
            return Err(invalid());
        }
        let epoch = storage_contract::parse_u64_decimal(
            activation.result["membership_epoch"]
                .as_str()
                .ok_or_else(invalid)?,
        )
        .map_err(|_| invalid())?;
        verify_current_move_transition(previous, next, self, op, epoch)?;
        let mut view = self.clone();
        for (id, branch) in next
            .branches
            .iter()
            .filter(|(id, branch)| previous.branches.get(*id) != Some(*branch))
        {
            let entries = &next.commits[&branch.head_commit_id].entries;
            Arc::make_mut(&mut view.branches).insert(
                id.clone(),
                Arc::new(CurrentScope {
                    head: branch.head_commit_id.clone(),
                    entries: Arc::new(entries.clone()),
                    source_head: branch.head_commit_id.clone(),
                    source_entries: Arc::new(entries.clone()),
                    writable: true,
                    hidden_paths: BTreeSet::new(),
                }),
            );
        }
        view.reserve_history_copy_identities(previous, next);
        Ok(Some(view))
    }

    pub(in crate::engine) fn verify_history_move_candidate(
        &self,
        previous: &State,
        next: &State,
        candidate: &crate::journal::Candidate,
    ) -> Result<bool> {
        let Some(op) = receipt(previous, next)? else {
            return Ok(false);
        };
        if candidate.registry() != Some(&self.registry)
            || candidate.operation_id != op.operation_id
            || candidate.sequence != op.sequence
            || candidate.request_digest != op.request_digest
        {
            return Err(invalid());
        }
        let versions = op.result["version_ids"].as_array().ok_or_else(invalid)?;
        if versions.len() != candidate.versions.len() {
            return Err(invalid());
        }
        for (id, evidence) in versions.iter().zip(&candidate.versions) {
            let id = id.as_str().ok_or_else(invalid)?;
            if next.versions.get(id) != Some(&evidence.version)
                || next.placements.get(id) != Some(&evidence.placement)
            {
                return Err(invalid());
            }
        }
        Ok(true)
    }
}

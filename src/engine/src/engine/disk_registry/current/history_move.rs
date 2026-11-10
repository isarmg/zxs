//! Native move advances both namespace admission and its single source D.
//! Replaying an authenticated snapshot never treats a receipt map as authority.
use super::*;
use crate::OperationResult;
use crate::business::{FILE_HISTORY_MOVE_KIND, verify_current_move_transition};
use storage_contract::file_relocation::OBJECT_MOVE_KIND;

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "Current native move differs from its original complete transition",
    )
}

fn receipt<'a>(previous: &State, next: &'a State) -> Result<Option<&'a OperationResult>> {
    let selected = publication_delta::new_receipt(
        previous,
        next,
        |op| {
            matches!(op.kind.as_str(), FILE_HISTORY_MOVE_KIND | OBJECT_MOVE_KIND)
                || op.result.get("move_request").is_some()
                || op.result.get("movement").is_some()
        },
        invalid,
    )?;
    if selected
        .is_some_and(|op| !matches!(op.kind.as_str(), FILE_HISTORY_MOVE_KIND | OBJECT_MOVE_KIND))
    {
        return Err(invalid());
    }
    Ok(selected)
}

impl CurrentNamespaces {
    pub(crate) fn reject_degraded_history_move(
        candidate: &crate::journal::Candidate,
        next: &State,
    ) -> Result<()> {
        if next
            .operations
            .get(&candidate.operation_id)
            .is_some_and(|op| matches!(op.kind.as_str(), FILE_HISTORY_MOVE_KIND | OBJECT_MOVE_KIND))
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
        if op.kind == OBJECT_MOVE_KIND {
            crate::business::object_move::verify_transition(previous, next, Some(self), op)?;
        } else {
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
        }
        let mut view = self.clone();
        for (id, branch) in next
            .branches
            .iter()
            .filter(|(id, branch)| previous.branches.get(*id) != Some(*branch))
        {
            let entries = &next.commits[&branch.head_commit_id].entries;
            let old = self
                .branches
                .get(id)
                .filter(|_| op.kind == OBJECT_MOVE_KIND);
            Arc::make_mut(&mut view.branches).insert(
                id.clone(),
                Arc::new(CurrentScope {
                    head: branch.head_commit_id.clone(),
                    entries: Arc::new(entries.clone()),
                    source_head: old
                        .map(|scope| scope.source_head.clone())
                        .unwrap_or_else(|| branch.head_commit_id.clone()),
                    source_entries: old
                        .map(|scope| Arc::clone(&scope.source_entries))
                        .unwrap_or_else(|| Arc::new(entries.clone())),
                    writable: old.map(|scope| scope.writable).unwrap_or(true),
                    hidden_paths: old
                        .map(|scope| scope.hidden_paths.clone())
                        .unwrap_or_default(),
                }),
            );
        }
        if op.kind == OBJECT_MOVE_KIND {
            // verify_transition above has already admitted every imported and
            // retained branch, including nonselected nested repository branches.
            // Do not reclassify those branches as newly-created current forks.
            for (id, branch) in super::changed_directory_branches(previous, next) {
                if previous.directory_branches.get(id) == Some(branch) {
                    continue;
                }
                let repository = next
                    .repositories
                    .get(&branch.repository_id)
                    .ok_or_else(invalid)?;
                let owner = view
                    .branches
                    .get(&repository.main_branch_id)
                    .ok_or_else(invalid)?;
                let scope = self.project_directory_scope(branch, owner);
                Arc::make_mut(&mut view.directories).insert(id.clone(), scope);
            }
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
        let versions: Vec<&json_codec::Value> = if op.kind == OBJECT_MOVE_KIND {
            op.result["version_mapping"]
                .as_object()
                .ok_or_else(invalid)?
                .values()
                .collect()
        } else {
            op.result["version_ids"]
                .as_array()
                .ok_or_else(invalid)?
                .iter()
                .collect()
        };
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

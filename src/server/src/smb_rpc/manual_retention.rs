//! Retire only the private copy of an exactly acknowledged manual save.
use super::*;

pub(super) fn retire(
    shared: &Shared,
    identity: &Identity,
    request: &crate::PersistContainerDraft,
    limit: u64,
) -> Result<(), ApiError> {
    let revision = &request.revision;
    shared.budget.retire_manual_body(
        &identity.principal, revision.operation,
        || {
            let directory = platform_io::fs_safety::PrivateDirectory::open_trusted(&shared.spool)
                .map_err(crate::upload_fs_error)?;
            let retained = manual_recovery::retirement_request(
                &directory, revision.operation, &identity.principal,
            )?;
            if retained.entry != request.entry || retained.context != request.context
                || retained.revision != request.revision
            {
                return Err(ApiError::conflict(storage_contract::ErrorCode::RecoveryRequired,
                    "manual retirement record differs from the verified request"));
            }
            // Recheck the permanent result while the retirement lock is held.
            // Presence of spool material or matching bytes is not authorization.
            if shared.backend.container_draft_save_receipt(identity, request.clone())?.is_none() {
                return Err(ApiError::conflict(storage_contract::ErrorCode::RecoveryRequired,
                    "manual save has no exact permanent result; frozen material retained"));
            }
            Ok(())
        },
        |body| {
            if manual_recovery::fingerprint(body, limit)?
                != (revision.content_hash, revision.size)
            {
                return Err(ApiError::conflict(storage_contract::ErrorCode::RecoveryRequired,
                    "acknowledged manual body was changed; material retained"));
            }
            Ok(())
        },
    )
}

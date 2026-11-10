//! Current session admission and the actual target Engine own every intent write.
use super::*;
use storage_contract::storage_foreign::ForeignDiskIdentity;
use storage_engine::{
    ConfirmedForeignImportIntent, ForeignEvidenceDigest, ForeignImportIntentInfo,
    ForeignImportIntentRequest, ForeignImportIntentSource, ForeignImportIntentTarget,
    PreparedForeignImportIntent,
};

pub(crate) fn target_identity(
    target: &ForeignDiskIdentity,
) -> Result<storage_engine::DiskRegistrationIdentity, ApiError> {
    storage_contract::validate_id(&target.pool_id, "pool").map_err(ApiError::invalid_source)?;
    storage_contract::validate_id(&target.disk_id, "disk").map_err(ApiError::invalid_source)?;
    name_policy::validate_slot(&target.slot_id).map_err(ApiError::invalid_source)?;
    if target.binding_generation == 0 {
        return Err(ApiError::invalid(
            "foreign intent target has zero binding generation",
        ));
    }
    Ok(storage_engine::DiskRegistrationIdentity {
        pool_id: target.pool_id.clone(),
        disk_id: target.disk_id.clone(),
        slot_id: target.slot_id.clone(),
        binding_generation: target.binding_generation,
    })
}
impl EngineBackend {
    pub(crate) fn prepare_foreign_intent(
        &self,
        identity: &Identity,
        source: ForeignImportIntentSource,
        target: ForeignImportIntentTarget,
        request: ForeignImportIntentRequest,
    ) -> Result<PreparedForeignImportIntent, ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        if request.principal != current.principal {
            return Err(ApiError::forbidden());
        }
        engine
            .prepare_foreign_import_intent(source, target, request)
            .map_err(error)
    }
    pub(crate) fn save_foreign_intent(
        &self,
        identity: &Identity,
        confirmed: ConfirmedForeignImportIntent,
        ensure_running: impl FnOnce() -> Result<(), ApiError>,
    ) -> Result<ForeignImportIntentInfo, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        ensure_running()?;
        engine
            .save_foreign_import_intent(confirmed, &current.principal)
            .map_err(error)
    }
    pub(crate) fn reconcile_foreign_intent(
        &self,
        identity: &Identity,
        query: &storage_contract::storage_foreign_intent::ForeignIntentQuery,
        digest: &ForeignEvidenceDigest,
        ensure_running: impl FnOnce() -> Result<(), ApiError>,
    ) -> Result<Option<ForeignImportIntentInfo>, ApiError> {
        let target = target_identity(&query.target)?;
        let (mut engine, current) = self.lock_current(identity)?;
        ensure_running()?;
        engine
            .reconcile_foreign_import_intent(
                &target,
                query.operation_id,
                &current.principal,
                digest,
            )
            .map_err(error)
    }
    pub(crate) fn foreign_intent_status(
        &self,
        identity: &Identity,
        query: &storage_contract::storage_foreign_intent::ForeignIntentQuery,
    ) -> Result<Option<ForeignImportIntentInfo>, ApiError> {
        let target = target_identity(&query.target)?;
        let (engine, current) = self.lock_current(identity)?;
        engine
            .foreign_import_intent(&target, query.operation_id, &current.principal)
            .map_err(error)
    }
}

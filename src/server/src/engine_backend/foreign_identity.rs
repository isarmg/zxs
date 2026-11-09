//! Administrator-authenticated capture/recheck of an inert foreign naming
//! baseline. Long original-source I/O stays outside the Engine mutex.
use super::*;
use storage_engine::{ForeignIdentityTarget, ForeignIdentityTargetRequest};

impl EngineBackend {
    pub fn capture_foreign_identity_target(
        &self,
        identity: &Identity,
        request: ForeignIdentityTargetRequest,
    ) -> Result<ForeignIdentityTarget, ApiError> {
        let engine = crate::lock(&self.engine);
        current_protected_identity(&engine, identity)?;
        if identity.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        engine
            .capture_foreign_identity_target(request)
            .map_err(error)
    }
    pub fn recheck_foreign_identity_target(
        &self,
        identity: &Identity,
        target: &ForeignIdentityTarget,
    ) -> Result<(), ApiError> {
        let engine = crate::lock(&self.engine);
        current_protected_identity(&engine, identity)?;
        if identity.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        engine
            .recheck_foreign_identity_target(target)
            .map_err(error)
    }
}

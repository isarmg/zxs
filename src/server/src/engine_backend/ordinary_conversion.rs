//! A conversion uses the exact newly admitted primary and normal workspace proof.
use super::*;
use storage_contract::{
    CommitRequest, OperationId, OperationResult, storage_addition::AdditionPreview,
};

impl EngineBackend {
    pub(crate) fn ordinary_conversion_identity(
        target: &AdditionPreview,
    ) -> Result<storage_engine::DiskRegistrationIdentity, ApiError> {
        let [addition] = target.request.additions.as_slice() else {
            return Err(ApiError::invalid(
                "ordinary conversion requires exactly one new primary",
            ));
        };
        Ok(storage_engine::DiskRegistrationIdentity {
            pool_id: target.pool_id.clone(),
            disk_id: format!(
                "disk_{}",
                &crate::management::storage_addition::digest(&(
                    target.request.operation_id,
                    &addition.slot_id
                ))[..32]
            ),
            slot_id: addition.slot_id.clone(),
            binding_generation: 1,
        })
    }

    pub(crate) fn stop_ordinary_conversion(
        &self,
        identity: &Identity,
        request: &storage_engine::DirectoryImportStopRequest,
    ) -> Result<storage_engine::DirectoryImportStopOutcome, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        if request.principal != current.principal {
            return Err(ApiError::forbidden());
        }
        engine.stop_directory_import(request).map_err(error)
    }

    pub(crate) fn ordinary_conversion_stop_result(
        &self,
        identity: &Identity,
        request: &storage_engine::DirectoryImportStopRequest,
    ) -> Result<Option<storage_engine::DirectoryImportStopOutcome>, ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        if request.principal != current.principal {
            return Err(ApiError::forbidden());
        }
        engine.directory_import_stop_result(request).map_err(error)
    }

    pub(crate) fn ordinary_conversion_stop_settled(
        &self,
        identity: &Identity,
        request: &storage_engine::DirectoryImportStopRequest,
    ) -> Result<bool, ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        engine
            .directory_import_stop_result(request)
            .map(|result| result.is_some())
            .map_err(error)
    }
    /// Check a prior workflow's exact publication without revealing its result
    /// or impersonating its owner. The current administrator only gets a settled
    /// boolean, allowing later independent work after credential recovery.
    pub(crate) fn ordinary_conversion_settled(
        &self,
        identity: &Identity,
        request: &CommitRequest,
    ) -> Result<bool, ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        match engine.native_workspace_commit_completed(request) {
            Ok(completed) => Ok(completed),
            Err(source) if source.code() == storage_contract::ErrorCode::OperationNotFound => {
                Ok(false)
            }
            Err(source) => Err(error(source)),
        }
    }

    pub(crate) fn ordinary_conversion_result(
        &self,
        identity: &Identity,
        request: &CommitRequest,
    ) -> Result<Option<OperationResult>, ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        if request.principal != current.principal {
            return Err(ApiError::forbidden());
        }
        match engine.native_workspace_commit_completed(request) {
            Ok(true) => Ok(Some(
                operations::authorized_record(&engine, &current, request.operation_id)?
                    .into_result(),
            )),
            Ok(false) => Ok(None),
            Err(source) if source.code() == storage_contract::ErrorCode::OperationNotFound => {
                Ok(None)
            }
            Err(source) => Err(error(source)),
        }
    }

    fn ordinary_conversion_target(
        engine: &storage_engine::Engine,
        target: &AdditionPreview,
    ) -> Result<storage_engine::DiskRegistrationIdentity, ApiError> {
        let [addition] = target.request.additions.as_slice() else {
            return Err(ApiError::invalid(
                "ordinary conversion requires exactly one new primary",
            ));
        };
        let expected = Self::ordinary_conversion_identity(target)?;
        let disk = engine
            .config()
            .disks
            .iter()
            .find(|disk| disk.slot_id == addition.slot_id)
            .ok_or_else(|| ApiError::invalid("converted primary registration is absent"))?;
        if disk.disk_id != expected.disk_id
            || disk.root != Path::new(&addition.selection.root_path)
            || disk.binding_generation != 1
            || disk.level != 1
            || disk.upstream_slot_id.is_some()
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::DiskIdentityMismatch,
                "conversion target differs from the reviewed new primary",
            ));
        }
        Ok(expected)
    }

    pub(crate) fn inspect_registered_conversion(
        &self,
        identity: &Identity,
        target: &AdditionPreview,
        source: std::fs::File,
        source_path: &Path,
    ) -> Result<storage_engine::PreparedOrdinaryDirectory, ApiError> {
        let target = {
            let (engine, _) = self.lock_current(identity)?;
            let target = Self::ordinary_conversion_target(&engine, target)?;
            storage_engine::PreparedOrdinaryDirectory::capture_registered_target(&engine, &target)
                .map_err(error)?
        };
        target.inspect(source, source_path).map_err(error)
    }

    pub(crate) fn run_ordinary_conversion(
        &self,
        identity: &Identity,
        target: &AdditionPreview,
        source: storage_engine::PreparedOrdinaryDirectory,
        operation: OperationId,
        observer: impl FnMut(&storage_engine::DirectoryImportProgress) -> storage_engine::Result<()>,
    ) -> Result<OperationResult, OrdinaryConversionFailure> {
        let mut engine = crate::lock(&self.engine);
        let current = current_protected_identity(&engine, identity)
            .map_err(OrdinaryConversionFailure::Admission)?;
        let target = Self::ordinary_conversion_target(&engine, target)
            .map_err(OrdinaryConversionFailure::Admission)?;
        let prepared = source
            .prepare_new_disk_import(&engine, &target.slot_id)
            .map_err(|source| OrdinaryConversionFailure::Admission(error(source)))?;
        let confirmation = storage_engine::DirectoryConversionConfirmation::contents_only(
            prepared.preview().digest,
        );
        prepared
            .execute_with_progress(
                &mut engine,
                &current.principal,
                operation,
                confirmation,
                observer,
            )
            .map_err(OrdinaryConversionFailure::Import)
    }
}

pub(crate) enum OrdinaryConversionFailure {
    Admission(ApiError),
    Import(storage_engine::DirectoryImportFailure),
}
impl OrdinaryConversionFailure {
    pub(crate) fn before_import(&self) -> bool {
        matches!(self, Self::Admission(_))
    }
    pub(crate) fn into_api(self) -> ApiError {
        match self {
            Self::Admission(error) => error,
            Self::Import(source) => ApiError::new(
                http::StatusCode::SERVICE_UNAVAILABLE,
                source.cause().code(),
                source.to_string(),
            )
            .with_source(source),
        }
    }
}

//! Authenticated, scoped read-only independent-disk preview. No path, display
//! identity or successful observation is converted into a registration permit.
use super::*;
use platform_io::fs_safety::PrivateDirectory;
use storage_contract::storage_independent_topology::{
    IndependentDiskPreview, IndependentDiskPreviewRequest,
};
use storage_engine::disk_authority::{self, IndependentSourceReader, source_topology_display};

impl ManagementBackend {
    pub(super) fn independent_disk_preview(
        &self,
        identity: &Identity,
        request: IndependentDiskPreviewRequest,
    ) -> Result<IndependentDiskPreview, ApiError> {
        self.inner.current_management_identity(identity)?;
        let result = (|| {
            let (excluded, stopped) = self.discovery_scope(identity)?;
            let observed = &request.source;
            let file = crate::storage_directories::reopen_source(
                self.inner.discovery_root.as_str(),
                observed,
                &excluded,
                &stopped,
            )?;
            let root = PrivateDirectory::from_directory(file, PathBuf::from(&observed.path))
                .map_err(disk_authority::Error::Filesystem)
                .map_err(source_error)?;
            let reader = IndependentSourceReader::from_directory(root).map_err(source_error)?;
            let topology = reader.source_topology().map_err(source_error)?;
            let topology = source_topology_display(&topology).map_err(display_error)?;
            // This is a comparison with the original discovery observation, not a
            // second source from which to read. The lifecycle gate remains held by
            // setup_operation, and revoked sessions win before releasing any result.
            let recheck = crate::storage_directories::reopen_source(
                self.inner.discovery_root.as_str(),
                observed,
                &excluded,
                &stopped,
            );
            drop(recheck?);
            let output = IndependentDiskPreview {
                source: request.source,
                topology,
            };
            crate::history_window_encode::encode_bounded(
                &output,
                storage_contract::storage_independent_source::MAX_OBSERVATION_BYTES,
            )?;
            Ok(output)
        })();
        self.inner.current_management_identity(identity)?;
        result
    }
}

fn display_error(
    source: storage_contract::storage_independent_source::InvalidObservation,
) -> ApiError {
    use storage_contract::{ErrorCode, storage_independent_source::InvalidObservation};
    let (status, code) = match source {
        InvalidObservation::TopologyBudget | InvalidObservation::PinBudget => (
            http::StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::ResourceLimit,
        ),
        _ => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::InternalError,
        ),
    };
    ApiError::new(
        status,
        code,
        "independent source graph could not be represented",
    )
    .with_source(source)
}

fn source_error(source: disk_authority::Error) -> ApiError {
    use storage_contract::ErrorCode;
    let (status, code) = match &source {
        disk_authority::Error::Budget => (
            http::StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::ResourceLimit,
        ),
        disk_authority::Error::Identity => (http::StatusCode::CONFLICT, ErrorCode::DiskChanged),
        disk_authority::Error::Filesystem(platform_io::fs_safety::Error::UnsafePermissions(_)) => {
            (http::StatusCode::FORBIDDEN, ErrorCode::PermissionDenied)
        }
        disk_authority::Error::Io(error)
        | disk_authority::Error::Filesystem(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            (http::StatusCode::FORBIDDEN, ErrorCode::PermissionDenied)
        }
        disk_authority::Error::Io(_) | disk_authority::Error::Filesystem(_) => (
            http::StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::DiskUnavailable,
        ),
        _ => (http::StatusCode::CONFLICT, ErrorCode::RecoveryRequired),
    };
    // Keep the complete source for server diagnostics without disclosing paths
    // or arbitrary untrusted disk bytes through its Display implementation.
    ApiError::new(status, code, "independent disk source inspection failed").with_source(source)
}

#[cfg(test)]
#[path = "../../../../test/server/unit/management/independent_preview.rs"]
mod tests;

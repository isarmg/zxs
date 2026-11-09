//! Route a saved export to the same authorized outer namespace. Source and
//! destination files are not looked up again after an unknown publication.
use super::*;
use crate::smb_rpc::namespace::{NamespaceInput, RetainedExport};

impl DefaultShare {
    pub(super) fn resume_directory_export(
        &mut self,
        source_path: &str,
        destination: &str,
        replace: bool,
        operation: OperationId,
    ) -> Result<Option<Value>, ApiError> {
        let Some(retained) = RetainedExport::read(&self.shared, &self.identity, operation)? else {
            return Ok(None);
        };
        let source = self.resolve(source_path, SmbPathIntent::Lookup)?;
        let target = self.resolve(destination, SmbPathIntent::Lookup)?;
        if source.repository.repository_id != retained.request.repository_id
            || target.repository.repository_id != retained.request.repository_id
            || source.relative_path.is_empty()
            || target.relative_path.is_empty()
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "directory export retry changed its original namespace",
            ));
        }
        let key = self.session_key(source.repository)?;
        let input = NamespaceInput::parse(
            NamespaceOperation::Rename,
            source.relative_path,
            Some(target.relative_path),
            replace,
        )?;
        self.sessions
            .get_mut(&key)
            .expect("invariant: export session was inserted")
            .retry_directory_export(&input, retained)
            .map(Some)
    }
}

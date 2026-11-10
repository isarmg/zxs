//! Explicit retries keep the original export request. A retained spool is only
//! an intent, never proof of publication or permission to resolve a new source.
use super::*;
use data_serialization::Deserialize;

const FORMAT: &str = "smb-directory-history-export";

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Record {
    format: String,
    principal: String,
    operation_id: OperationId,
    publication_outcome: String,
    #[serialization(with = "storage_contract::u64_string")]
    account_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    binding_generation: u64,
    replace: bool,
    request: Request,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Request {
    repository_id: String,
    branch_id: String,
    source_path: String,
    destination: String,
    expected_head: String,
}

pub(in crate::smb_rpc) struct RetainedExport {
    admission: NamespaceAdmission,
    pub(in crate::smb_rpc) request: crate::DirectoryFileExport,
    account_generation: u64,
    binding_generation: u64,
    replace: bool,
}

fn recovery(message: &str) -> ApiError {
    ApiError::conflict(storage_contract::ErrorCode::RecoveryRequired, message)
}

impl RetainedExport {
    pub(in crate::smb_rpc) fn read(
        shared: &Arc<Shared>,
        identity: &Identity,
        operation: OperationId,
    ) -> Result<Option<Self>, ApiError> {
        let Some(admission) = NamespaceAdmission::retain_move(shared, identity, operation)? else {
            return Ok(None);
        };
        let value: Value = json_codec::from_slice(&admission.retained_move_bytes()?)
            .map_err(ApiError::invalid_source)?;
        if value.get("format").and_then(Value::as_str) != Some(FORMAT) {
            // Do not consume or rewrite another native operation's spool.
            return Ok(None);
        }
        let record: Record = json_codec::from_value(value).map_err(ApiError::invalid_source)?;
        if record.format != FORMAT
            || record.principal != identity.principal
            || record.operation_id != operation
            || record.publication_outcome != "unconfirmed"
        {
            return Err(recovery(
                "retained directory export belongs to another original request",
            ));
        }
        storage_contract::validate_id(&record.request.repository_id, "repo")
            .map_err(ApiError::invalid_source)?;
        storage_contract::validate_id(&record.request.branch_id, "branch")
            .map_err(ApiError::invalid_source)?;
        storage_contract::validate_id(&record.request.expected_head, "commit")
            .map_err(ApiError::invalid_source)?;
        let request = crate::DirectoryFileExport {
            repository_id: record.request.repository_id,
            branch_id: record.request.branch_id,
            source_path: name_policy::PortablePath::parse(&record.request.source_path)
                .map_err(ApiError::invalid_source)?,
            destination: name_policy::PortablePath::parse(&record.request.destination)
                .map_err(ApiError::invalid_source)?,
            expected_head: record.request.expected_head,
        };
        Ok(Some(Self {
            admission,
            request,
            account_generation: record.account_generation,
            binding_generation: record.binding_generation,
            replace: record.replace,
        }))
    }
}

impl Session {
    pub(super) fn try_directory_file_export(
        &self,
        source: &name_policy::LookupPath,
        destination: &name_policy::PortablePath,
        replace: bool,
        admission: &NamespaceAdmission,
    ) -> Result<Option<Value>, ApiError> {
        let Some(request) = self.shared.backend.prepare_directory_file_export(
            &self.identity,
            crate::PrepareDirectoryFileExport {
                branch_id: self.branch.clone(),
                source_path: source.clone(),
                destination: destination.clone(),
            },
        )?
        else {
            return Ok(None);
        };
        let binding = self
            .identity
            .smb_binding
            .as_ref()
            .ok_or_else(ApiError::forbidden)?;
        if binding.repository_id != request.repository_id || binding.branch_id != request.branch_id
        {
            return Err(ApiError::forbidden());
        }
        admission.record_intent(&json!({
            "format":FORMAT,
            "principal":self.identity.principal,
            "operation_id":admission.operation_id(),
            "publication_outcome":"unconfirmed",
            "account_generation":self.account_generation.to_string(),
            "binding_generation":binding.generation.to_string(),
            "replace":replace,
            "request":{
                "repository_id":request.repository_id,
                "branch_id":request.branch_id,
                "source_path":request.source_path.as_str(),
                "destination":request.destination.as_str(),
                "expected_head":request.expected_head,
            },
        }))?;
        self.submit_directory_export(request, admission).map(Some)
    }

    pub(in crate::smb_rpc) fn retry_directory_export(
        &mut self,
        input: &NamespaceInput,
        retained: RetainedExport,
    ) -> Result<Value, ApiError> {
        let binding = self
            .identity
            .smb_binding
            .as_ref()
            .ok_or_else(ApiError::forbidden)?;
        if input.kind != NamespaceOperation::Rename
            || input.replace != retained.replace
            || input.change.key() != retained.request.source_path.key()
            || input.change.destination() != Some(&retained.request.destination)
            || self.account_generation != retained.account_generation
            || binding.generation != retained.binding_generation
            || binding.repository_id != retained.request.repository_id
            || binding.branch_id != retained.request.branch_id
            || self.branch != retained.request.branch_id
        {
            return Err(recovery(
                "directory export retry differs from the captured request or binding",
            ));
        }
        let _paths = path_reservation::PathReservation::acquire(
            &self.shared,
            [
                (self.branch.as_str(), retained.request.source_path.key()),
                (self.branch.as_str(), retained.request.destination.key()),
            ],
        )?;
        self.submit_directory_export(retained.request, &retained.admission)
    }

    fn submit_directory_export(
        &self,
        request: crate::DirectoryFileExport,
        admission: &NamespaceAdmission,
    ) -> Result<Value, ApiError> {
        let operation = admission.operation_id();
        let result = self.shared.backend.export_directory_file(
            &self.identity,
            crate::ExportDirectoryFile {
                request,
                operation_id: operation,
            },
        )?;
        if result.operation_id != operation || result.kind != "file.directory.export" {
            return Err(recovery(
                "directory export returned a different permanent receipt",
            ));
        }
        if result.commit_state != storage_contract::CommitState::Published
            || result.protection_at_publish != storage_contract::ProtectionAtPublish::TargetMet
            || result.current_health != storage_contract::CurrentHealth::Healthy
        {
            return Err(recovery(
                "directory export is not fully published; retain the original operation",
            ));
        }
        let reply = crate::operation_wire::operation_wire(result.into())?;
        admission.finish_published();
        Ok(reply)
    }
}

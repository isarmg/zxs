//! Explicit container policy calls never submit file content or create history.
use crate::{Client, Error};
use json_codec::{Value, json};
use storage_contract::container_edit::{CommitMode, ContainerEdit};
use storage_contract::file_container_tags::ReadRequest;
use storage_contract::{Generation, OperationId};

#[derive(Debug, error_definition::Error)]
pub enum DraftPrepareError {
    #[error("prepared draft commit does not match the explicitly selected draft")]
    Binding,
    #[error("invalid prepared draft commit: {0}")]
    Wire(#[from] storage_contract::PublicCommitParseError),
}

impl Client {
    pub async fn container_edit(&self, request: &ReadRequest) -> Result<ContainerEdit, Error> {
        let query = url_encoding::form::Serializer::new(String::new())
            .append_pair("repository", &request.context.repository_id.to_string())
            .append_pair("path", request.context.path.as_str())
            .finish();
        let route = format!("/api/file-manager/files/{}/edit-mode?{query}", request.file_id);
        Ok(json_codec::from_value(self.request("GET", &route, None).await?)?)
    }

    /// Explicit preparation only. Retain the returned envelope and use
    /// commit_prepared to publish it, including after a lost commit response.
    pub async fn prepare_container_draft_commit(
        &self, request: &ReadRequest, generation: Generation,
        draft: OperationId, operation: OperationId, message: &str,
    ) -> Result<Value, Error> {
        let seal = self.request("POST", &format!(
            "/api/file-manager/files/{}/prepare-draft-commit", request.file_id),
            Some(&json!({"repository":request.context.repository_id,"path":request.context.path,
                "expected_generation":generation,"draft_operation":draft,"operation_id":operation})),
        ).await?;
        if seal["operation_id"].as_str() != Some(operation.to_string().as_str()) {
            return Err(DraftPrepareError::Binding.into());
        }
        let workspace = seal["workspace_id"].as_str()
            .ok_or(DraftPrepareError::Binding)?.to_owned();
        let body = storage_contract::client_protocol::prepare_commit(
            seal, &workspace, &operation.to_string(), message, None,
        )?;
        let envelope = storage_contract::PublicCommitEnvelope::parse(&workspace, body.clone())
            .map_err(DraftPrepareError::Wire)?;
        if envelope.container_draft_commit().is_none_or(|binding| {
            binding.file != request.file_id || binding.generation != generation
                || binding.revision.operation != draft || binding.operation != operation
        }) { return Err(DraftPrepareError::Binding.into()); }
        Ok(body)
    }

    /// Keep the context, generation and operation ID unchanged after an uncertain response.
    /// A dirty draft must be explicitly committed or discarded, never implicitly published here.
    pub async fn set_container_commit_mode(
        &self,
        request: &ReadRequest,
        expected_generation: Generation,
        mode: CommitMode,
        operation_id: OperationId,
    ) -> Result<Value, Error> {
        self.request("POST", &format!("/api/file-manager/files/{}/edit-mode", request.file_id),
            Some(&json!({
                "repository": request.context.repository_id,
                "path": request.context.path,
                "expected_generation": expected_generation,
                "mode": mode,
                "operation_id": operation_id,
            })),
        ).await
    }
}

#[cfg(test)]
#[path = "../../../test/client/unit/container_edit.rs"]
mod tests;

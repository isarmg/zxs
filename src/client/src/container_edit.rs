//! Explicit container policy calls never submit file content or create history.
use crate::{Client, Error};
use json_codec::{Value, json};
use storage_contract::container_edit::{CommitMode, ContainerEdit};
use storage_contract::file_container_tags::ReadRequest;
use storage_contract::{Generation, OperationId};

impl Client {
    pub async fn container_edit(&self, request: &ReadRequest) -> Result<ContainerEdit, Error> {
        let query = url_encoding::form::Serializer::new(String::new())
            .append_pair("repository", &request.context.repository_id.to_string())
            .append_pair("path", request.context.path.as_str())
            .finish();
        let route = format!("/api/file-manager/files/{}/edit-mode?{query}", request.file_id);
        Ok(json_codec::from_value(self.request("GET", &route, None).await?)?)
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

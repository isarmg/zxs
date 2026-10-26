//! Immutable decoded public commit evidence and typed seal wire response.
use crate::{ErrorCode, OperationId};
use json_codec::{Value, json};
use std::collections::BTreeMap;

/// Frozen public spelling is retained as evidence; only boundary parsing can
/// construct this envelope. Domain code never reparses its JSON fields.
#[derive(Clone, Debug)]
pub struct PublicCommitEnvelope {
    body: Value,
    fields: PublicCommitBody,
    route_workspace: String,
    operation_id: OperationId,
    fencing_token: u64,
    digest: String,
}

#[derive(Debug, error_definition::Error)]
pub enum PublicCommitParseError {
    #[error("invalid public commit shape: {0}")]
    Shape(#[source] json_codec::Error),
    #[error(transparent)]
    Field(#[from] crate::Error),
    #[error("invalid fencing token: {0}")]
    Fence(#[source] crate::Error),
}
impl PublicCommitParseError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Shape(_) | Self::Fence(_) => ErrorCode::InvalidRequest,
            Self::Field(error) => error.code(),
        }
    }
}

#[derive(Clone, Debug, data_serialization::Deserialize)]
#[serialization(deny_unknown_fields)]
struct PublicCommitBody {
    #[serialization(deserialize_with = "nonempty")]
    workspace_id: String,
    #[serialization(deserialize_with = "nonempty")]
    operation_id: String,
    repository_id: String,
    branch_id: String,
    #[serialization(deserialize_with = "nonempty")]
    expected_head: String,
    #[serialization(deserialize_with = "nonempty")]
    fencing_token: String,
    message: String,
    requested_protection: String,
    file_placement: BTreeMap<String, FilePlacement>,
    expected_topology_generation: String,
    sealed_manifest_hash: String,
    request_digest: String,
}

#[derive(Clone, Debug, data_serialization::Deserialize, data_serialization::Serialize)]
#[serialization(deny_unknown_fields)]
pub struct FilePlacement {
    pub primary_slot_id: String,
    pub target_copies: u32,
}

impl PublicCommitEnvelope {
    /// Called only at an external HTTP/persisted request decoding boundary.
    /// The route workspace participates in the digest independently of body binding.
    pub fn parse(
        workspace: &str,
        body: Value,
    ) -> std::result::Result<Self, PublicCommitParseError> {
        let fields = <PublicCommitBody as data_serialization::Deserialize>::deserialize(&body)
            .map_err(PublicCommitParseError::Shape)?;
        let fencing_token = crate::parse_u64_decimal(&fields.fencing_token)
            .map_err(PublicCommitParseError::Fence)?;
        let operation_id = fields.operation_id.parse()?;
        let digest = crate::request_digest(
            "POST",
            "/api/workspaces/{workspace}/commit",
            json!({"workspace":workspace}),
            json!({}),
            &body,
        )?;
        Ok(Self {
            body,
            fields,
            route_workspace: workspace.into(),
            operation_id,
            fencing_token,
            digest,
        })
    }
    pub fn workspace_id(&self) -> &str {
        &self.fields.workspace_id
    }
    pub fn route_workspace(&self) -> &str {
        &self.route_workspace
    }
    pub fn operation_id(&self) -> OperationId {
        self.operation_id
    }
    pub fn fencing_token(&self) -> u64 {
        self.fencing_token
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn expected_head(&self) -> &str {
        &self.fields.expected_head
    }
    pub fn message(&self) -> &str {
        &self.fields.message
    }
    pub fn repository_id(&self) -> &str {
        &self.fields.repository_id
    }
    pub fn branch_id(&self) -> &str {
        &self.fields.branch_id
    }
    pub fn requested_protection(&self) -> &str {
        &self.fields.requested_protection
    }
    pub fn expected_topology_generation(&self) -> &str {
        &self.fields.expected_topology_generation
    }
    pub fn sealed_manifest_hash(&self) -> &str {
        &self.fields.sealed_manifest_hash
    }
    pub fn claimed_digest(&self) -> &str {
        &self.fields.request_digest
    }
    pub fn file_placement(&self) -> &BTreeMap<String, FilePlacement> {
        &self.fields.file_placement
    }
    /// Consume the unchanged boundary evidence only when persisting the accepted request.
    pub fn into_body(self) -> Value {
        self.body
    }
}

/// Frozen seal response, built from domain values and serialized by the adapter.
#[derive(Debug, data_serialization::Serialize)]
pub struct SealedWorkspace {
    pub operation_id: OperationId,
    pub workspace_id: String,
    pub repository_id: String,
    pub branch_id: String,
    pub expected_head: String,
    #[serialization(with = "crate::u64_string")]
    pub fencing_token: u64,
    pub message: String,
    pub requested_protection: &'static str,
    pub file_placement: BTreeMap<String, FilePlacement>,
    #[serialization(with = "crate::u64_string")]
    pub expected_topology_generation: u64,
    pub sealed_manifest_hash: String,
}

fn nonempty<'de, D: data_serialization::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    use data_serialization::{Deserialize, de::Error};
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        return Err(D::Error::custom("expected nonempty string"));
    }
    Ok(value)
}

#[cfg(test)]
#[path = "../../../test/contract/unit/public_commit.rs"]
mod tests;

//! Decode archive identity once without changing its raw receipt response shape.
use crate::{
    ApiError, AppState, ArchiveBranch, ArchiveBranchTarget,
    backend_archive::ArchiveResponse,
    http_boundary::{ApiJson, ApiPath, Authenticated, JsonBody},
};
use data_serialization::{Deserialize, Deserializer};
use http_routing::{Json, extract::State};
use json_codec::{Map, Value};

pub(crate) enum ArchiveBranchPath {
    Canonical(storage_contract::BranchId),
    Malformed {
        original: String,
        source: storage_contract::Error,
    },
}

impl<'de> Deserialize<'de> for ArchiveBranchPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let original = String::deserialize(deserializer)?;
        Ok(match original.parse() {
            Ok(branch) => Self::Canonical(branch),
            Err(source) => Self::Malformed { original, source },
        })
    }
}

impl ArchiveBranchPath {
    fn bind(self, body: &ArchiveBody) -> Result<ArchiveBranchTarget, ApiError> {
        match self {
            Self::Canonical(branch) => {
                body.matches_branch(&branch.to_string())?;
                Ok(branch.into())
            }
            Self::Malformed { original, source } => {
                body.matches_branch(&original)?;
                Ok(ArchiveBranchTarget::malformed(source))
            }
        }
    }
}

pub(crate) struct ArchiveBody(Map<String, Value>);

impl JsonBody for ArchiveBody {
    fn from_json(body: Value) -> Result<Self, ApiError> {
        match body {
            Value::Object(body) => Ok(Self(body)),
            _ => Err(ApiError::invalid("expected JSON object")),
        }
    }
}

impl ArchiveBody {
    fn matches_branch(&self, branch: &str) -> Result<(), ApiError> {
        if self.0.get("branch_id").is_some_and(|value| value.as_str() != Some(branch)) {
            return Err(ApiError::invalid("path and body identity disagree"));
        }
        Ok(())
    }
}

fn parse(
    ArchiveBody(mut body): ArchiveBody,
    branch_id: ArchiveBranchTarget,
) -> Result<ArchiveBranch, ApiError> {
    let operation = match body.remove("operation_id") {
        Some(Value::String(value)) if !value.is_empty() => value,
        _ => return Err(ApiError::invalid("missing string field operation_id")),
    };
    if let Some(key) = body.keys().find(|key| key.as_str() != "branch_id") {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let operation_id = operation
        .parse::<storage_contract::OperationId>()
        .map_err(ApiError::contract_input)?;
    Ok(ArchiveBranch {
        branch_id,
        operation_id,
    })
}

pub(crate) async fn archive(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(branch): ApiPath<ArchiveBranchPath>,
    ApiJson(body): ApiJson<ArchiveBody>,
) -> Result<Json<ArchiveResponse>, ApiError> {
    let branch = branch.bind(&body)?;
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            let request = parse(body, branch)?;
            backend
                .archive_branch(&identity, request)
                .map(ArchiveResponse::from)
        })
        .await
        .map(Json)
}
#[cfg(test)]
#[path = "../../../test/server/unit/http_archive.rs"]
mod tests;

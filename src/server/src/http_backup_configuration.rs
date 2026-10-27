//! Backup configuration HTTP shape parsing and response encoding.
use crate::{
    ApiError, AppState,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery},
};
use data_serialization::{Deserialize, Deserializer};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::Value;
use std::collections::BTreeMap;

fn nonempty<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let value = String::deserialize(d)?;
    if value.is_empty() {
        return Err(data_serialization::de::Error::custom(
            "expected nonempty string",
        ));
    }
    Ok(value)
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Fields {
    #[serialization(deserialize_with = "nonempty")]
    repository_id: String,
    #[serialization(default)]
    path: String,
    target_copies: u32,
    backup_prefixes: BTreeMap<String, String>,
    #[serialization(with = "storage_contract::u64_string")]
    expected_generation: u64,
    #[serialization(deserialize_with = "nonempty")]
    expected_scope_id: String,
    #[serialization(deserialize_with = "nonempty")]
    operation_id: String,
}
fn parse(body: Value) -> Result<crate::ConfigureBackups, ApiError> {
    let Fields {
        repository_id,
        path,
        target_copies,
        backup_prefixes,
        expected_generation,
        expected_scope_id,
        operation_id,
    } = json_codec::from_value(body).map_err(ApiError::invalid_source)?;
    if !path.is_empty() {
        name_policy::validate_path(&path).map_err(ApiError::invalid_source)?;
    }
    let operation_id = operation_id
        .parse::<storage_contract::OperationId>()
        .map_err(|source| {
            ApiError::new(
                http::StatusCode::BAD_REQUEST,
                source.code(),
                source.message().into_owned(),
            )
            .with_source(source)
        })?;
    Ok(crate::ConfigureBackups {
        repository_id,
        path,
        target_copies,
        backup_prefixes,
        expected_generation,
        expected_scope_id,
        operation_id,
    })
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository_id): ApiPath<String>,
    ApiQuery(mut query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<crate::backend_backup_configuration::BackupConfigurationResponse>, ApiError> {
    let request = crate::ReadBackupConfiguration {
        repository_id,
        path: query.remove("path").unwrap_or_default(),
    };
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _auth_owner = state.auth;
        state
            .backend
            .backup_configuration(&identity, request)
            .map(crate::backend_backup_configuration::BackupConfigurationResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

pub(crate) async fn configure(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let body = crate::with_id(
        crate::json_body(request).await?,
        "repository_id",
        repository,
    )?;
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _auth_owner = state.auth;
        state
            .backend
            .configure_backups(&identity, parse(body)?)
            .and_then(crate::operation_wire::operation_wire)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_backup_configuration/tests.rs"]
mod tests;

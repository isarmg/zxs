//! Primary placement is distinct from backup-copy configuration.
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

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Query {
    #[serialization(default)]
    path: String,
}
fn nonempty<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        return Err(data_serialization::de::Error::custom(
            "expected nonempty identity",
        ));
    }
    Ok(value)
}
fn rule_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    if value.as_ref().is_some_and(String::is_empty) {
        return Err(data_serialization::de::Error::custom(
            "expected nonempty rule identity or null",
        ));
    }
    Ok(value)
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Fields {
    #[serialization(deserialize_with = "nonempty")]
    repository_id: String,
    path: String,
    #[serialization(deserialize_with = "nonempty")]
    primary_slot_id: String,
    #[serialization(deserialize_with = "nonempty")]
    expected_scope_id: String,
    #[serialization(deserialize_with = "rule_id")]
    expected_rule_id: Option<String>,
    #[serialization(with = "storage_contract::u64_string")]
    expected_generation: u64,
    operation_id: storage_contract::OperationId,
}
fn parse(value: Value) -> Result<crate::ConfigurePlacement, ApiError> {
    let Fields {
        repository_id,
        path,
        primary_slot_id,
        expected_scope_id,
        expected_rule_id,
        expected_generation,
        operation_id,
    } = json_codec::from_value(value).map_err(ApiError::invalid_source)?;
    if !path.is_empty() {
        name_policy::validate_path(&path).map_err(ApiError::invalid_source)?;
    }
    Ok(crate::ConfigurePlacement {
        repository_id,
        path,
        primary_slot_id,
        expected_scope_id,
        expected_rule_id,
        expected_generation,
        operation_id,
    })
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository_id): ApiPath<String>,
    ApiQuery(query): ApiQuery<Query>,
) -> Result<Json<crate::backend_placement_configuration::PlacementConfigurationResponse>, ApiError>
{
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .placement_configuration(
                    &identity,
                    crate::ReadPlacementConfiguration {
                        repository_id,
                        path: query.path,
                    },
                )
                .map(crate::backend_placement_configuration::PlacementConfigurationResponse::from)
        })
        .await
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
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .configure_placement(&identity, parse(body)?)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_placement_configuration.rs"]
mod tests;

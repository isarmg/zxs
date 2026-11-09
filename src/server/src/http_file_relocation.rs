//! Exact original-file request; never creates a disk root or directory repository.
use crate::{ApiError, AppState, MoveDiskFile, auth::Identity, http_boundary::ApiPath};
use data_serialization::Deserialize;
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::Value;

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Body {
    operation_id: storage_contract::OperationId,
    relocation: storage_contract::file_relocation::FileRelocation,
}
pub(crate) async fn submit(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(slot_id): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    name_policy::validate_slot(&slot_id).map_err(ApiError::invalid_source)?;
    let body: Body = json_codec::from_value(crate::json_body(request).await?)
        .map_err(ApiError::invalid_source)?;
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            backend
                .move_disk_file(
                    &identity,
                    MoveDiskFile {
                        slot_id,
                        relocation: body.relocation,
                        operation_id: body.operation_id,
                    },
                )
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}

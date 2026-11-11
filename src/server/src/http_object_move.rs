//! HTTP admission for the shared move command, with one backend publication.
use crate::{ApiError, AppState, MoveDiskObject, auth::Identity, http_boundary::ApiPath};
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
    movement: storage_contract::file_relocation::ObjectMoveRequest,
}

pub(crate) async fn submit(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(source_slot_id): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let source_slot_id =
        name_policy::SlotId::parse(&source_slot_id).map_err(ApiError::invalid_source)?;
    let body: Body = json_codec::from_value(crate::json_body(request).await?)
        .map_err(ApiError::invalid_source)?;
    name_policy::validate_slot(&body.movement.target_slot_id).map_err(ApiError::invalid_source)?;
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .move_disk_object(
                    &identity,
                    MoveDiskObject {
                        source_slot_id,
                        movement: body.movement,
                        operation_id: body.operation_id,
                    },
                )
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}

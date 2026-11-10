//! Authenticated, bounded disk-card reads and absolute note updates.
use crate::{
    ApiError, AppState,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery},
};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use std::collections::BTreeMap;
use storage_contract::disk_details::{DiskDetailsRecord, DiskNoteRequest, DiskNoteResponse};

fn parse(body: json_codec::Value) -> Result<DiskNoteRequest, ApiError> {
    if !body.is_object() {
        return Err(ApiError::invalid("expected disk note object"));
    }
    json_codec::from_value(body).map_err(ApiError::invalid_source)
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(slot): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<DiskDetailsRecord>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.disk_details(&identity, &slot))
        .await
        .map(Json)
}

pub(crate) async fn update_note(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(slot): ApiPath<String>,
    request: Request,
) -> Result<Json<DiskNoteResponse>, ApiError> {
    let request = parse(crate::json_body(request).await?)?;
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.update_disk_note(&identity, &slot, request))
        .await
        .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_disk_details.rs"]
mod tests;

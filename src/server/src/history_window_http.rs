//! History-window HTTP reads retain the existing backend slot through the last
//! response buffer. Static decoding, admission and encoding run in its worker.
use crate::{
    ApiError, AppState,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery},
};
use bytes::Bytes;
use http_routing::{
    Extension,
    body::Body,
    extract::State,
    response::{IntoResponse, Response},
};
use std::collections::BTreeMap;
use storage_contract::history_window::{
    HistoryCollection, HistoryMergeId, HistoryNodeId, HistoryVersionId, HistoryWindowCancellation,
};
use tokio::sync::OwnedSemaphorePermit;

struct OwnedHistoryBytes {
    bytes: Vec<u8>,
    _permit: OwnedSemaphorePermit,
}

struct CancelHistoryWindowOnDrop(HistoryWindowCancellation);

impl Drop for CancelHistoryWindowOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl AsRef<[u8]> for OwnedHistoryBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

fn response(bytes: Bytes) -> Response {
    (
        [(http::header::CONTENT_TYPE, "application/json")],
        Body::from(bytes),
    )
        .into_response()
}

async fn read(
    state: AppState,
    identity: Identity,
    file: String,
    query: BTreeMap<String, String>,
    target: impl FnOnce() -> Result<HistoryCollection, ApiError> + Send + 'static,
) -> Result<Response, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    // The awaiting future owns this guard before the blocking job is enqueued,
    // so cancellation is visible even if the worker has not started capture.
    let cancellation = HistoryWindowCancellation::new();
    let _cancel_on_drop = CancelHistoryWindowOnDrop(cancellation.clone());
    let bytes = crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run_with_permit(move |backend, permit| {
            let request = crate::history_window_query::parse(&file, target()?, query)?;
            let page = backend.history_window(&identity, request, cancellation)?;
            let bytes = crate::history_window_encode::encode(&page)?;
            Ok(Bytes::from_owner(OwnedHistoryBytes {
                bytes,
                _permit: permit,
            }))
        })
        .await?;
    Ok(response(bytes))
}

pub(crate) async fn nodes(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    read(state, identity, file, query, || {
        Ok(HistoryCollection::Nodes)
    })
    .await
}

pub(crate) async fn versions(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    read(state, identity, file, query, || {
        Ok(HistoryCollection::Versions)
    })
    .await
}

pub(crate) async fn branches(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    read(state, identity, file, query, || {
        Ok(HistoryCollection::Branches)
    })
    .await
}

pub(crate) async fn merges(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    read(state, identity, file, query, || {
        Ok(HistoryCollection::Merges)
    })
    .await
}

pub(crate) async fn node_parents(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((file, node)): ApiPath<(String, String)>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    read(state, identity, file, query, move || {
        Ok(HistoryCollection::NodeParents {
            node_id: HistoryNodeId::parse(&node).map_err(ApiError::contract_input)?,
        })
    })
    .await
}

pub(crate) async fn version_sources(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((file, version)): ApiPath<(String, String)>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    read(state, identity, file, query, move || {
        Ok(HistoryCollection::VersionSources {
            version_id: HistoryVersionId::parse(&version).map_err(ApiError::contract_input)?,
        })
    })
    .await
}

pub(crate) async fn merge_inputs(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((file, merge)): ApiPath<(String, String)>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    read(state, identity, file, query, move || {
        Ok(HistoryCollection::MergeInputs {
            merge_id: HistoryMergeId::parse(&merge).map_err(ApiError::contract_input)?,
        })
    })
    .await
}

pub(crate) async fn merge_parents(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((file, merge)): ApiPath<(String, String)>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    read(state, identity, file, query, move || {
        Ok(HistoryCollection::MergeParents {
            merge_id: HistoryMergeId::parse(&merge).map_err(ApiError::contract_input)?,
        })
    })
    .await
}

pub(crate) async fn node_field(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((file, node, field)): ApiPath<(String, String, String)>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    read(state, identity, file, query, move || {
        Ok(HistoryCollection::NodeField {
            node_id: HistoryNodeId::parse(&node).map_err(ApiError::contract_input)?,
            field: crate::history_window_query::field(&field)?,
        })
    })
    .await
}

#[cfg(test)]
#[path = "../../../test/server/unit/history_window_http.rs"]
mod tests;

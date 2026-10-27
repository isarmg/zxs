//! Thin HTTP admission. Lifecycle ownership and credential installation remain
//! in ManagementBackend, so client disconnects cannot release setup ownership.
use crate::{ApiError, AppState, auth::Identity, http_boundary};
use http_routing::{
    Json, Router,
    extract::{Request, State},
    routing::{get, post},
};
use json_codec::{Value, json};

pub(crate) fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/api/setup/status", get(status))
        .route("/api/setup/initialize", post(initialize))
        .route("/api/setup/preset-preview", post(preset_preview))
        .route("/api/setup/initialize-preset", post(initialize_preset))
        .route(
            "/api/setup/complete-initialization",
            post(complete_initialization),
        )
        .route("/api/setup/restore-system", post(bootstrap_restore))
}
pub(crate) fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/api/management/offline-check", post(offline_check))
        .route("/api/management/restore-system", post(restore))
        .route("/api/management/recovery-preview", post(recovery_preview))
        .route("/api/management/recovery-commit", post(recovery_commit))
}
fn header(request: &Request, name: &'static str) -> Result<String, ApiError> {
    if request.headers().get_all(name).iter().count() != 1 {
        return Err(ApiError::unauthorized());
    }
    request
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(ApiError::unauthorized)
}
fn csrf(token: &str) -> String {
    blake3::derive_key("zxs bootstrap csrf v1", token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn bootstrap_request(
    state: &AppState,
    request: &Request,
    changing: bool,
) -> Result<String, ApiError> {
    crate::http_auth::setup_admission(state, request)?;
    if request.uri().query().is_some() {
        return Err(ApiError::invalid(
            "setup credentials must not appear in a URL",
        ));
    }
    if request
        .headers()
        .get("sec-fetch-site")
        .is_some_and(|v| v != "same-origin")
    {
        return Err(ApiError::forbidden());
    }
    crate::http_auth::same_origin(request.headers(), &state.config, changing)?;
    let token = header(request, "x-zxs-bootstrap")?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::unauthorized());
    }
    if changing {
        let supplied = header(request, "x-csrf-token")?;
        let expected = csrf(&token);
        if supplied.len() != expected.len()
            || supplied
                .bytes()
                .zip(expected.bytes())
                .fold(0u8, |n, (a, b)| n | (a ^ b))
                != 0
        {
            return Err(ApiError::forbidden());
        }
    }
    Ok(token)
}
async fn dispatch(
    state: AppState,
    action: &'static str,
    input: Value,
    bootstrap: Option<String>,
    identity: Option<Identity>,
) -> Result<Value, ApiError> {
    let permit = state
        .management_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| {
            ApiError::limit("management operation capacity reached").with_source(source)
        })?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            backend.setup_operation(action, input, bootstrap.as_deref(), identity.as_ref())
        })
        .await
}
async fn body(request: Request) -> Result<Value, ApiError> {
    if !request
        .headers()
        .get(http_routing::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|v| v.trim() == "application/json")
        })
    {
        return Err(ApiError::new(
            ::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            storage_contract::ErrorCode::UnsupportedMediaType,
            "application/json is required",
        ));
    }
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        http_boundary::read_body(request, super::MAX_SETUP_BODY_BYTES),
    )
    .await
    .map_err(|source| {
        ApiError::new(
            ::http::StatusCode::REQUEST_TIMEOUT,
            storage_contract::ErrorCode::RequestTimeout,
            "setup request body timed out",
        )
        .with_source(source)
    })??;
    storage_contract::strict_json(&bytes).map_err(ApiError::invalid_source)
}
async fn status(State(state): State<AppState>, request: Request) -> Result<Json<Value>, ApiError> {
    let token = bootstrap_request(&state, &request, false)?;
    let csrf_token = csrf(&token);
    let mut value = dispatch(state, "status", json!({}), Some(token), None).await?;
    value
        .as_object_mut()
        .ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::InternalError,
                "invalid setup status",
            )
        })?
        .insert("csrf_token".into(), Value::String(csrf_token));
    Ok(Json(value))
}
async fn public_mutation(
    state: AppState,
    request: Request,
    action: &'static str,
) -> Result<Json<Value>, ApiError> {
    let token = bootstrap_request(&state, &request, true)?;
    let input = body(request).await?;
    dispatch(state, action, input, Some(token), None)
        .await
        .map(Json)
}
async fn initialize(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    public_mutation(state, request, "initialize").await
}
async fn preset_preview(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    public_mutation(state, request, "preset_preview").await
}
async fn initialize_preset(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    public_mutation(state, request, "initialize_preset").await
}
async fn complete_initialization(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    public_mutation(state, request, "complete_initialization").await
}
async fn bootstrap_restore(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    public_mutation(state, request, "restore_system").await
}
async fn protected_mutation(
    state: AppState,
    request: Request,
    action: &'static str,
) -> Result<Json<Value>, ApiError> {
    let identity = request
        .extensions()
        .get::<Identity>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    crate::http_auth::recovery_admission(&state, &request)?;
    // The ordinary authorize middleware has checked the session and CSRF.
    let input = body(request).await?;
    dispatch(state, action, input, None, Some(identity))
        .await
        .map(Json)
}
async fn offline_check(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "offline_check").await
}
async fn restore(State(state): State<AppState>, request: Request) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "restore_system").await
}
async fn recovery_preview(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "recovery_preview").await
}
async fn recovery_commit(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "recovery_commit").await
}

#[cfg(test)]
#[path = "../../../../test/server/unit/setup_recovery/http_tests.rs"]
mod tests;

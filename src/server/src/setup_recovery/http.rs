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
        .route("/api/setup/recovery-status", get(recovery_status))
        .route("/api/setup/initialize", post(initialize))
        .route("/api/setup/restore-system", post(bootstrap_restore))
}
pub(crate) fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/storage/status", get(storage_status))
        .route("/api/admin/disks/preview", post(addition_preview))
        .route("/api/admin/disks/addition", get(addition_status))
        .route("/api/admin/disks/addition/resume", post(addition_resume))
        .route("/api/admin/disks/addition/cancel", post(addition_cancel))
        .route("/api/admin/disks/exit/preview", post(exit_preview))
        .route("/api/admin/disks/exit", get(exit_status).post(exit_commit))
        .route("/api/admin/disks/exit/resume", post(exit_resume))
        .route("/api/admin/disks/registrations", get(registration_status))
        .route(
            "/api/admin/disks/readmission/preview",
            post(readmission_preview),
        )
        .route(
            "/api/admin/disks/readmission",
            get(readmission_status).post(readmission_commit),
        )
        .route(
            "/api/admin/disks/readmission/resume",
            post(readmission_resume),
        )
        .route("/api/admin/storage/discovery-root", get(discovery_root))
        .route("/api/admin/storage/discovered-disks", get(discovered_disks))
        .route(
            "/api/admin/storage/foreign/preview",
            post(foreign_disk_preview),
        )
        .route(
            "/api/admin/storage/independent/preview",
            post(independent_disk_preview),
        )
        .route(
            "/api/admin/storage/foreign/plans/preview",
            post(foreign_plan_preview),
        )
        .route(
            "/api/admin/storage/foreign/plans/save",
            post(foreign_plan_save),
        )
        .route(
            "/api/admin/storage/foreign/plans/status",
            get(foreign_plan_status),
        )
        .route(
            "/api/admin/storage/foreign/plans/revalidate",
            post(foreign_plan_revalidate),
        )
        .route(
            "/api/admin/storage/foreign/plans/execute",
            post(foreign_plan_execute),
        )
        .route(
            "/api/admin/storage/foreign/execution-intents/preview",
            post(foreign_intent_preview),
        )
        .route(
            "/api/admin/storage/foreign/execution-intents/save",
            post(foreign_intent_save),
        )
        .route(
            "/api/admin/storage/foreign/execution-intents/status",
            post(foreign_intent_status),
        )
        .route(
            "/api/admin/storage/conversion/preview",
            post(ordinary_conversion_preview),
        )
        .route(
            "/api/admin/storage/conversion/commit",
            post(ordinary_conversion_commit),
        )
        .route(
            "/api/admin/storage/conversion/resume",
            post(ordinary_conversion_resume),
        )
        .route(
            "/api/admin/storage/conversion/stop",
            post(ordinary_conversion_stop),
        )
        .route(
            "/api/admin/storage/conversion/status",
            get(ordinary_conversion_status),
        )
        .route("/api/admin/storage/directories", get(storage_directories))
        .route(
            "/api/admin/storage/directories/inspect",
            post(inspect_storage_directory),
        )
        .route("/api/admin/storage/activate", post(activate))
        .route(
            "/api/admin/storage/complete-activation",
            post(complete_activation),
        )
        .route("/api/management/offline-check", post(offline_check))
        .route("/api/management/restore-system", post(restore))
        .route("/api/management/recovery-preview", post(recovery_preview))
        .route("/api/management/recovery-commit", post(recovery_commit))
}
fn header(request: &Request, name: &'static str) -> Result<String, ApiError> {
    let values: Vec<_> = request
        .headers()
        .get_all(name)
        .iter()
        .map(|v| v.as_bytes())
        .collect();
    authentication::require_single_security_header_value(name, &values)
        .map(str::to_owned)
        .map_err(|source| ApiError::unauthorized().with_source(source))
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
    authentication::require_same_origin_fetch_metadata(&crate::http_auth::header_values(
        request.headers(),
        authentication::SEC_FETCH_SITE_HEADER,
    ))
    .map_err(|source| ApiError::forbidden().with_source(source))?;
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
// Two 4096-byte host paths may each require six JSON bytes per source byte.
const MAX_DIRECTORY_BODY_BYTES: usize = 64 * 1024;
// 32 selections bind four host paths apiece, plus bounded observed identities.
const MAX_ACTIVATION_BODY_BYTES: usize = 4 * 1024 * 1024;

async fn body(request: Request, limit: usize) -> Result<Value, ApiError> {
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
        http_boundary::read_body(request, limit),
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
fn first_account_request(
    state: &AppState,
    request: &Request,
    changing: bool,
) -> Result<(), ApiError> {
    crate::http_auth::setup_admission(state, request)?;
    if request.uri().query().is_some() {
        return Err(ApiError::invalid("first setup accepts no query parameters"));
    }
    authentication::require_same_origin_fetch_metadata(&crate::http_auth::header_values(
        request.headers(),
        authentication::SEC_FETCH_SITE_HEADER,
    ))
    .map_err(|source| ApiError::forbidden().with_source(source))?;
    crate::http_auth::same_origin(request.headers(), &state.config, changing)
}
async fn status(State(state): State<AppState>, request: Request) -> Result<Json<Value>, ApiError> {
    first_account_request(&state, &request, false)?;
    dispatch(state, "status", json!({}), None, None)
        .await
        .map(Json)
}
async fn recovery_status(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let token = bootstrap_request(&state, &request, false)?;
    let csrf_token = csrf(&token);
    let mut value = dispatch(state, "recovery_status", json!({}), Some(token), None).await?;
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
    let input = body(request, super::MAX_SETUP_BODY_BYTES).await?;
    dispatch(state, action, input, Some(token), None)
        .await
        .map(Json)
}
async fn initialize(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    first_account_request(&state, &request, true)?;
    let input = body(request, super::MAX_SETUP_BODY_BYTES).await?;
    dispatch(state, "initialize", input, None, None)
        .await
        .map(Json)
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
    let limit = match action {
        "foreign_plan_preview" | "foreign_plan_save" | "foreign_intent_save" => {
            2 * crate::JSON_BODY_LIMIT
        }
        "activate" => MAX_ACTIVATION_BODY_BYTES,
        "disk_add_preview" => crate::JSON_BODY_LIMIT,
        "disk_exit_commit"
        | "disk_readmit_commit"
        | "ordinary_conversion_preview"
        | "foreign_disk_preview"
        | "independent_disk_preview"
        | "ordinary_conversion_commit" => MAX_DIRECTORY_BODY_BYTES,
        _ => super::MAX_SETUP_BODY_BYTES,
    };
    let input = body(request, limit).await?;
    dispatch(state, action, input, None, Some(identity))
        .await
        .map(Json)
}
async fn storage_status(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let identity = request
        .extensions()
        .get::<Identity>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    if request.uri().query().is_some() {
        return Err(ApiError::invalid(
            "storage status accepts no query parameters",
        ));
    }
    dispatch(state, "storage_status", json!({}), None, Some(identity))
        .await
        .map(Json)
}
async fn discovery_root(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let identity = request
        .extensions()
        .get::<Identity>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    if request.uri().query().is_some() {
        return Err(ApiError::invalid(
            "discovery root accepts no query parameters",
        ));
    }
    dispatch(state, "discovery_root", json!({}), None, Some(identity))
        .await
        .map(Json)
}
async fn discovered_disks(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let identity = request
        .extensions()
        .get::<Identity>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    if request.uri().query().is_some() {
        return Err(ApiError::invalid(
            "disk discovery accepts no query parameters",
        ));
    }
    dispatch(state, "discovered_disks", json!({}), None, Some(identity))
        .await
        .map(Json)
}

async fn foreign_disk_preview(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    if request.uri().query().is_some() {
        return Err(ApiError::invalid(
            "foreign disk preview accepts no query parameters",
        ));
    }
    protected_mutation(state, request, "foreign_disk_preview").await
}
async fn independent_disk_preview(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    if request.uri().query().is_some() {
        return Err(ApiError::invalid(
            "independent disk preview accepts no query parameters",
        ));
    }
    protected_mutation(state, request, "independent_disk_preview").await
}
async fn foreign_plan_mutation(
    state: AppState,
    request: Request,
    action: &'static str,
) -> Result<Json<Value>, ApiError> {
    if request.uri().query().is_some() {
        return Err(ApiError::invalid(
            "foreign plan mutations accept no query parameters",
        ));
    }
    protected_mutation(state, request, action).await
}
async fn foreign_plan_preview(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    foreign_plan_mutation(state, request, "foreign_plan_preview").await
}
async fn foreign_plan_save(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    foreign_plan_mutation(state, request, "foreign_plan_save").await
}
async fn foreign_plan_revalidate(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    foreign_plan_mutation(state, request, "foreign_plan_revalidate").await
}
async fn foreign_plan_execute(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    foreign_plan_mutation(state, request, "foreign_plan_execute").await
}
async fn foreign_intent_preview(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    foreign_plan_mutation(state, request, "foreign_intent_preview").await
}
async fn foreign_intent_save(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    foreign_plan_mutation(state, request, "foreign_intent_save").await
}
async fn foreign_intent_status(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    foreign_plan_mutation(state, request, "foreign_intent_status").await
}
#[derive(data_serialization::Deserialize)]
#[serialization(deny_unknown_fields)]
struct ForeignPlanQuery {
    operation_id: storage_contract::OperationId,
}
async fn foreign_plan_status(
    State(state): State<AppState>,
    crate::http_boundary::ApiQuery(query): crate::http_boundary::ApiQuery<ForeignPlanQuery>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let identity = request
        .extensions()
        .get::<Identity>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    dispatch(
        state,
        "foreign_plan_status",
        json!({"operation_id":query.operation_id}),
        None,
        Some(identity),
    )
    .await
    .map(Json)
}
async fn ordinary_conversion_preview(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "ordinary_conversion_preview").await
}
async fn ordinary_conversion_commit(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "ordinary_conversion_commit").await
}
async fn ordinary_conversion_resume(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "ordinary_conversion_resume").await
}
async fn ordinary_conversion_stop(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "ordinary_conversion_stop").await
}
#[derive(data_serialization::Deserialize)]
#[serialization(deny_unknown_fields)]
struct ConversionStatusQuery {
    operation_id: Option<storage_contract::OperationId>,
}
async fn ordinary_conversion_status(
    State(state): State<AppState>,
    crate::http_boundary::ApiQuery(query): crate::http_boundary::ApiQuery<ConversionStatusQuery>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let identity = request
        .extensions()
        .get::<Identity>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    dispatch(
        state,
        "ordinary_conversion_status",
        json!({"operation_id":query.operation_id}),
        None,
        Some(identity),
    )
    .await
    .map(Json)
}

#[derive(data_serialization::Deserialize)]
#[serialization(deny_unknown_fields)]
struct DirectoryQuery {
    path: String,
}
async fn storage_directories(
    State(state): State<AppState>,
    crate::http_boundary::ApiQuery(query): crate::http_boundary::ApiQuery<DirectoryQuery>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let identity = request
        .extensions()
        .get::<Identity>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    dispatch(
        state,
        "storage_directories",
        json!({"path":query.path}),
        None,
        Some(identity),
    )
    .await
    .map(Json)
}
async fn inspect_storage_directory(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let identity = request
        .extensions()
        .get::<Identity>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    let input = body(request, MAX_DIRECTORY_BODY_BYTES).await?;
    dispatch(
        state,
        "inspect_storage_directory",
        input,
        None,
        Some(identity),
    )
    .await
    .map(Json)
}
async fn activate(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "activate").await
}
async fn complete_activation(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "complete_activation").await
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

async fn addition_preview(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "disk_add_preview").await
}
async fn addition_cancel(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "disk_add_cancel").await
}
async fn addition_resume(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "disk_add_resume").await
}
#[derive(data_serialization::Deserialize)]
#[serialization(deny_unknown_fields)]
struct AdditionQuery {
    operation_id: Option<storage_contract::OperationId>,
}
async fn addition_status(
    State(state): State<AppState>,
    crate::http_boundary::ApiQuery(query): crate::http_boundary::ApiQuery<AdditionQuery>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let identity = request
        .extensions()
        .get::<Identity>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    dispatch(
        state,
        "disk_add_status",
        json!({"operation_id":query.operation_id}),
        None,
        Some(identity),
    )
    .await
    .map(Json)
}

async fn exit_preview(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "disk_exit_preview").await
}
async fn exit_commit(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "disk_exit_commit").await
}
async fn exit_resume(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "disk_exit_resume").await
}
async fn exit_status(
    State(state): State<AppState>,
    crate::http_boundary::ApiQuery(query): crate::http_boundary::ApiQuery<AdditionQuery>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let identity = request
        .extensions()
        .get::<Identity>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    dispatch(
        state,
        "disk_exit_status",
        json!({"operation_id":query.operation_id}),
        None,
        Some(identity),
    )
    .await
    .map(Json)
}

async fn readmission_preview(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "disk_readmit_preview").await
}
async fn readmission_commit(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "disk_readmit_commit").await
}
async fn readmission_resume(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    protected_mutation(state, request, "disk_readmit_resume").await
}
async fn readmission_status(
    State(state): State<AppState>,
    crate::http_boundary::ApiQuery(query): crate::http_boundary::ApiQuery<AdditionQuery>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let identity = request
        .extensions()
        .get::<Identity>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    dispatch(
        state,
        "disk_readmit_status",
        json!({"operation_id":query.operation_id}),
        None,
        Some(identity),
    )
    .await
    .map(Json)
}
async fn registration_status(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let identity = request
        .extensions()
        .get::<Identity>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    if request.uri().query().is_some() {
        return Err(ApiError::invalid(
            "registration metadata accepts no query parameters",
        ));
    }
    dispatch(state, "disk_registrations", json!({}), None, Some(identity))
        .await
        .map(Json)
}

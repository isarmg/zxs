use super::{MAX_REQUEST_BYTES, State, boundary, work};
use data_serialization::{Deserialize, de::DeserializeOwned};
use http_routing::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request, State as ExtractState},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use json_codec::json;
use std::time::Duration;
use tokio::sync::OwnedSemaphorePermit;

#[derive(Debug)]
pub(super) struct Failure {
    status: StatusCode,
    code: &'static str,
    message: String,
}
impl Failure {
    pub(super) fn new(
        status: StatusCode,
        code: &'static str,
        error: impl std::error::Error + 'static,
    ) -> Self {
        Self {
            status,
            code,
            message: crate::diagnostic_chain(&error),
        }
    }
    pub(super) fn message(status: StatusCode, code: &'static str, message: &str) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error": {"code":self.code, "message":self.message}})),
        )
            .into_response()
    }
}

pub(super) fn router(state: State) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/console.css", get(styles))
        .route("/console.js", get(script))
        .route("/api/status", get(status))
        .route("/api/authority/check", post(check))
        .route("/api/authority/export", post(export))
        .route("/api/shutdown", post(shutdown))
        .route("/api/install/payload", get(install_payload))
        .route("/api/install/preview", post(install_preview))
        .route("/api/install/commit", post(install_commit))
        .route("/api/deployment/preview", post(deployment_preview))
        .route("/api/deployment/prepare", post(deployment_prepare))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), security))
        .with_state(state)
}
async fn index() -> Html<&'static str> {
    Html(include_str!("../../web/index.html"))
}
async fn styles() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../../web/console.css"),
    )
}
async fn script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../../web/console.js"),
    )
}
async fn not_found() -> Failure {
    Failure::message(
        StatusCode::NOT_FOUND,
        "not_implemented",
        "This helper operation is not implemented; no service or disk operation was attempted.",
    )
}

async fn security(
    ExtractState(state): ExtractState<State>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = match boundary::host(&state, &request) {
        Ok(()) => next.run(request).await,
        Err(error) => error.into_response(),
    };
    for (name, value) in [
        ("cache-control", "no-store"),
        ("pragma", "no-cache"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
        ("cross-origin-resource-policy", "same-origin"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        ),
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
        ),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}
async fn status(ExtractState(state): ExtractState<State>) -> Json<json_codec::Value> {
    Json(json!({
        "name":"zxs-helper", "version":env!("CARGO_PKG_VERSION"),
        "mode":"program_install_and_independent_check", "authentication":"none",
        "csrf":state.csrf.as_ref(), "stopping":*state.stopping.borrow(),
        "busy":state.jobs.available_permits() == 0,
        "service_exclusion_established":false, "source_writes_available":false, "program_installation":crate::installation::availability(),
        "implemented":["independent_content_check","verified_browser_download","helper_shutdown","embedded_program_installation","inactive_systemd_samba_bundle"],
        "not_implemented":["first_run_setup","disk_add","disk_deregister","foreign_admission","managed_disk_initialization","full_system_rebuild","password_reset","program_repair","active_systemd_smb_installation"]
    }))
}

fn admit_job(state: &State, request: &Request) -> Result<OwnedSemaphorePermit, Failure> {
    boundary::post(state, request)?;
    if *state.stopping.borrow() {
        return Err(Failure::message(
            StatusCode::SERVICE_UNAVAILABLE,
            "helper_stopping",
            "The helper is draining; no new maintenance operation was started.",
        ));
    }
    state
        .jobs
        .clone()
        .try_acquire_owned()
        .map_err(|error| Failure::new(StatusCode::CONFLICT, "helper_busy", error))
}
async fn input<T: DeserializeOwned>(request: Request) -> Result<T, Failure> {
    let bytes = tokio::time::timeout(
        Duration::from_secs(15),
        to_bytes(request.into_body(), MAX_REQUEST_BYTES),
    )
    .await
    .map_err(|error| Failure::new(StatusCode::REQUEST_TIMEOUT, "request_timeout", error))?
    .map_err(|error| Failure::new(StatusCode::BAD_REQUEST, "request_body", error))?;
    json_codec::from_slice(&bytes)
        .map_err(|error| Failure::new(StatusCode::BAD_REQUEST, "request_json", error))
}
async fn check(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: work::Check = input(request).await?;
    work::admit(&input.selection, input.writers_stopped)?;
    let report = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::authority::check::collect(&input.selection).map_err(work::disk_error)
    })
    .await
    .map_err(|error| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "read_worker", error))??;
    Ok(Json(report).into_response())
}
async fn export(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: work::Export = input(request).await?;
    work::admit(&input.selection, input.writers_stopped)?;
    let exported = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        work::export(input)
    })
    .await
    .map_err(|error| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "read_worker", error))??;
    let mut response = Response::new(Body::from(exported.bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    for (name, value) in [
        (
            "content-disposition",
            format!("attachment; filename=\"zxs-{}.bin\"", exported.commit),
        ),
        ("x-zxs-commit", exported.commit.to_string()),
        ("x-zxs-body-digest", exported.body.to_string()),
        (
            "x-zxs-checkpoint",
            exported
                .checkpoint
                .map_or_else(|| "empty".into(), |value| value.to_string()),
        ),
    ] {
        // All dynamic fields are protocol-validated ASCII digests, never paths.
        headers.insert(
            name,
            HeaderValue::from_str(&value).expect("invariant: generated digest header is ASCII"),
        );
    }
    Ok(response)
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Shutdown {
    confirmation: String,
}
async fn shutdown(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    boundary::post(&state, &request)?;
    let input: Shutdown = input(request).await?;
    if input.confirmation != "stop-helper" {
        return Err(Failure::message(
            StatusCode::BAD_REQUEST,
            "shutdown_confirmation",
            "Explicitly confirm stop-helper. This stops only this helper, not ZXS or any disk.",
        ));
    }
    state.stopping.send_replace(true);
    Ok(
        Json(json!({"stopping":true,"wait_for_process_exit":true,"zxs_stopped_by_helper":false}))
            .into_response(),
    )
}

async fn install_payload() -> Json<json_codec::Value> {
    Json(crate::installation::availability())
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct InstallPreviewRequest {}
async fn install_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let _: InstallPreviewRequest = input(request).await?;
    let preview = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::installation::preview().map_err(install_failure)
    })
    .await
    .map_err(|error| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "install_worker", error))??;
    Ok(Json(preview).into_response())
}
async fn install_commit(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::installation::Commit = input(request).await?;
    let receipt = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::installation::commit(input).map_err(install_failure)
    })
    .await
    .map_err(|error| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "install_worker", error))??;
    Ok(Json(receipt).into_response())
}
fn install_failure(error: Box<dyn std::error::Error + Send + Sync>) -> Failure {
    Failure::message(
        StatusCode::CONFLICT,
        "program_installation",
        &crate::diagnostic_chain(error.as_ref()),
    )
}

async fn deployment_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::deployment::Input = input(request).await?;
    let preview = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::deployment::preview(input).map_err(deployment_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "deployment_worker",
            error,
        )
    })??;
    Ok(Json(preview).into_response())
}
async fn deployment_prepare(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::deployment::Prepare = input(request).await?;
    let receipt = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::deployment::prepare(input).map_err(deployment_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "deployment_worker",
            error,
        )
    })??;
    Ok(Json(receipt).into_response())
}
fn deployment_failure(error: Box<dyn std::error::Error + Send + Sync>) -> Failure {
    Failure::message(
        StatusCode::CONFLICT,
        "inactive_deployment_preparation",
        &crate::diagnostic_chain(error.as_ref()),
    )
}

use super::{MAX_REQUEST_BYTES, State, boundary, work};
mod configuration_recovery;
mod materials;
mod response_body;
use data_serialization::{Deserialize, de::DeserializeOwned};
use http_routing::{
    Json, Router,
    body::to_bytes,
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
        .route(
            "/api/initialization/storage-preview",
            post(first_storage_preview),
        )
        .route(
            "/api/initialization/storage-recover-preview",
            post(first_storage_recover),
        )
        .route(
            "/api/initialization/storage-create",
            post(first_storage_create),
        )
        .route("/api/disks/registrations", post(disk_removal_inventory))
        .route(
            "/api/disks/terminal-chain-export",
            post(terminal_chain_export),
        )
        .route(
            "/api/disks/terminal-chain-readmission-proposal",
            post(terminal_chain_readmission_proposal),
        )
        .route(
            "/api/disks/terminal-chain-target-status",
            post(terminal_chain_target_status),
        )
        .route(
            "/api/disks/terminal-chain-target-synchronize",
            post(terminal_chain_target_synchronize),
        )
        .route(
            "/api/disks/terminal-chain-first-prepare",
            post(terminal_chain_first_prepare),
        )
        .route(
            "/api/disks/terminal-chain-first-publish",
            post(terminal_chain_first_publish),
        )
        .route(
            "/api/disks/terminal-chain-current",
            post(terminal_chain_current),
        )
        .route(
            "/api/disks/terminal-chain-activation-plan",
            post(terminal_chain_activation_plan),
        )
        .route(
            "/api/disks/terminal-chain-io-decide",
            post(terminal_chain_io_decide),
        )
        .route(
            "/api/disks/terminal-chain-generation-log",
            post(terminal_chain_generation_log),
        )
        .route(
            "/api/disks/terminal-chain-generation-select",
            post(terminal_chain_generation_select),
        )
        .route(
            "/api/disks/terminal-chain-generation-read",
            post(terminal_chain_generation_read),
        )
        .route("/api/disks/deregister-preview", post(disk_removal_preview))
        .route(
            "/api/disks/deregister-original",
            post(disk_removal_original),
        )
        .route("/api/disks/deregister-commit", post(disk_removal_commit))
        .route(
            "/api/disks/terminal-removal-inspect",
            post(terminal_removal_inspect),
        )
        .route(
            "/api/disks/terminal-retirement-preview",
            post(terminal_retirement_preview),
        )
        .route(
            "/api/disks/terminal-retirement-original",
            post(terminal_retirement_original),
        )
        .route(
            "/api/disks/terminal-retirement-commit",
            post(terminal_retirement_commit),
        )
        .route(
            "/api/disks/readmission-preview",
            post(disk_readmission_preview),
        )
        .route(
            "/api/disks/readmission-original",
            post(disk_readmission_original),
        )
        .route(
            "/api/disks/readmission-commit",
            post(disk_readmission_commit),
        )
        .route("/api/disks/add-preview", post(disk_addition_preview))
        .route("/api/disks/add-original", post(disk_addition_original))
        .route("/api/disks/add-commit", post(disk_addition_commit))
        .route(
            "/api/disks/foreign-chain-preview",
            post(foreign_chain_preview),
        )
        .route(
            "/api/disks/foreign-chain-original",
            post(foreign_chain_original),
        )
        .route(
            "/api/disks/foreign-chain-prepare",
            post(foreign_chain_prepare),
        )
        .route(
            "/api/disks/foreign-chain-databases-preview",
            post(foreign_chain_databases_preview),
        )
        .route(
            "/api/disks/foreign-chain-databases-prepare",
            post(foreign_chain_databases_prepare),
        )
        .route(
            "/api/disks/foreign-chain-identity-preview",
            post(foreign_chain_identity_preview),
        )
        .route(
            "/api/disks/foreign-chain-objects-preview",
            post(foreign_chain_objects_preview),
        )
        .route(
            "/api/disks/foreign-chain-objects-prepare",
            post(foreign_chain_objects_prepare),
        )
        .route("/api/authority/check", post(check))
        .route("/api/authority/export", post(export))
        .route("/api/shutdown", post(shutdown))
        .route("/api/install/payload", get(install_payload))
        .route("/api/install/materials-download", post(materials::archive))
        .route("/api/install/materials-manifest", post(materials::manifest))
        .route("/api/install/preview", post(install_preview))
        .route("/api/install/commit", post(install_commit))
        .route("/api/install/repair-preview", post(repair_preview))
        .route(
            "/api/install/repair-recover-preview",
            post(repair_recover_preview),
        )
        .route("/api/install/repair-commit", post(repair_commit))
        .route("/api/deployment/preview", post(deployment_preview))
        .route("/api/deployment/prepare", post(deployment_prepare))
        .route(
            "/api/deployment/install-preview",
            post(configuration_preview),
        )
        .route("/api/deployment/install-files", post(configuration_install))
        .route(
            "/api/deployment/install-recover-preview",
            post(configuration_recovery::preview),
        )
        .route(
            "/api/deployment/install-recover-files",
            post(configuration_recovery::commit),
        )
        .route(
            "/api/credentials/reset-preview",
            post(administrator_reset_preview),
        )
        .route("/api/credentials/reset", post(administrator_reset))
        .route(
            "/api/credentials/reset-recover-preview",
            post(administrator_reset_recover_preview),
        )
        .route("/api/credentials/samba-preview", post(samba_reset_preview))
        .route("/api/credentials/samba-reset", post(samba_reset))
        .route("/api/credentials/samba-status", post(samba_reset_status))
        .route("/api/host/preview", post(host_preview))
        .route("/api/host/prepare", post(host_prepare))
        .route("/api/initialization/layout-preview", post(layout_preview))
        .route("/api/initialization/layout-prepare", post(layout_prepare))
        .route("/api/initialization/runtime-preview", post(runtime_preview))
        .route("/api/initialization/runtime-prepare", post(runtime_prepare))
        .route(
            "/api/initialization/administrator-preview",
            post(first_administrator_preview),
        )
        .route(
            "/api/initialization/administrator-create",
            post(first_administrator_create),
        )
        .route(
            "/api/initialization/administrator-recover-preview",
            post(first_administrator_recover_preview),
        )
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
        "service_exclusion_established":false, "participating_zxs_lease_root":platform_io::service_lease::HOST_ROOT, "source_writes_available":false, "first_storage_initialization":"confirmed_empty_existing_roots_only", "program_installation":crate::installation::availability(),
        "implemented":["independent_content_check","verified_browser_download","helper_shutdown","embedded_program_installation","embedded_license_material_download","same_payload_program_repair_with_retained_original","inactive_systemd_samba_bundle","host_lease_root_preparation","empty_service_private_layout","empty_service_runtime_and_protected_export","fixed_configuration_file_installation","original_configuration_installation_recovery","existing_administrator_password_reset","first_management_administrator_initialization","first_empty_storage_batch","existing_local_tdbsam_password_reset","active_empty_directory_addition","foreign_complete_lower_chain_material","foreign_original_state_and_three_scope_database_material","foreign_complete_chain_identity_proposal","foreign_complete_object_identity_material","nonterminal_disk_deregistration","final_voter_terminal_deregistration","terminal_remaining_registration_inspection","sequential_terminal_registration_retirement","retained_terminal_metadata_readmission","completed_terminal_chain_checkpoint_export","terminal_chain_retained_readmission_proposal","terminal_chain_target_decision_preparation","terminal_chain_first_native_inputs","terminal_chain_actual_first_publication","terminal_chain_published_current_read","terminal_chain_exact_epoch_plan","terminal_chain_actual_io_decision","terminal_chain_generation_log_and_frozen_current","terminal_chain_exact_generation_selector_read"],
        "not_implemented":["complete_first_run_setup","general_disk_add","metadata_readmission_after_terminal_retirement","foreign_admission","device_formatting_and_mounting","full_system_rebuild","other_samba_backends_and_account_creation","program_repair","systemd_smb_activation"]
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
    let response = tokio::task::spawn_blocking(move || {
        let report = crate::authority::check::collect(&input.selection).map_err(work::disk_error)?;
        let bytes = json_codec::to_vec(&report).map_err(|error| {
            Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "report_json", error)
        })?;
        Ok::<_, Failure>((
            [(header::CONTENT_TYPE, "application/json")],
            response_body::owned(bytes.into(), permit),
        ).into_response())
    })
    .await
    .map_err(|error| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "read_worker", error))??;
    Ok(response)
}
async fn export(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: work::Export = input(request).await?;
    work::admit(&input.selection, input.writers_stopped)?;
    let (exported, permit) = tokio::task::spawn_blocking(move || {
        let exported = work::export(input)?;
        Ok::<_, Failure>((exported, permit))
    })
    .await
    .map_err(|error| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "read_worker", error))??;
    let mut response = Response::new(response_body::owned(exported.bytes.into(), permit));
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

async fn repair_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let _: InstallPreviewRequest = input(request).await?;
    let preview = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::installation::repair::preview().map_err(repair_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "program_repair_worker",
            error,
        )
    })??;
    Ok(Json(preview).into_response())
}
async fn repair_recover_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::installation::repair::Original = input(request).await?;
    let preview = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::installation::repair::recover_preview(input).map_err(repair_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "program_repair_worker",
            error,
        )
    })??;
    Ok(Json(preview).into_response())
}
async fn repair_commit(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::installation::repair::Commit = input(request).await?;
    let receipt = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::installation::repair::commit(input).map_err(repair_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "program_repair_worker",
            error,
        )
    })??;
    Ok(Json(receipt).into_response())
}
fn repair_failure(error: Box<dyn std::error::Error + Send + Sync>) -> Failure {
    Failure::message(
        StatusCode::CONFLICT,
        "same_payload_program_repair",
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

async fn host_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let _: InstallPreviewRequest = input(request).await?;
    let preview = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::host::preview().map_err(host_failure)
    })
    .await
    .map_err(|error| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "host_worker", error))??;
    Ok(Json(preview).into_response())
}
async fn host_prepare(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::host::Prepare = input(request).await?;
    let receipt = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::host::prepare(input).map_err(host_failure)
    })
    .await
    .map_err(|error| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "host_worker", error))??;
    Ok(Json(receipt).into_response())
}
fn host_failure(error: Box<dyn std::error::Error + Send + Sync>) -> Failure {
    Failure::message(
        StatusCode::CONFLICT,
        "host_root_preparation",
        &crate::diagnostic_chain(error.as_ref()),
    )
}

async fn layout_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::initialization::Input = input(request).await?;
    let preview = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::initialization::preview(input).map_err(layout_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "initialization_worker",
            error,
        )
    })??;
    Ok(Json(preview).into_response())
}
async fn layout_prepare(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::initialization::Prepare = input(request).await?;
    let receipt = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::initialization::prepare(input).map_err(layout_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "initialization_worker",
            error,
        )
    })??;
    Ok(Json(receipt).into_response())
}
fn layout_failure(error: Box<dyn std::error::Error + Send + Sync>) -> Failure {
    Failure::message(
        StatusCode::CONFLICT,
        "private_layout_preparation",
        &crate::diagnostic_chain(error.as_ref()),
    )
}

async fn runtime_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::initialization::Input = input(request).await?;
    let preview = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::initialization::runtime::preview(input).map_err(runtime_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_directory_worker",
            error,
        )
    })??;
    Ok(Json(preview).into_response())
}

async fn runtime_prepare(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::initialization::runtime::Prepare = input(request).await?;
    let receipt = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::initialization::runtime::prepare(input).map_err(runtime_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_directory_worker",
            error,
        )
    })??;
    Ok(Json(receipt).into_response())
}

fn runtime_failure(error: Box<dyn std::error::Error + Send + Sync>) -> Failure {
    Failure::message(
        StatusCode::CONFLICT,
        "empty_runtime_preparation",
        &crate::diagnostic_chain(error.as_ref()),
    )
}

async fn configuration_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::deployment::Input = input(request).await?;
    let preview = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::deployment::install::preview(input).map_err(configuration_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "configuration_worker",
            error,
        )
    })??;
    Ok(Json(preview).into_response())
}
async fn configuration_install(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::deployment::install::Commit = input(request).await?;
    let receipt = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::deployment::install::commit(input).map_err(configuration_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "configuration_worker",
            error,
        )
    })??;
    Ok(Json(receipt).into_response())
}
fn configuration_failure(error: Box<dyn std::error::Error + Send + Sync>) -> Failure {
    Failure::message(
        StatusCode::CONFLICT,
        "configuration_file_installation",
        &crate::diagnostic_chain(error.as_ref()),
    )
}

async fn administrator_reset_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::AdministratorResetPreview,
    )
    .await
}
async fn administrator_reset(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::AdministratorReset,
    )
    .await
}

async fn administrator_reset_recover_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::AdministratorResetRecoverPreview,
    )
    .await
}

async fn secret_input<T: DeserializeOwned>(request: Request) -> Result<T, Failure> {
    let bytes = tokio::time::timeout(
        Duration::from_secs(15),
        to_bytes(request.into_body(), MAX_REQUEST_BYTES),
    )
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::REQUEST_TIMEOUT,
            "credential_request_timeout",
            error,
        )
    })?
    .map_err(|error| Failure::new(StatusCode::BAD_REQUEST, "credential_request_body", error))?;
    let decoded = json_codec::from_slice::<T>(&bytes);
    // Clear the transport allocation when uniquely owned, while admitted
    // SecretString fields clear on drop. No claim covers all library copies.
    if let Ok(mut owned) = bytes.try_into_mut() {
        owned.fill(0);
    }
    decoded.map_err(|_| {
        Failure::message(
            StatusCode::BAD_REQUEST,
            "credential_request_json",
            "Invalid credential request; password values and parser details are not returned.",
        )
    })
}
async fn first_administrator_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::FirstAdministratorPreview,
    )
    .await
}
async fn first_administrator_recover_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::FirstAdministratorRecoverPreview,
    )
    .await
}
async fn first_administrator_create(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::FirstAdministratorCreate,
    )
    .await
}

async fn first_storage_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::FirstStoragePreview,
    )
    .await
}
async fn first_storage_recover(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::FirstStorageRecover,
    )
    .await
}
async fn first_storage_create(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::FirstStorageCreate,
    )
    .await
}

async fn samba_reset_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::deployment::samba_password::Input = input(request).await?;
    let preview = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::deployment::samba_password::preview(input).map_err(samba_reset_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "samba_reset_worker",
            error,
        )
    })??;
    Ok(Json(preview).into_response())
}
async fn samba_reset(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::deployment::samba_password::Reset = secret_input(request).await?;
    let receipt = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::deployment::samba_password::reset(input).map_err(samba_reset_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "samba_reset_worker",
            error,
        )
    })??;
    Ok(Json(receipt).into_response())
}
async fn samba_reset_status(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: crate::deployment::samba_password::Original = input(request).await?;
    let status = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::deployment::samba_password::status(input).map_err(samba_reset_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "samba_reset_worker",
            error,
        )
    })??;
    Ok(Json(status).into_response())
}
fn samba_reset_failure(error: Box<dyn std::error::Error + Send + Sync>) -> Failure {
    // Never format command output, secret input, a verifier, or a source chain.
    Failure::message(
        StatusCode::CONFLICT,
        "local_samba_password_reset",
        &error.to_string(),
    )
}

async fn foreign_chain_objects_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::ForeignChainObjectsPreview,
    )
    .await
}
async fn foreign_chain_objects_prepare(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::ForeignChainObjectsPrepare,
    )
    .await
}

async fn foreign_chain_identity_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::ForeignChainIdentityPreview,
    )
    .await
}
async fn foreign_chain_databases_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::ForeignChainDatabasesPreview,
    )
    .await
}
async fn foreign_chain_databases_prepare(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::ForeignChainDatabasesPrepare,
    )
    .await
}

async fn foreign_chain_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::ForeignChainPreview,
    )
    .await
}
async fn foreign_chain_original(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::ForeignChainOriginal,
    )
    .await
}
async fn foreign_chain_prepare(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::ForeignChainPrepare,
    )
    .await
}

async fn disk_addition_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::DiskAdditionPreview,
    )
    .await
}

async fn disk_addition_original(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::DiskAdditionOriginal,
    )
    .await
}

async fn disk_addition_commit(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::DiskAdditionCommit,
    )
    .await
}

async fn disk_removal_inventory(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::DiskRemovalInventory,
    )
    .await
}

async fn disk_removal_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::DiskRemovalPreview,
    )
    .await
}

async fn disk_removal_original(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::DiskRemovalOriginal,
    )
    .await
}

async fn disk_removal_commit(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::DiskRemovalCommit,
    )
    .await
}

async fn terminal_removal_inspect(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalRemovalInspect,
    )
    .await
}

async fn terminal_retirement_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalRetirementPreview,
    )
    .await
}

async fn terminal_retirement_original(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalRetirementOriginal,
    )
    .await
}

async fn terminal_retirement_commit(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalRetirementCommit,
    )
    .await
}

async fn disk_readmission_preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::DiskReadmissionPreview,
    )
    .await
}

async fn disk_readmission_original(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::DiskReadmissionOriginal,
    )
    .await
}

async fn disk_readmission_commit(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::DiskReadmissionCommit,
    )
    .await
}

async fn terminal_chain_export(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalChainExport,
    )
    .await
}

async fn terminal_chain_readmission_proposal(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalChainReadmissionProposal,
    )
    .await
}

async fn terminal_chain_target_status(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalChainTargetStatus,
    )
    .await
}
async fn terminal_chain_target_synchronize(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalChainTargetSynchronize,
    )
    .await
}

async fn terminal_chain_first_prepare(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalChainFirstPrepare,
    )
    .await
}

async fn terminal_chain_first_publish(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalChainFirstPublish,
    )
    .await
}

async fn terminal_chain_current(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalChainCurrent,
    )
    .await
}

async fn terminal_chain_activation_plan(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalChainActivationPlan,
    )
    .await
}

async fn terminal_chain_io_decide(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalChainIoDecide,
    )
    .await
}

async fn terminal_chain_generation_log(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalChainGenerationLog,
    )
    .await
}

async fn terminal_chain_generation_select(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalChainGenerationSelect,
    )
    .await
}

async fn terminal_chain_generation_read(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    maintenance_operation(
        state,
        request,
        storage_engine::offline_maintenance::Operation::TerminalChainGenerationRead,
    )
    .await
}

/// The helper is only the HTTP transport for management semantics. The matching
/// embedded program parses the exact typed request before any host access.
async fn maintenance_operation(
    state: State,
    request: Request,
    operation: storage_engine::offline_maintenance::Operation,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let bytes = tokio::time::timeout(
        Duration::from_secs(15),
        to_bytes(request.into_body(), MAX_REQUEST_BYTES),
    )
    .await
    .map_err(|error| Failure::new(StatusCode::REQUEST_TIMEOUT, "request_timeout", error))?
    .map_err(|error| Failure::new(StatusCode::BAD_REQUEST, "request_body", error))?;
    let body = platform_io::secret::SecretBytes::new(bytes.to_vec());
    if let Ok(mut unique) = bytes.try_into_mut() {
        unique.fill(0);
    }
    let (result, permit) = tokio::task::spawn_blocking(move || {
        let result = crate::maintenance::run(operation, body)?;
        Ok::<_, crate::maintenance::Error>((result, permit))
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "maintenance_worker",
            error,
        )
    })?
    .map_err(|error| Failure::message(error.status(), error.code(operation), &error.to_string()))?;
    Ok((
        [(header::CONTENT_TYPE, "application/json")],
        response_body::owned(result.into(), permit),
    ).into_response())
}

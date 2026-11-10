use super::{MAX_REQUEST_BYTES, State, boundary, work};
mod activation;
mod job;
mod materials;
mod packages;
mod response_body;
mod samba_enrollment;
mod service_account;
use data_serialization::{Deserialize, de::DeserializeOwned};
use http_routing::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State as ExtractState},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{MethodRouter, get, post},
};
use job::{Empty, Job};
use json_codec::json;
use std::time::Duration;
use storage_engine::offline_maintenance::Operation;
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
    let program = Job::new("program_installation", "install_worker");
    let repair = Job::new("same_payload_program_repair", "program_repair_worker");
    let deployment = Job::new("inactive_deployment_preparation", "deployment_worker");
    let host = Job::new("host_root_preparation", "host_worker");
    let layout = Job::new("private_layout_preparation", "initialization_worker");
    let runtime = Job::new("empty_runtime_preparation", "runtime_directory_worker");
    let configuration = Job::new("configuration_file_installation", "configuration_worker");
    let configuration_recovery = Job::new(
        "configuration_file_installation",
        "configuration_recovery_worker",
    );
    let service_account = Job::new("local_service_account", "account_worker");
    let packages = Job::new("system_packages", "packages_worker");
    let vfs = Job::new("sealed_vfs_installation", "vfs_installation_worker");
    let activation = Job::new("service_activation", "activation_worker");
    Router::new()
        .route("/", get(index))
        .route("/console.css", get(styles))
        .route("/console.js", get(script))
        .route("/service-account.js", get(service_account::script))
        .route(
            "/api/initialization/account-preview",
            service_account.route(crate::initialization::account::preview),
        )
        .route(
            "/api/initialization/account-prepare",
            service_account.route(crate::initialization::account::prepare),
        )
        .route("/first-install.js", get(first_install_script))
        .route("/vfs-install.js", get(vfs_install_script))
        .route("/packages.js", get(packages::script))
        .route(
            "/api/initialization/packages-preview",
            packages.route(crate::initialization::packages::preview),
        )
        .route(
            "/api/initialization/packages-refresh",
            packages.route(crate::initialization::packages::refresh),
        )
        .route(
            "/api/initialization/packages-install",
            packages.route(crate::initialization::packages::install),
        )
        .route(
            "/api/initialization/packages-finish",
            packages.route(crate::initialization::packages::finish),
        )
        .route("/api/status", get(status))
        .route(
            "/api/initialization/storage-directories-preview",
            maintenance(Operation::FirstStorageDirectoriesPreview),
        )
        .route(
            "/api/initialization/storage-directories-prepare",
            maintenance(Operation::FirstStorageDirectoriesPrepare),
        )
        .route(
            "/api/initialization/storage-preview",
            maintenance(Operation::FirstStoragePreview),
        )
        .route(
            "/api/initialization/storage-recover-preview",
            maintenance(Operation::FirstStorageRecover),
        )
        .route(
            "/api/initialization/storage-create",
            maintenance(Operation::FirstStorageCreate),
        )
        .route(
            "/api/disks/registrations",
            maintenance(Operation::DiskRemovalInventory),
        )
        .route(
            "/api/disks/terminal-chain-export",
            maintenance(Operation::TerminalChainExport),
        )
        .route(
            "/api/disks/terminal-chain-readmission-proposal",
            maintenance(Operation::TerminalChainReadmissionProposal),
        )
        .route(
            "/api/disks/terminal-chain-target-status",
            maintenance(Operation::TerminalChainTargetStatus),
        )
        .route(
            "/api/disks/terminal-chain-target-synchronize",
            maintenance(Operation::TerminalChainTargetSynchronize),
        )
        .route(
            "/api/disks/terminal-chain-first-prepare",
            maintenance(Operation::TerminalChainFirstPrepare),
        )
        .route(
            "/api/disks/terminal-chain-first-publish",
            maintenance(Operation::TerminalChainFirstPublish),
        )
        .route(
            "/api/disks/terminal-chain-current",
            maintenance(Operation::TerminalChainCurrent),
        )
        .route(
            "/api/disks/terminal-chain-activation-plan",
            maintenance(Operation::TerminalChainActivationPlan),
        )
        .route(
            "/api/disks/terminal-chain-io-decide",
            maintenance(Operation::TerminalChainIoDecide),
        )
        .route(
            "/api/disks/terminal-chain-generation-log",
            maintenance(Operation::TerminalChainGenerationLog),
        )
        .route(
            "/api/disks/terminal-chain-generation-select",
            maintenance(Operation::TerminalChainGenerationSelect),
        )
        .route(
            "/api/disks/terminal-chain-generation-read",
            maintenance(Operation::TerminalChainGenerationRead),
        )
        .route(
            "/api/disks/deregister-preview",
            maintenance(Operation::DiskRemovalPreview),
        )
        .route(
            "/api/disks/deregister-original",
            maintenance(Operation::DiskRemovalOriginal),
        )
        .route(
            "/api/disks/deregister-commit",
            maintenance(Operation::DiskRemovalCommit),
        )
        .route(
            "/api/disks/terminal-removal-inspect",
            maintenance(Operation::TerminalRemovalInspect),
        )
        .route(
            "/api/disks/terminal-retirement-preview",
            maintenance(Operation::TerminalRetirementPreview),
        )
        .route(
            "/api/disks/terminal-retirement-original",
            maintenance(Operation::TerminalRetirementOriginal),
        )
        .route(
            "/api/disks/terminal-retirement-commit",
            maintenance(Operation::TerminalRetirementCommit),
        )
        .route(
            "/api/disks/readmission-preview",
            maintenance(Operation::DiskReadmissionPreview),
        )
        .route(
            "/api/disks/readmission-original",
            maintenance(Operation::DiskReadmissionOriginal),
        )
        .route(
            "/api/disks/readmission-commit",
            maintenance(Operation::DiskReadmissionCommit),
        )
        .route(
            "/api/disks/add-preview",
            maintenance(Operation::DiskAdditionPreview),
        )
        .route(
            "/api/disks/add-original",
            maintenance(Operation::DiskAdditionOriginal),
        )
        .route(
            "/api/disks/add-commit",
            maintenance(Operation::DiskAdditionCommit),
        )
        .route(
            "/api/disks/foreign-chain-preview",
            maintenance(Operation::ForeignChainPreview),
        )
        .route(
            "/api/disks/foreign-chain-original",
            maintenance(Operation::ForeignChainOriginal),
        )
        .route(
            "/api/disks/foreign-chain-prepare",
            maintenance(Operation::ForeignChainPrepare),
        )
        .route(
            "/api/disks/foreign-chain-databases-preview",
            maintenance(Operation::ForeignChainDatabasesPreview),
        )
        .route(
            "/api/disks/foreign-chain-databases-prepare",
            maintenance(Operation::ForeignChainDatabasesPrepare),
        )
        .route(
            "/api/disks/foreign-chain-identity-preview",
            maintenance(Operation::ForeignChainIdentityPreview),
        )
        .route(
            "/api/disks/foreign-chain-objects-preview",
            maintenance(Operation::ForeignChainObjectsPreview),
        )
        .route(
            "/api/disks/foreign-chain-objects-prepare",
            maintenance(Operation::ForeignChainObjectsPrepare),
        )
        .route("/api/authority/check", post(check))
        .route("/api/authority/export", post(export))
        .route("/api/shutdown", post(shutdown))
        .route("/api/install/payload", get(install_payload))
        .route("/api/install/materials-download", post(materials::archive))
        .route("/api/install/materials-manifest", post(materials::manifest))
        .route(
            "/api/install/preview",
            program.route(|_: Empty| crate::installation::preview()),
        )
        .route(
            "/api/install/commit",
            program.route(crate::installation::commit),
        )
        .route(
            "/api/install/vfs-preview",
            vfs.route(|_: Empty| crate::installation::vfs::preview()),
        )
        .route(
            "/api/install/vfs-status",
            vfs.route(crate::installation::vfs::status),
        )
        .route(
            "/api/install/vfs-install",
            vfs.route(crate::installation::vfs::commit),
        )
        .route(
            "/api/install/repair-preview",
            repair.route(|_: Empty| crate::installation::repair::preview()),
        )
        .route(
            "/api/install/repair-recover-preview",
            repair.route(crate::installation::repair::recover_preview),
        )
        .route(
            "/api/install/repair-commit",
            repair.route(crate::installation::repair::commit),
        )
        .route(
            "/api/deployment/activation-preview",
            activation.route(crate::deployment::activation::preview),
        )
        .route("/api/deployment/activate", post(activation::activate))
        .route(
            "/api/deployment/activation-status",
            activation.route(crate::deployment::activation::status),
        )
        .route(
            "/api/deployment/preview",
            deployment.route(crate::deployment::preview),
        )
        .route(
            "/api/deployment/prepare",
            deployment.route(crate::deployment::prepare),
        )
        .route(
            "/api/deployment/install-preview",
            configuration.route(crate::deployment::install::preview),
        )
        .route(
            "/api/deployment/install-files",
            configuration.route(crate::deployment::install::commit),
        )
        .route(
            "/api/deployment/install-recover-preview",
            configuration_recovery.route(crate::deployment::install::recovery::preview),
        )
        .route(
            "/api/deployment/install-recover-files",
            configuration_recovery.route(crate::deployment::install::recovery::commit),
        )
        .route(
            "/api/credentials/reset-preview",
            maintenance(Operation::AdministratorResetPreview),
        )
        .route(
            "/api/credentials/reset",
            maintenance(Operation::AdministratorReset),
        )
        .route(
            "/api/credentials/reset-recover-preview",
            maintenance(Operation::AdministratorResetRecoverPreview),
        )
        .route("/api/credentials/samba-preview", post(samba_reset_preview))
        .route("/api/credentials/samba-reset", post(samba_reset))
        .route("/api/credentials/samba-status", post(samba_reset_status))
        .route(
            "/api/credentials/samba-account-preview",
            post(samba_enrollment::preview),
        )
        .route(
            "/api/credentials/samba-account-create",
            post(samba_enrollment::create),
        )
        .route(
            "/api/credentials/samba-account-status",
            post(samba_enrollment::status),
        )
        .route(
            "/api/host/preview",
            host.route(|_: Empty| crate::host::preview()),
        )
        .route("/api/host/prepare", host.route(crate::host::prepare))
        .route(
            "/api/initialization/layout-preview",
            layout.route(crate::initialization::preview),
        )
        .route(
            "/api/initialization/layout-prepare",
            layout.route(crate::initialization::prepare),
        )
        .route(
            "/api/initialization/runtime-preview",
            runtime.route(crate::initialization::runtime::preview),
        )
        .route(
            "/api/initialization/runtime-prepare",
            runtime.route(crate::initialization::runtime::prepare),
        )
        .route(
            "/api/initialization/administrator-preview",
            maintenance(Operation::FirstAdministratorPreview),
        )
        .route(
            "/api/initialization/administrator-create",
            maintenance(Operation::FirstAdministratorCreate),
        )
        .route(
            "/api/initialization/administrator-recover-preview",
            maintenance(Operation::FirstAdministratorRecoverPreview),
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
async fn first_install_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../../web/first_install.js"),
    )
}
async fn vfs_install_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../../web/vfs_install.js"),
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
        "service_exclusion_established":true, "participating_zxs_lease_root":platform_io::service_lease::HOST_ROOT, "source_writes_available":false, "first_storage_initialization":"confirmed_empty_existing_roots_only", "program_installation":crate::installation::availability(),
        "implemented":["independent_content_check","verified_browser_download","helper_shutdown","embedded_program_installation","sealed_same_source_vfs_installation","embedded_license_material_download","same_payload_program_repair_with_retained_original","inactive_systemd_samba_bundle","host_lease_root_preparation","local_service_account_preparation","separate_samba_unix_account_preparation","debian_13_samba_dependency_installation","empty_service_private_layout","empty_service_runtime_and_protected_export","fixed_configuration_file_installation","explicit_installed_service_activation","original_configuration_installation_recovery","existing_administrator_password_reset","first_management_administrator_initialization","first_empty_storage_batch","existing_local_tdbsam_password_reset","missing_local_tdbsam_account_enrollment","active_empty_directory_addition","foreign_complete_lower_chain_material","foreign_original_state_and_three_scope_database_material","foreign_complete_chain_identity_proposal","foreign_complete_object_identity_material","nonterminal_disk_deregistration","final_voter_terminal_deregistration","terminal_remaining_registration_inspection","sequential_terminal_registration_retirement","retained_terminal_metadata_readmission","completed_terminal_chain_checkpoint_export","terminal_chain_retained_readmission_proposal","terminal_chain_target_decision_preparation","terminal_chain_first_native_inputs","terminal_chain_actual_first_publication","terminal_chain_published_current_read","terminal_chain_exact_epoch_plan","terminal_chain_actual_io_decision","terminal_chain_generation_log_and_frozen_current","terminal_chain_exact_generation_selector_read"],
        "not_implemented":["complete_first_run_setup","general_disk_add","metadata_readmission_after_terminal_retirement","foreign_admission","device_formatting_and_mounting","full_system_rebuild","other_samba_backends","program_repair"]
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
        let report =
            crate::authority::check::collect(&input.selection).map_err(work::disk_error)?;
        let bytes = json_codec::to_vec(&report).map_err(|error| {
            Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "report_json", error)
        })?;
        Ok::<_, Failure>(
            (
                [(header::CONTENT_TYPE, "application/json")],
                response_body::owned(bytes.into(), permit),
            )
                .into_response(),
        )
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
    // Clear the uniquely owned transport allocation; SecretString fields
    // clear their own allocations on drop.
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
    // Credential failures use a fixed public diagnostic to keep secrets private.
    Failure::message(
        StatusCode::CONFLICT,
        "local_samba_password_reset",
        &error.to_string(),
    )
}

fn maintenance(operation: Operation) -> MethodRouter<State> {
    post(
        move |ExtractState(state): ExtractState<State>, request: Request| {
            maintenance_operation(state, request, operation)
        },
    )
}

/// The helper is only the HTTP transport for management semantics. The matching
/// embedded program parses the exact typed request before any host access.
async fn maintenance_operation(
    state: State,
    request: Request,
    operation: Operation,
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
    )
        .into_response())
}

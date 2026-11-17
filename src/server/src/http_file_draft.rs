//! Authenticated shared Draft commands and bounded descriptor-based BODY I/O.
use crate::{ApiError, AppState, ChangeFileWorking, FileWorkingSnapshot};
use crate::http_boundary::{ApiJson, ApiPath, ApiQuery, Authenticated, JsonBody};
use crate::transfer_limits::timeout_error;
use data_serialization::Deserialize;
use futures::StreamExt;
use http_routing::{Json, extract::{Request, State}, response::Response};
use http::{HeaderMap, Method, StatusCode};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use storage_contract::{EntryId, FileId, RepositoryId};
use storage_contract::file_draft::{DraftCommand, DraftLocation};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct LocationQuery {
    repository_id: RepositoryId,
    path: name_policy::PortablePath,
    entry_id: EntryId,
}
impl LocationQuery {
    pub(super) fn target(self, file: FileId) -> DraftLocation {
        DraftLocation { repository: self.repository_id, path: self.path,
            entry: self.entry_id, file }
    }
}

pub(crate) struct CommandBody(ChangeFileWorking);
impl JsonBody for CommandBody {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        if !value.is_object()
            || !value.get("command").is_some_and(json_codec::Value::is_object)
            || !value.get("command").and_then(|value| value.get("target"))
                .is_some_and(json_codec::Value::is_object)
        {
            return Err(ApiError::invalid("draft command and target must be named objects"));
        }
        json_codec::from_value(value).map(Self).map_err(ApiError::invalid_source)
    }
}

pub(crate) async fn status(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FileId>, ApiQuery(query): ApiQuery<LocationQuery>,
) -> Result<Json<FileWorkingSnapshot>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.file_working(&identity, query.target(file)))
        .await.map(Json)
}

pub(crate) async fn change(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FileId>, ApiJson(body): ApiJson<CommandBody>,
) -> Result<Json<json_codec::Value>, ApiError> {
    if body.0.command.target().file != file {
        return Err(ApiError::invalid("draft command file differs from route"));
    }
    if matches!(body.0.command, DraftCommand::Write { .. }) {
        return Err(ApiError::invalid("draft writes require the streamed body endpoint"));
    }
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.change_file_working(&identity, body.0)
            .and_then(|result| crate::operation_wire::operation_wire(result.into())))
        .await.map(Json)
}

struct UploadAdmission {
    _permit: crate::transfer_limits::Permit,
    _claim: crate::uploads::Claim,
}

pub(crate) async fn write(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FileId>, request: Request,
) -> Result<Json<json_codec::Value>, ApiError> {
    state.http_spool()?;
    let deadline = tokio::time::Instant::now() + state.config.http_transfer_limits.total_timeout;
    let publication_admitted = AtomicBool::new(false);
    tokio::time::timeout_at(deadline,
        write_inner(state, identity, file, request, deadline, &publication_admitted))
        .await.map_err(|source| {
            let error = if publication_admitted.load(Ordering::Acquire) {
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE,
                    storage_contract::ErrorCode::TransferTimeout,
                    concat!("draft publication may have completed; ",
                        "query the original operation before retrying"),
                )
            } else { timeout_error() };
            error.with_source(source)
        })?
}

async fn write_inner(
    state: AppState, identity: crate::Identity, file: FileId, request: Request,
    deadline: tokio::time::Instant, publication_admitted: &AtomicBool,
) -> Result<Json<json_codec::Value>, ApiError> {
    let headers = request.headers();
    if headers.get_all("x-zxs-draft-command").iter().count() != 1 {
        return Err(ApiError::invalid("exactly one X-ZXS-Draft-Command is required"));
    }
    let encoded = headers.get("x-zxs-draft-command")
        .ok_or_else(|| ApiError::invalid("X-ZXS-Draft-Command is required"))?;
    if encoded.as_bytes().len() > 16_384 {
        return Err(ApiError::limit("draft command header exceeds its byte budget"));
    }
    let value = storage_contract::strict_json(encoded.as_bytes())
        .map_err(ApiError::contract_input)?;
    let CommandBody(command) = CommandBody::from_json(value)?;
    if command.command.target().file != file {
        return Err(ApiError::invalid("draft command file differs from route"));
    }
    let DraftCommand::Write { content_hash, logical_size, .. } = &command.command else {
        return Err(ApiError::invalid("body endpoint requires a draft write command"));
    };
    let (expected_hash, expected_length) = (*content_hash, *logical_size);
    if expected_length > state.config.max_upload_bytes {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE,
            storage_contract::ErrorCode::ResourceLimitExceeded, "draft exceeds upload limit"));
    }
    if headers.get_all("content-length").iter().count() > 1 {
        return Err(ApiError::invalid("duplicate Content-Length"));
    }
    if let Some(length) = headers.get("content-length") {
        let length = storage_contract::parse_u64_decimal(
            length.to_str().map_err(ApiError::invalid_source)?,
        ).map_err(ApiError::contract_input)?;
        if length != expected_length {
            return Err(ApiError::invalid("Content-Length differs from the draft command"));
        }
    }
    let permit = state.upload_slots.acquire()?;
    let key = format!("draft:{}:{}", command.command.target().repository, file);
    if !crate::lock(&state.upload_claims).insert(key.clone()) {
        return Err(ApiError::conflict(storage_contract::ErrorCode::FileWriteBusy,
            "another draft upload owns this file"));
    }
    let admission = Arc::new(UploadAdmission { _permit: permit,
        _claim: crate::uploads::Claim { claims: state.upload_claims.clone(), key } });
    let target = command.command.target().clone();
    let owner = identity.clone();
    let ownership = admission.clone();
    crate::http_job::HttpBackendJob::new(state.backend.clone(), state.auth.clone(), ownership)
        .run(move |backend| backend.authorize_file_draft_write(&owner, &target, expected_length))
        .await?;

    let name = format!("draft-upload-{}.data",
        authentication::random_token().map_err(ApiError::authentication_operation)?);
    let path = state.config.spool_dir.join(&name);
    let budget = state.http_spool()?;
    let principal = identity.principal.clone();
    let ownership = admission.clone();
    let staged = tokio::task::spawn_blocking(move || {
        let _ownership = ownership;
        let reservation = budget.reserve(&principal, &name, expected_length)?;
        crate::uploads::StagedFile::open_budgeted(path,
            platform_io::fs_safety::PayloadOpenMode::CreateNew, reservation)
    }).await.map_err(ApiError::join)??;
    let mut stream = request.into_body().into_data_stream();
    let mut length = 0u64;
    let mut digest = blake3::Hasher::new();
    let idle = state.config.http_transfer_limits.idle_timeout;
    let mut idle_deadline = (tokio::time::Instant::now() + idle).min(deadline);
    loop {
        if tokio::time::Instant::now() >= idle_deadline { return Err(timeout_error()); }
        let next = tokio::time::timeout_at(idle_deadline, stream.next()).await
            .map_err(|source| timeout_error().with_source(source))?;
        let Some(bytes) = next else { break; };
        let bytes = bytes.map_err(|source| ApiError::invalid("draft upload interrupted")
            .with_source(source))?;
        if bytes.is_empty() { continue; }
        length = length.checked_add(bytes.len() as u64)
            .ok_or_else(|| ApiError::limit("draft upload length overflow"))?;
        if length > expected_length {
            return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE,
                storage_contract::ErrorCode::UploadRangeConflict,
                "draft upload exceeds its declared length"));
        }
        let destination = staged.clone();
        let ownership = admission.clone();
        digest = tokio::task::spawn_blocking(move || {
            let _ownership = ownership;
            destination.write(&bytes)?;
            digest.update(&bytes);
            Ok::<_, ApiError>(digest)
        }).await.map_err(ApiError::join)??;
        idle_deadline = (tokio::time::Instant::now() + idle).min(deadline);
    }
    if length != expected_length {
        return Err(ApiError::new(StatusCode::BAD_REQUEST,
            storage_contract::ErrorCode::UploadIncomplete,
            "draft upload ended before its declared length"));
    }
    let hash = storage_contract::ContentHash::from_digest(digest.finalize());
    if hash != expected_hash {
        return Err(ApiError::new(StatusCode::BAD_REQUEST,
            storage_contract::ErrorCode::ChecksumMismatch,
            "draft upload hash differs from its command"));
    }
    // Every task owns the same quota, exclusive claim and staged descriptor.
    // Cancellation cannot release these while a blocking write/publication runs.
    publication_admitted.store(true, Ordering::Release);
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, admission)
        .run(move |backend| {
            staged.sync()?;
            let source = staged.source(length, hash)?;
            backend.write_file_draft(&identity, command, &source)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        }).await.map(Json)
}

pub(crate) async fn content(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FileId>, ApiQuery(query): ApiQuery<LocationQuery>,
    method: Method, headers: HeaderMap,
) -> Result<Response, ApiError> {
    let limits = state.config.http_transfer_limits.clone();
    let deadline = tokio::time::Instant::now() + limits.total_timeout;
    let permit = state.read_slots.acquire()?;
    let request = crate::ReadFileDraft {
        target: query.target(file), deadline: deadline.into_std(),
    };
    let content = tokio::time::timeout_at(deadline,
        crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
            .run_with_permit(move |backend, permit| {
                let mut content = backend.file_draft_content(&identity, request)?;
                content.lease = Box::new((content.lease, permit));
                Ok(content)
            })).await.map_err(|source| timeout_error().with_source(source))??;
    crate::transfer::content_response_with_limits(content, method, headers, &limits, deadline)
}

//! Shared complete-body staging. Quota, claim, descriptor and authorization
//! owners survive cancellation until the actual blocking publication finishes.
use crate::{ApiError, AppState, Backend, Identity, UploadSource};
use crate::transfer_limits::timeout_error;
use futures::StreamExt;
use http::StatusCode;
use http_routing::extract::Request;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

pub(crate) trait CompleteUpload: Send + 'static {
    type Output: Send + 'static;

    fn size(&self) -> u64;
    fn hash(&self) -> storage_contract::ContentHash;
    fn claim_key(&self) -> String;
    fn authorize(&self, backend: &dyn Backend, identity: &Identity) -> Result<(), ApiError>;
    fn publish(
        self,
        backend: &dyn Backend,
        identity: &Identity,
        source: &UploadSource,
    ) -> Result<Self::Output, ApiError>;
}

struct UploadAdmission {
    _permit: crate::transfer_limits::Permit,
    _claim: crate::uploads::Claim,
}

pub(crate) async fn receive<C: CompleteUpload>(
    state: AppState,
    identity: Identity,
    command: C,
    request: Request,
    budget: Arc<crate::spool_budget::SpoolBudget>,
) -> Result<C::Output, ApiError> {
    let deadline = tokio::time::Instant::now() + state.config.http_transfer_limits.total_timeout;
    let publication_admitted = AtomicBool::new(false);
    tokio::time::timeout_at(
        deadline,
        receive_inner(state, identity, command, request, budget, deadline, &publication_admitted),
    ).await.map_err(|source| {
        let error = if publication_admitted.load(Ordering::Acquire) {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::TransferTimeout,
                concat!("upload publication may have completed; ",
                    "query the original operation before retrying"),
            )
        } else {
            timeout_error()
        };
        error.with_source(source)
    })?
}

async fn receive_inner<C: CompleteUpload>(
    state: AppState,
    identity: Identity,
    command: C,
    request: Request,
    budget: Arc<crate::spool_budget::SpoolBudget>,
    deadline: tokio::time::Instant,
    publication_admitted: &AtomicBool,
) -> Result<C::Output, ApiError> {
    let headers = request.headers();
    let (expected_hash, expected_length) = (command.hash(), command.size());
    if expected_length > state.config.max_upload_bytes {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE,
            storage_contract::ErrorCode::ResourceLimitExceeded, "upload exceeds upload limit"));
    }
    if headers.get_all("content-length").iter().count() > 1 {
        return Err(ApiError::invalid("duplicate Content-Length"));
    }
    if let Some(length) = headers.get("content-length") {
        let length = storage_contract::parse_u64_decimal(
            length.to_str().map_err(ApiError::invalid_source)?,
        ).map_err(ApiError::contract_input)?;
        if length != expected_length {
            return Err(ApiError::invalid("Content-Length differs from the upload command"));
        }
    }
    let permit = state.upload_slots.acquire()?;
    let key = command.claim_key();
    if !crate::lock(&state.upload_claims).insert(key.clone()) {
        return Err(ApiError::conflict(storage_contract::ErrorCode::FileWriteBusy,
            "another complete upload owns this file"));
    }
    let admission = Arc::new(UploadAdmission { _permit: permit,
        _claim: crate::uploads::Claim { claims: state.upload_claims.clone(), key } });
    let owner = identity.clone();
    let ownership = admission.clone();
    let command = crate::http_job::HttpBackendJob::new(
        state.backend.clone(), state.auth.clone(), ownership,
    ).run(move |backend| {
        command.authorize(backend, &owner)?;
        Ok(command)
    }).await?;

    let name = format!("complete-upload-{}.data",
        authentication::random_token().map_err(ApiError::authentication_operation)?);
    let path = state.config.spool_dir.join(&name);
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
        let bytes = bytes.map_err(|source| ApiError::invalid("complete upload interrupted")
            .with_source(source))?;
        if bytes.is_empty() { continue; }
        length = length.checked_add(bytes.len() as u64)
            .ok_or_else(|| ApiError::limit("complete upload length overflow"))?;
        if length > expected_length {
            return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE,
                storage_contract::ErrorCode::UploadRangeConflict,
                "complete upload exceeds its declared length"));
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
            "complete upload ended before its declared length"));
    }
    let hash = storage_contract::ContentHash::from_digest(digest.finalize());
    if hash != expected_hash {
        return Err(ApiError::new(StatusCode::BAD_REQUEST,
            storage_contract::ErrorCode::ChecksumMismatch,
            "complete upload hash differs from its command"));
    }
    // Every task owns the same quota, exclusive claim and staged descriptor.
    // Cancellation cannot release these while a blocking write/publication runs.
    publication_admitted.store(true, Ordering::Release);
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, admission)
        .run(move |backend| {
            staged.sync()?;
            let source = staged.source(length, hash)?;
            command.publish(backend, &identity, &source)
        }).await
}

//! HTTP upload staging and immutable content streaming.
//!
//! The backend owns storage authorization and publication. This boundary owns
//! request limits, byte ranges, staging progress and the read-lease lifetime.
use crate::{
    ApiError, AppState, Content,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery},
    lock,
    transfer_limits::{BoundedStream, TransferLimits, timeout_error},
    uploads,
};
use data_serialization::Deserialize;
use futures::{FutureExt, StreamExt};
use http_routing::{
    Json,
    body::{Body, BodyDataStream},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use json_codec::{Value, json};
use std::{
    io::{Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) async fn upload_status(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((workspace, item)): ApiPath<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    state.http_spool()?;
    let read_permit = state.read_slots.acquire()?;
    let backend_permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        // Both quotas cover authorization, engine status and local spool reads.
        // Keep them until actual work ends, even if the requester disconnects.
        let _read_permit = read_permit;
        let _backend_permit = backend_permit;
        let input = json!({"workspace_id":workspace,"item_id":item});
        state.backend.call(
            crate::BackendAction::WorkspaceAuthorize,
            &identity,
            input.clone(),
        )?;
        let published =
            match state
                .backend
                .call(crate::BackendAction::UploadStatus, &identity, input)
            {
                Ok(status) => Ok(status),
                Err(error) if error.status == ::http::StatusCode::NOT_FOUND => Err(error),
                Err(error) => return Err(error),
            };
        if let Some(record) = uploads::load(&state.config.spool_dir, &workspace, &item)? {
            if record.principal != identity.principal {
                return Err(ApiError::forbidden());
            }
            // A replacement can be durably receiving while the engine still owns
            // the previous completed generation. Equal/newer engine generations
            // remain authoritative, including their completed publication state.
            let published_generation = published
                .as_ref()
                .ok()
                .and_then(|status| status.get("upload_generation"))
                .and_then(Value::as_str)
                .and_then(|generation| storage_contract::parse_u64_decimal(generation).ok());
            if published_generation.is_none_or(|generation| record.generation > generation) {
                return Ok(Json(uploads::status(&record)));
            }
        }
        published.map(Json)
    })
    .await
    .map_err(ApiError::join)?
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct UploadQuery {
    path: String,
}

/// Shared with every blocking metadata/publication task. Request cancellation
/// must not admit a retry until all tasks that can replace its record finish.
struct UploadAdmission {
    _permit: crate::transfer_limits::Permit,
    _claim: uploads::Claim,
}

struct UploadDeadline {
    at: tokio::time::Instant,
    publication_admitted: AtomicBool,
}

async fn persist_upload(
    record: &uploads::UploadRecord,
    admission: &Arc<UploadAdmission>,
    staged: &Arc<uploads::StagedFile>,
) -> Result<(), ApiError> {
    let record = record.clone();
    let admission = admission.clone();
    let staged = staged.clone();
    tokio::task::spawn_blocking(move || {
        let _admission = admission;
        staged.sync()?;
        uploads::persist_staged(&record, &staged)
    })
    .await
    .map_err(ApiError::join)?
}
// Only ready frames are combined: a paused sender's accepted tail must reach
// the file without waiting for the next frame or the idle timeout.
fn ready_upload_batch(
    stream: &mut futures::stream::Peekable<BodyDataStream>,
    first: bytes::Bytes,
    remaining: u64,
    checkpoint: u64,
    deadline: tokio::time::Instant,
) -> bytes::Bytes {
    const MAX_BYTES: usize = 256 * 1024;
    const MAX_FRAMES: usize = 64;
    let mut length = first.len();
    let mut combined: Option<bytes::BytesMut> = None;
    for _ in 1..MAX_FRAMES {
        if length >= MAX_BYTES
            || length as u64 >= checkpoint
            || tokio::time::Instant::now() >= deadline
        {
            break;
        }
        let next_length = match std::pin::Pin::new(&mut *stream).peek().now_or_never() {
            Some(Some(Ok(next))) if !next.is_empty() => next.len(),
            Some(Some(Ok(_))) | Some(Some(Err(_))) | Some(None) | None => break,
        };
        // Frame sizes and the declared remaining length are HTTP boundaries.
        if next_length > MAX_BYTES - length || next_length as u64 > remaining - length as u64 {
            break;
        }
        let next = stream
            .next()
            .now_or_never()
            .expect("invariant: peeked frame remains ready")
            .expect("invariant: peeked frame remains present")
            .expect("invariant: peeked frame is successful");
        let buffer = combined.get_or_insert_with(|| {
            let mut buffer = bytes::BytesMut::with_capacity(MAX_BYTES);
            buffer.extend_from_slice(&first);
            buffer
        });
        buffer.extend_from_slice(&next);
        length += next.len();
    }
    match combined {
        Some(buffer) => buffer.freeze(),
        None => first,
    }
}

pub(crate) async fn upload(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((workspace, item)): ApiPath<(String, String)>,
    ApiQuery(query): ApiQuery<UploadQuery>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    state.http_spool()?;
    let deadline = UploadDeadline {
        at: tokio::time::Instant::now() + state.config.http_transfer_limits.total_timeout,
        publication_admitted: AtomicBool::new(false),
    };
    tokio::time::timeout_at(
        deadline.at,
        upload_inner(state, identity, workspace, item, query, request, &deadline),
    )
    .await
    .map_err(|source| {
        let error = if deadline.publication_admitted.load(Ordering::Acquire) {
            ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::TransferTimeout,
                "upload publication may have completed; check upload status before retrying",
            )
        } else {
            timeout_error()
        };
        error.with_source(source)
    })?
}

async fn upload_inner(
    state: AppState,
    identity: Identity,
    workspace: String,
    item: String,
    query: UploadQuery,
    request: Request,
    deadline: &UploadDeadline,
) -> Result<Json<Value>, ApiError> {
    let _permit = state.upload_slots.acquire()?;
    let key = uploads::key(&workspace, &item);
    {
        let mut claims = lock(&state.upload_claims);
        if !claims.insert(key.clone()) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::FileWriteBusy,
                "another upload owns this item",
            ));
        }
    }
    let admission = Arc::new(UploadAdmission {
        _permit,
        _claim: uploads::Claim {
            claims: state.upload_claims.clone(),
            key,
        },
    });
    let logical_path =
        name_policy::PortablePath::parse(&query.path).map_err(ApiError::invalid_source)?;
    let headers = request.headers();
    let parse_header = |name: &'static str| -> Result<Option<u64>, ApiError> {
        if headers.get_all(name).iter().count() > 1 {
            return Err(ApiError::invalid("duplicate upload control header"));
        }
        headers
            .get(name)
            .map(|value| {
                value
                    .to_str()
                    .map_err(ApiError::invalid_source)
                    .and_then(|value| {
                        storage_contract::parse_u64_decimal(value).map_err(ApiError::invalid_source)
                    })
            })
            .transpose()
    };
    let expected_length = parse_header("x-zxs-logical-size")?
        .or(parse_header("content-length")?)
        .ok_or_else(|| ApiError::invalid("Content-Length or X-ZXS-Logical-Size is required"))?;
    if expected_length > state.config.max_upload_bytes {
        return Err(ApiError::new(
            ::http::StatusCode::PAYLOAD_TOO_LARGE,
            storage_contract::ErrorCode::ResourceLimitExceeded,
            "file exceeds upload limit",
        ));
    }
    let generation = parse_header("x-zxs-upload-generation")?.unwrap_or(1);
    if generation == 0 {
        return Err(ApiError::invalid("upload generation must be positive"));
    }
    let expected_hash = headers
        .get("x-zxs-content-hash")
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(ApiError::invalid_source)
        })
        .transpose()?;
    if expected_hash.is_none() {
        return Err(ApiError::invalid("X-ZXS-Content-Hash is required"));
    }
    if headers.get_all("x-zxs-content-hash").iter().count() != 1 {
        return Err(ApiError::invalid("duplicate content hash header"));
    }
    if expected_hash.as_ref().is_some_and(|hash| {
        hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(ApiError::invalid("invalid BLAKE3 upload hash"));
    }
    let backend = state.backend.clone();
    let owner = identity.clone();
    let ws = workspace.clone();
    let item_id = item.clone();
    let ownership = admission.clone();
    tokio::task::spawn_blocking(move || {
        let _ownership = ownership;
        backend.call(
            crate::BackendAction::UploadAuthorize,
            &owner,
            json!({"workspace_id":ws,"item_id":item_id,"logical_size":expected_length.to_string()}),
        )
    })
    .await
    .map_err(ApiError::join)??;
    let spool = state.config.spool_dir.clone();
    let root = spool.clone();
    let ws = workspace.clone();
    let item_id = item.clone();
    let ownership = admission.clone();
    let previous = tokio::task::spawn_blocking(move || {
        let _ownership = ownership;
        uploads::load(&root, &ws, &item_id)
    })
    .await
    .map_err(ApiError::join)??;
    let mut prefix = None;
    if let Some(previous) = &previous {
        if previous.principal != identity.principal || previous.path != query.path {
            return Err(ApiError::forbidden());
        }
        if generation < previous.generation {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::UploadGenerationConflict,
                "an upload from a newer generation already exists",
            ));
        }
        if generation == previous.generation {
            if expected_length != previous.logical_size
                || expected_hash
                    .as_ref()
                    .zip(previous.content_hash.as_ref())
                    .is_some_and(|(a, b)| a != b)
            {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::UploadGenerationConflict,
                    "length or content hash changed; explicitly start a newer upload generation",
                ));
            }
            let root = spool.clone();
            let previous = previous.clone();
            let ownership = admission.clone();
            prefix = tokio::task::spawn_blocking(move || {
                let _ownership = ownership;
                uploads::ExistingPrefix::open(&root, &previous)
            })
            .await
            .map_err(ApiError::join)??;
        }
    }
    let retry = previous
        .as_ref()
        .filter(|previous| previous.generation == generation);
    let data_file = match retry {
        Some(previous) => previous.data_file.clone(),
        None => format!(
            "upload-{}.data",
            authentication::random_token().map_err(ApiError::invalid_source)?
        ),
    };
    let path = spool.join(&data_file);
    let create_path = path.clone();
    let create_admission = admission.clone();
    let budget = state.http_spool()?;
    let principal = identity.principal.clone();
    let name = data_file.clone();
    let retry = retry.is_some();
    let staged_file = tokio::task::spawn_blocking(move || {
        let _admission = create_admission;
        let reservation = budget.reserve(&principal, &name, expected_length)?;
        uploads::StagedFile::open_budgeted(create_path, retry, reservation)
    })
    .await
    .map_err(ApiError::join)??;
    let mut record = uploads::UploadRecord {
        workspace_id: workspace.clone(),
        item_id: item.clone(),
        principal: identity.principal.clone(),
        path: query.path.clone(),
        generation,
        logical_size: expected_length,
        content_hash: expected_hash.clone(),
        persisted_bytes: 0,
        persisted_hash: blake3::hash(b"").to_hex().to_string(),
        state: crate::uploads::UploadState::Incomplete,
        data_file,
    };
    if let Some(previous) = previous
        .as_ref()
        .filter(|previous| generation > previous.generation)
    {
        let root = spool.clone();
        let previous = previous.clone();
        let ownership = (admission.clone(), staged_file.clone());
        tokio::task::spawn_blocking(move || {
            // Capture the whole owner, not only the staged-file tuple field:
            // cancellation must retain the per-item claim and transfer slot too.
            let _ownership = ownership;
            let result = uploads::retain_generation(&root, &previous);
            if result.is_err() {
                _ownership.1.block_admission();
            }
            result
        })
        .await
        .map_err(ApiError::join)??;
    }
    if previous
        .as_ref()
        .is_none_or(|previous| generation > previous.generation)
    {
        persist_upload(&record, &admission, &staged_file).await?;
    }
    let mut stream = request.into_body().into_data_stream().peekable();
    let mut length = 0u64;
    let mut last_persisted = 0u64;
    let mut hash = blake3::Hasher::new();
    let idle = state.config.http_transfer_limits.idle_timeout;
    let mut idle_deadline = (tokio::time::Instant::now() + idle).min(deadline.at);
    loop {
        // Ready empty frames must not starve the timer or extend idle time.
        let next = if tokio::time::Instant::now() >= idle_deadline {
            Err(None)
        } else {
            tokio::time::timeout_at(idle_deadline, stream.next())
                .await
                .map_err(Some)
        };
        let chunk = match next {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(source) => {
                if prefix.as_ref().is_none_or(|prefix| prefix.complete()) {
                    record.persisted_bytes = length;
                    record.persisted_hash = hash.clone().finalize().to_hex().to_string();
                    persist_upload(&record, &admission, &staged_file).await?;
                }
                let error = timeout_error();
                return Err(match source {
                    Some(source) => error.with_source(source),
                    None => error,
                });
            }
        };
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                if prefix.as_ref().is_none_or(|prefix| prefix.complete()) {
                    record.persisted_bytes = length;
                    record.persisted_hash = hash.clone().finalize().to_hex().to_string();
                    persist_upload(&record, &admission, &staged_file).await?;
                }
                return Err(ApiError::invalid(format!(
                    "upload interrupted; durable work data is retained: {error}"
                ))
                .with_source(error));
            }
        };
        if chunk.is_empty() {
            tokio::task::yield_now().await;
            continue;
        }
        idle_deadline = (tokio::time::Instant::now() + idle).min(deadline.at);
        length = length
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| ApiError::invalid("upload length overflow"))?;
        if length > expected_length {
            return Err(ApiError::new(
                ::http::StatusCode::PAYLOAD_TOO_LARGE,
                storage_contract::ErrorCode::UploadRangeConflict,
                "received more bytes than declared; original upload is retained",
            ));
        }
        let previous_length = length - chunk.len() as u64;
        let checkpoint = (1024 * 1024u64)
            .saturating_sub(previous_length - last_persisted)
            .max(
                prefix
                    .as_ref()
                    .map_or(0, uploads::ExistingPrefix::remaining),
            );
        let chunk = ready_upload_batch(
            &mut stream,
            chunk,
            expected_length - previous_length,
            checkpoint,
            idle_deadline,
        );
        length = previous_length + chunk.len() as u64;
        idle_deadline = (tokio::time::Instant::now() + idle).min(deadline.at);
        let skipped = prefix.as_ref().map_or(0, |existing| {
            existing.remaining().min(chunk.len() as u64) as usize
        });
        let mut existing = prefix.take();
        let bytes = chunk.clone();
        let staged = staged_file.clone();
        let ownership = admission.clone();
        prefix = tokio::task::spawn_blocking(move || {
            let _admission = ownership;
            if let Some(prefix) = &mut existing {
                prefix.check(&bytes)?;
            }
            if skipped < bytes.len() {
                staged.write(&bytes[skipped..])?;
            }
            Ok::<_, ApiError>(existing.filter(|prefix| !prefix.complete()))
        })
        .await
        .map_err(ApiError::join)??;
        hash.update(&chunk);
        if length.saturating_sub(last_persisted) >= 1024 * 1024
            && prefix.as_ref().is_none_or(|prefix| prefix.complete())
        {
            record.persisted_bytes = length;
            record.persisted_hash = hash.clone().finalize().to_hex().to_string();
            persist_upload(&record, &admission, &staged_file).await?;
            last_persisted = length;
        }
    }
    if prefix.as_ref().is_some_and(|prefix| !prefix.complete()) {
        return Err(ApiError::new(
            ::http::StatusCode::BAD_REQUEST,
            storage_contract::ErrorCode::UploadIncomplete,
            "retry ended before the previously persisted prefix; original work data is retained",
        ));
    }
    record.persisted_bytes = length;
    record.persisted_hash = hash.clone().finalize().to_hex().to_string();
    persist_upload(&record, &admission, &staged_file).await?;
    if length != expected_length {
        return Err(ApiError::new(
            ::http::StatusCode::BAD_REQUEST,
            storage_contract::ErrorCode::UploadIncomplete,
            "upload ended before its declared length; durable work data is retained",
        ));
    }
    let actual_hash = hash.finalize().to_hex().to_string();
    if expected_hash.is_some_and(|expected| expected != actual_hash) {
        record.state = crate::uploads::UploadState::ChecksumMismatch;
        persist_upload(&record, &admission, &staged_file).await?;
        return Err(ApiError::new(
            ::http::StatusCode::BAD_REQUEST,
            storage_contract::ErrorCode::ChecksumMismatch,
            "uploaded bytes do not match the declared hash; work data is retained",
        ));
    }
    record.content_hash = Some(actual_hash.clone());
    let backend = state.backend.clone();
    let owner = identity.clone();
    let ws = workspace.clone();
    let item_id = item.clone();
    // Once admitted, blocking publication can outlive the HTTP deadline, even
    // while still queued. A timeout can no longer promise a rejected mutation.
    deadline.publication_admitted.store(true, Ordering::Release);
    let staged = tokio::task::spawn_blocking(move || {
        // A disconnected HTTP request cannot cancel a running blocking task.
        // Keep its admission slot and per-item claim with publication itself,
        // including the final durable status, so a retry cannot race it.
        let _admission = admission;
        let source = staged_file.source(expected_length, actual_hash)?;
        let staged =
            backend.upload_generation(&owner, &ws, &item_id, &logical_path, &source, generation)?;
        record.state = crate::uploads::UploadState::Complete;
        uploads::persist_staged(&record, &staged_file)?;
        Ok::<_, ApiError>(staged)
    })
    .await
    .map_err(ApiError::join)??;
    Ok(Json(staged))
}

fn parse_range(value: &str, size: u64) -> Result<(u64, u64), ApiError> {
    let value = value.strip_prefix("bytes=").ok_or_else(|| {
        ApiError::new(
            ::http::StatusCode::RANGE_NOT_SATISFIABLE,
            storage_contract::ErrorCode::RangeNotSatisfiable,
            "unsupported byte range",
        )
    })?;
    if value.contains(',') || size == 0 {
        return Err(ApiError::new(
            ::http::StatusCode::RANGE_NOT_SATISFIABLE,
            storage_contract::ErrorCode::RangeNotSatisfiable,
            "only one satisfiable byte range is supported",
        ));
    }
    let (start, end) = value
        .split_once('-')
        .ok_or_else(|| invalid_range("invalid byte range"))?;
    let parse = |value: &str| {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid_range("invalid byte range"));
        }
        value
            .parse::<u64>()
            .map_err(|source| invalid_range("invalid byte range").with_source(source))
    };
    let (start, end) = if start.is_empty() {
        let suffix = parse(end)?;
        if suffix == 0 {
            return Err(ApiError::new(
                ::http::StatusCode::RANGE_NOT_SATISFIABLE,
                storage_contract::ErrorCode::RangeNotSatisfiable,
                "empty suffix range",
            ));
        }
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start = parse(start)?;
        let end = if end.is_empty() {
            size - 1
        } else {
            parse(end)?.min(size - 1)
        };
        (start, end)
    };
    if start >= size || start > end {
        return Err(ApiError::new(
            ::http::StatusCode::RANGE_NOT_SATISFIABLE,
            storage_contract::ErrorCode::RangeNotSatisfiable,
            "byte range is outside the file",
        ));
    }
    Ok((start, end))
}

fn invalid_range(message: &str) -> ApiError {
    ApiError::new(
        ::http::StatusCode::RANGE_NOT_SATISFIABLE,
        storage_contract::ErrorCode::RangeNotSatisfiable,
        message,
    )
}

fn unsatisfiable_range(error: ApiError, size: u64, method: &Method) -> Response {
    let mut response = error.into_response();
    response.headers_mut().insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes */{size}")).unwrap(),
    );
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if *method == Method::HEAD {
        *response.body_mut() = Body::empty();
    }
    response
}
#[derive(Default, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ContentQuery {
    repository_id: Option<String>,
    path: Option<String>,
}
pub(crate) async fn content(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(version): ApiPath<String>,
    ApiQuery(query): ApiQuery<ContentQuery>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let limits = state.config.http_transfer_limits.clone();
    let deadline = tokio::time::Instant::now() + limits.total_timeout;
    let permit = state.read_slots.acquire()?;
    let content = tokio::time::timeout_at(
        deadline,
        tokio::task::spawn_blocking(move || {
            if query.repository_id.is_some() != query.path.is_some() {
                return Err(ApiError::invalid(
                    "repository_id and path must be supplied together",
                ));
            }
            let logical_name = if let (Some(repo), Some(path)) = (query.repository_id, query.path) {
                let metadata = state.backend.call(
                    crate::BackendAction::FileManagerDownloadContext,
                    &identity,
                    json!({
                        "repository_id":repo,"path":path,"version_id":version
                    }),
                )?;
                Some(
                    metadata
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            ApiError::new(
                                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                                storage_contract::ErrorCode::StateInvalid,
                                "download context has no filename",
                            )
                        })?
                        .to_owned(),
                )
            } else {
                None
            };
            let mut content = state.backend.range_content(&identity, &version)?;
            if logical_name.is_some() {
                content.download_name = logical_name;
            }
            content.lease = Box::new((content.lease, permit));
            Ok(content)
        }),
    )
    .await
    .map_err(|source| timeout_error().with_source(source))?
    .map_err(ApiError::join)??;
    content_response_with_limits(content, method, headers, &limits, deadline)
}
pub(crate) async fn workspace_content(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((workspace, item)): ApiPath<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let limits = state.config.http_transfer_limits.clone();
    let deadline = tokio::time::Instant::now() + limits.total_timeout;
    let permit = state.read_slots.acquire()?;
    let content = tokio::time::timeout_at(
        deadline,
        tokio::task::spawn_blocking(move || {
            let mut content = state
                .backend
                .workspace_content(&identity, &workspace, &item)?;
            content.lease = Box::new((content.lease, permit));
            Ok::<_, ApiError>(content)
        }),
    )
    .await
    .map_err(|source| timeout_error().with_source(source))?
    .map_err(ApiError::join)??;
    content_response_with_limits(content, method, headers, &limits, deadline)
}
pub(crate) async fn checksums(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(version): ApiPath<String>,
) -> Result<Response, ApiError> {
    let limits = state.config.http_transfer_limits.clone();
    let deadline = tokio::time::Instant::now() + limits.total_timeout;
    let permit = state.read_slots.acquire()?;
    let (bytes, permit) = tokio::time::timeout_at(
        deadline,
        tokio::task::spawn_blocking(move || {
            let bytes = state.backend.checksums(&identity, &version)?;
            Ok::<_, ApiError>((bytes, permit))
        }),
    )
    .await
    .map_err(|source| timeout_error().with_source(source))?
    .map_err(ApiError::join)??;
    let length = bytes.len();
    let stream = futures::stream::try_unfold(
        (bytes::Bytes::from(bytes), permit),
        |(mut bytes, permit)| async move {
            if bytes.is_empty() {
                return Ok::<_, std::io::Error>(None);
            }
            // A transport-owned chunk must not pin the entire manifest after
            // timeout drops the remaining body and returns its admission slot.
            let chunk = bytes::Bytes::copy_from_slice(&bytes.split_to(bytes.len().min(64 * 1024)));
            Ok(Some((chunk, (bytes, permit))))
        },
    );
    let mut response =
        Body::from_stream(BoundedStream::new(stream, &limits, deadline)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).unwrap(),
    );
    Ok(response)
}
#[cfg(test)]
fn content_response(
    content: Content,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let limits = TransferLimits::default();
    content_response_with_limits(
        content,
        method,
        headers,
        &limits,
        tokio::time::Instant::now() + limits.total_timeout,
    )
}
fn content_response_with_limits(
    content: Content,
    method: Method,
    headers: HeaderMap,
    limits: &TransferLimits,
    deadline: tokio::time::Instant,
) -> Result<Response, ApiError> {
    let manifest = content.checksum_manifest;
    let download_name = content.download_name;
    let mut file = content.file;
    let size = content.size;
    let hash = content.hash;
    let lease = content.lease;
    let etag = format!("\"{hash}\"");
    let range = if headers
        .get(header::IF_RANGE)
        .is_some_and(|value| value.to_str().ok() != Some(etag.as_str()))
    {
        None
    } else {
        headers.get(header::RANGE)
    };
    if headers.get_all(header::RANGE).iter().count() > 1 {
        return Ok(unsatisfiable_range(
            invalid_range("duplicate range header"),
            size,
            &method,
        ));
    }
    let (status, start, length) = if let Some(range) = range {
        match range
            .to_str()
            .map_err(|source| invalid_range("invalid byte range").with_source(source))
            .and_then(|range| parse_range(range, size))
        {
            Ok((start, end)) => (StatusCode::PARTIAL_CONTENT, start, end - start + 1),
            Err(error) => {
                return Ok(unsatisfiable_range(error, size, &method));
            }
        }
    } else {
        (StatusCode::OK, 0, size)
    };

    let body = if method == Method::HEAD {
        Body::empty()
    } else if let Some(manifest) = manifest {
        let reader = VerifiedRange::new(file, content.fallback_files, manifest, start, length)?;
        // One bounded chunk per blocking job. The state, lease and admission
        // permit travel with that job, including after HTTP cancellation.
        let stream =
            futures::stream::try_unfold((reader, lease), |(mut reader, lease)| async move {
                tokio::task::spawn_blocking(move || {
                    reader.next_chunk().map(|chunk| {
                        chunk.map(|chunk| (bytes::Bytes::from(chunk), (reader, lease)))
                    })
                })
                .await
                .map_err(std::io::Error::other)?
            });
        Body::from_stream(BoundedStream::new(stream, limits, deadline))
    } else {
        file.seek(SeekFrom::Start(start)).map_err(ApiError::io)?;
        let stream = futures::stream::try_unfold(
            (file, length, lease),
            |(mut file, remaining, lease)| async move {
                if remaining == 0 {
                    return Ok(None);
                }
                tokio::task::spawn_blocking(move || {
                    let mut bytes = vec![0; remaining.min(64 * 1024) as usize];
                    file.read_exact(&mut bytes)?;
                    let remaining = remaining - bytes.len() as u64;
                    Ok::<_, std::io::Error>(Some((
                        bytes::Bytes::from(bytes),
                        (file, remaining, lease),
                    )))
                })
                .await
                .map_err(std::io::Error::other)?
            },
        );
        Body::from_stream(BoundedStream::new(stream, limits, deadline))
    };
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let headers = response.headers_mut();
    if let Some(name) = download_name {
        let fallback: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let mut encoded = String::new();
        for byte in name.as_bytes() {
            if byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-') {
                encoded.push(*byte as char);
            } else {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!(
                "attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}"
            ))
            .expect("invariant: escaped download name is a valid header"),
        );
    }
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).unwrap(),
    );
    // Public Backend implementations supply the content hash.
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).map_err(ApiError::invalid_source)?,
    );
    if status == StatusCode::PARTIAL_CONTENT {
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{}/{size}", start + length - 1)).unwrap(),
        );
    }
    Ok(response)
}

/// Reads the entire authenticated chunk into a private buffer and exposes only
/// verified bytes from that same buffer. Never verify then reread the payload.
struct VerifiedRange<R> {
    file: R,
    fallbacks: Vec<R>,
    manifest: storage_contract::ChecksumManifest,
    position: u64,
    end: u64,
}
impl<R: std::io::Read + Seek> VerifiedRange<R> {
    fn new(
        file: R,
        fallbacks: Vec<R>,
        manifest: storage_contract::ChecksumManifest,
        start: u64,
        length: u64,
    ) -> Result<Self, ApiError> {
        let end = start
            .checked_add(length)
            .filter(|end| *end <= manifest.logical_size)
            .ok_or_else(|| ApiError::invalid("range exceeds authenticated size"))?;
        if !storage_contract::SUPPORTED_CHECKSUM_CHUNK_BYTES.contains(&manifest.chunk_bytes)
            || manifest.chunks.len() as u64
                != manifest.logical_size.div_ceil(manifest.chunk_bytes as u64)
        {
            return Err(ApiError::invalid("invalid checksum manifest"));
        }
        Ok(Self {
            file,
            fallbacks,
            manifest,
            position: start,
            end,
        })
    }
    fn next_chunk(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        if self.position == self.end {
            return Ok(None);
        }
        let chunk_bytes = self.manifest.chunk_bytes as u64;
        let index = self.position / chunk_bytes;
        let base = index * chunk_bytes;
        let count = chunk_bytes.min(self.manifest.logical_size - base) as usize;
        let mut buffer = vec![0; count];
        let expected = &self.manifest.chunks[index as usize];
        let mut verified_source = None;
        for (source, file) in std::iter::once(&mut self.file)
            .chain(self.fallbacks.iter_mut())
            .enumerate()
        {
            if file.seek(SeekFrom::Start(base)).is_ok()
                && file.read_exact(&mut buffer).is_ok()
                && blake3::hash(&buffer).as_bytes() == expected
            {
                verified_source = Some(source);
                break;
            }
        }
        let Some(source) = verified_source else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "CHECKSUM_MISMATCH: no replica has a verified payload chunk",
            ));
        };
        if source != 0 {
            // Prefer the last successful source, but retain every other replica:
            // corruption can affect different chunks on different members.
            // This is only read ordering, never evidence for the next chunk.
            std::mem::swap(&mut self.file, &mut self.fallbacks[source - 1]);
        }
        let from = (self.position - base) as usize;
        let to = (self.end - base).min(count as u64) as usize;
        self.position = base + to as u64;
        buffer.truncate(to);
        buffer.drain(..from);
        Ok(Some(buffer))
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/transfer/tests.rs"]
mod tests;

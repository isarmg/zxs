//! Shared HTTP rejections and bounded request decoding.
//!
//! Translate failures before a response starts. Content stream errors remain
//! transport failures; they cannot be replaced by a JSON envelope after headers.
use data_serialization::{Deserialize, Serialize, de::DeserializeOwned};
use http_routing::{
    Json,
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Path, Query, Request},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use json_codec::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub code: storage_contract::ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub request_id: String,
    #[serialization(skip)]
    pub status: u16,
}
impl ApiError {
    pub fn new(status: u16, code: storage_contract::ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retryable: (status >= 500 && status != 501) || status == 429,
            request_id: storage_contract::new_id("req"),
        }
    }
    pub fn invalid(error: impl std::fmt::Display) -> Self {
        Self::new(
            400,
            storage_contract::ErrorCode::InvalidRequest,
            error.to_string(),
        )
    }
    pub(crate) fn join(error: tokio::task::JoinError) -> Self {
        tracing::error!(%error, "blocking request task failed");
        Self::new(
            500,
            storage_contract::ErrorCode::InternalError,
            "request task failed",
        )
    }
    pub fn io(error: std::io::Error) -> Self {
        tracing::error!(%error, "local I/O failed");
        Self::new(
            500,
            storage_contract::ErrorCode::IoError,
            "local storage operation failed",
        )
    }
    pub fn unauthorized() -> Self {
        Self::new(
            401,
            storage_contract::ErrorCode::Unauthorized,
            "authentication required or session expired",
        )
    }
    pub fn forbidden() -> Self {
        Self::new(
            403,
            storage_contract::ErrorCode::Forbidden,
            "permission denied",
        )
    }
    pub fn conflict(code: storage_contract::ErrorCode, message: &str) -> Self {
        Self::new(409, code, message)
    }
    pub fn limit(message: &str) -> Self {
        Self::new(
            429,
            storage_contract::ErrorCode::ResourceLimitExceeded,
            message,
        )
    }
    pub fn unsupported() -> Self {
        Self::new(
            501,
            storage_contract::ErrorCode::UnsupportedOperation,
            "operation is not supported by this build",
        )
    }
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ApiError {}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(json!({"error":self}))).into_response()
    }
}

/// Reject duplicate decoded fields before map deserialization can erase them.
/// Preserve Axum query parsing, but never expose its plain-text rejection body.
pub(crate) struct ApiQuery<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let mut fields = std::collections::BTreeSet::new();
        for (field, _) in
            url_encoding::form::parse(parts.uri.query().unwrap_or_default().as_bytes())
        {
            if !fields.insert(field) {
                return Err(ApiError::invalid("duplicate query field"));
            }
        }
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|error| ApiError::invalid(error.body_text()))
    }
}

/// Invalid client path values are 400; missing routing state remains a 500.
pub(crate) struct ApiPath<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiPath<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(|error| {
                if error.status().is_server_error() {
                    tracing::error!(%error, "request path extraction failed");
                    ApiError::new(
                        500,
                        storage_contract::ErrorCode::InternalError,
                        "request routing failed",
                    )
                } else {
                    ApiError::invalid(error.body_text())
                }
            })
    }
}

/// Use Axum's bounded byte extractor so both fixed and chunked bodies report
/// 413 on overflow, without relying on Content-Length or collecting unbounded data.
pub(crate) async fn read_body(request: Request, limit: usize) -> Result<Bytes, ApiError> {
    read_body_with_timeout(request, limit, std::time::Duration::from_secs(30)).await
}

pub(crate) async fn read_body_with_timeout(
    mut request: Request,
    limit: usize,
    timeout: std::time::Duration,
) -> Result<Bytes, ApiError> {
    DefaultBodyLimit::max(limit).apply(&mut request);
    tokio::time::timeout(timeout, Bytes::from_request(request, &()))
        .await
        .map_err(|_| {
            ApiError::new(
                408,
                storage_contract::ErrorCode::RequestTimeout,
                "request body deadline exceeded",
            )
        })?
        .map_err(|error| {
            if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                ApiError::new(
                    413,
                    storage_contract::ErrorCode::PayloadTooLarge,
                    "request body exceeds limit",
                )
            } else {
                ApiError::invalid("failed to read request body")
            }
        })
}

/// Bound slow body senders before decoding or password-rate-limit state exists.
/// Read-only management/health requests keep independent admission.
pub(crate) async fn admit_request(
    http_routing::extract::State(slots): http_routing::extract::State<
        std::sync::Arc<tokio::sync::Semaphore>,
    >,
    request: Request,
    next: http_routing::middleware::Next,
) -> Result<Response, ApiError> {
    let _permit = if is_api_path(request.uri().path())
        && !matches!(
            *request.method(),
            http_routing::http::Method::GET
                | http_routing::http::Method::HEAD
                | http_routing::http::Method::OPTIONS
        ) {
        Some(
            slots
                .try_acquire_owned()
                .map_err(|_| ApiError::limit("HTTP request capacity reached"))?,
        )
    } else {
        None
    };
    Ok(next.run(request).await)
}

pub(crate) async fn not_found() -> ApiError {
    ApiError::new(
        404,
        storage_contract::ErrorCode::NotFound,
        "route not found",
    )
}

pub(crate) async fn method_not_allowed() -> ApiError {
    // Axum adds the matched route's Allow header to this fallback response.
    ApiError::new(
        405,
        storage_contract::ErrorCode::MethodNotAllowed,
        "method not allowed for this route",
    )
}

pub(crate) fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

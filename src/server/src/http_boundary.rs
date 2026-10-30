//! Shared HTTP rejections and bounded request decoding.
//!
//! Translate failures before a response starts. Content stream errors remain
//! transport failures; they cannot be replaced by a JSON envelope after headers.
use data_serialization::{Serialize, de::DeserializeOwned};
use http_routing::{
    Json,
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Path, Query, Request},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use json_codec::json;

#[derive(Debug, error_definition::Error)]
pub(crate) enum ApiErrorSource {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("{0}")]
    Contract(#[from] storage_contract::Error),
    #[error("{0}")]
    ContentHash(#[from] storage_contract::ParseContentHashError),
    #[cfg(feature = "core")]
    #[error("{0}")]
    Engine(#[from] storage_engine::Error),
    #[error("{0}")]
    Json(#[from] json_codec::Error),
    #[error("{0}")]
    Filesystem(#[from] platform_io::fs_safety::Error),
    #[error("{0}")]
    Authentication(#[from] authentication::Error),
    #[error("{0}")]
    Integer(#[from] std::num::ParseIntError),
    #[error("{0}")]
    IpAddress(#[from] std::net::AddrParseError),
    #[error("{0}")]
    IntegerRange(#[from] std::num::TryFromIntError),
    #[error("{0}")]
    FileLock(#[from] std::fs::TryLockError),
    #[error("filename is not valid UTF-8: {0:?}")]
    Filename(std::ffi::OsString),
    #[error("{0}")]
    HeaderText(#[from] http::header::ToStrError),
    #[error("{0}")]
    HeaderValue(#[from] http::header::InvalidHeaderValue),
    #[error("{0}")]
    Url(#[from] url_encoding::address::ParseError),
    #[error("{0}")]
    Query(#[from] http_routing::extract::rejection::QueryRejection),
    #[error("{0}")]
    Path(#[from] http_routing::extract::rejection::PathRejection),
    #[error("{0}")]
    Body(#[from] http_routing::extract::rejection::BytesRejection),
    #[error("{0}")]
    BodyStream(#[from] http_routing::Error),
    #[error("{0}")]
    Timeout(#[from] tokio::time::error::Elapsed),
    #[error("{0}")]
    Capacity(#[from] tokio::sync::TryAcquireError),
    #[error("{0}")]
    Timestamp(#[from] date_time::error::ComponentRange),
    #[error("{0}")]
    DateFormat(#[from] date_time::error::Format),
    #[error("{0}")]
    DateParse(#[from] date_time::error::Parse),
    #[error("{0}")]
    Clock(#[from] std::time::SystemTimeError),
    #[cfg(feature = "core")]
    #[error("{0}")]
    Admission(#[from] std::sync::Arc<ApiError>),
}

impl From<name_policy::Error> for ApiErrorSource {
    fn from(source: name_policy::Error) -> Self {
        Self::Contract(source.into())
    }
}

#[derive(Debug, Serialize, error_definition::Error)]
#[error("{code}: {message}")]
pub struct ApiError {
    pub code: storage_contract::ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub request_id: String,
    #[serialization(skip)]
    pub status: StatusCode,
    #[serialization(skip)]
    #[source]
    pub(crate) source: Option<Box<ApiErrorSource>>,
}
impl ApiError {
    pub fn new(
        status: StatusCode,
        code: storage_contract::ErrorCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            code,
            source: None,
            message: message.into(),
            retryable: (status.is_server_error() && status != StatusCode::NOT_IMPLEMENTED)
                || status == StatusCode::TOO_MANY_REQUESTS,
            request_id: storage_contract::new_id("req"),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(
            ::http::StatusCode::BAD_REQUEST,
            storage_contract::ErrorCode::InvalidRequest,
            message,
        )
    }
    pub(crate) fn invalid_source(source: impl Into<ApiErrorSource>) -> Self {
        let source = source.into();
        Self::invalid(source.to_string()).with_source(source)
    }
    pub(crate) fn contract_input(source: storage_contract::Error) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            source.code(),
            source.message().into_owned(),
        )
        .with_source(source)
    }
    pub(crate) fn with_source(mut self, source: impl Into<ApiErrorSource>) -> Self {
        self.source = Some(Box::new(source.into()));
        self
    }
    pub(crate) fn log_response(&self) {
        if self.status.is_server_error() {
            tracing::error!(error = ?self, "request failed");
        }
    }
    pub(crate) fn join(error: tokio::task::JoinError) -> Self {
        Self::new(
            ::http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::InternalError,
            "request task failed",
        )
        .with_source(error)
    }
    pub fn io(error: std::io::Error) -> Self {
        Self::new(
            ::http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::IoError,
            "local storage operation failed",
        )
        .with_source(error)
    }
    pub fn unauthorized() -> Self {
        Self::new(
            ::http::StatusCode::UNAUTHORIZED,
            storage_contract::ErrorCode::Unauthorized,
            "authentication required or session expired",
        )
    }
    pub fn forbidden() -> Self {
        Self::new(
            ::http::StatusCode::FORBIDDEN,
            storage_contract::ErrorCode::Forbidden,
            "permission denied",
        )
    }
    pub fn conflict(code: storage_contract::ErrorCode, message: &str) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }
    pub fn limit(message: &str) -> Self {
        Self::new(
            ::http::StatusCode::TOO_MANY_REQUESTS,
            storage_contract::ErrorCode::ResourceLimitExceeded,
            message,
        )
    }
    pub fn unsupported() -> Self {
        Self::new(
            ::http::StatusCode::NOT_IMPLEMENTED,
            storage_contract::ErrorCode::UnsupportedOperation,
            "operation is not supported by this build",
        )
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        self.log_response();
        (self.status, Json(json!({"error":self}))).into_response()
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
            .map_err(|error| ApiError::invalid(error.body_text()).with_source(error))
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
                    ApiError::new(
                        ::http::StatusCode::INTERNAL_SERVER_ERROR,
                        storage_contract::ErrorCode::InternalError,
                        "request routing failed",
                    )
                    .with_source(error)
                } else {
                    ApiError::invalid(error.body_text()).with_source(error)
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
        .map_err(|source| {
            ApiError::new(
                ::http::StatusCode::REQUEST_TIMEOUT,
                storage_contract::ErrorCode::RequestTimeout,
                "request body deadline exceeded",
            )
            .with_source(source)
        })?
        .map_err(|error| {
            if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                ApiError::new(
                    ::http::StatusCode::PAYLOAD_TOO_LARGE,
                    storage_contract::ErrorCode::PayloadTooLarge,
                    "request body exceeds limit",
                )
                .with_source(error)
            } else {
                ApiError::invalid("failed to read request body").with_source(error)
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
        Some(slots.try_acquire_owned().map_err(|source| {
            ApiError::limit("HTTP request capacity reached").with_source(source)
        })?)
    } else {
        None
    };
    Ok(next.run(request).await)
}

pub(crate) async fn not_found() -> ApiError {
    ApiError::new(
        ::http::StatusCode::NOT_FOUND,
        storage_contract::ErrorCode::NotFound,
        "route not found",
    )
}

pub(crate) async fn method_not_allowed() -> ApiError {
    // Axum adds the matched route's Allow header to this fallback response.
    ApiError::new(
        ::http::StatusCode::METHOD_NOT_ALLOWED,
        storage_contract::ErrorCode::MethodNotAllowed,
        "method not allowed for this route",
    )
}

pub(crate) fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

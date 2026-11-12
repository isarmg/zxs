//! Verified lifecycle evidence under bounded current-session read admission.
use crate::{
    ApiError, AppState,
    http_boundary::{ApiPath, ApiQuery, Authenticated},
};
use data_serialization::Deserialize;
use http_routing::{
    extract::{FromRequestParts, State},
    http::request::Parts,
    response::Response,
};
use storage_contract::file_lifecycle::{Request, MAX_RESPONSE_BYTES};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Query {
    history_root: String,
    repository_id: Option<String>,
    path: Option<String>,
    cursor: Option<String>,
    limit: usize,
}

/// Keep contract errors until bounded read admission has reserved capacity.
pub(crate) struct RequestInput(storage_contract::Result<Request>);

impl<S: Send + Sync> FromRequestParts<S> for RequestInput {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let ApiPath(file) = ApiPath::<String>::from_request_parts(parts, state).await?;
        let ApiQuery(query) = ApiQuery::<Query>::from_request_parts(parts, state).await?;
        Ok(Self(Request::parse(
            &file,
            &query.history_root,
            query.repository_id.as_deref(),
            query.path.as_deref(),
            query.cursor.as_deref(),
            query.limit,
        )))
    }
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    RequestInput(request): RequestInput,
) -> Result<Response, ApiError> {
    crate::bounded_history_http::read(state, move |backend, cancellation| {
        let request = request.map_err(ApiError::contract_input)?;
        let page = backend.file_lifecycle(&identity, request.clone(), cancellation)?;
        page.validate(&request).map_err(|source| {
            ApiError::new(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::StateInvalid,
                "lifecycle backend returned an invalid page",
            )
            .with_source(source)
        })?;
        crate::history_window_encode::encode_bounded(&page, MAX_RESPONSE_BYTES)
    })
    .await
}

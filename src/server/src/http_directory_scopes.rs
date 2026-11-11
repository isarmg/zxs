//! The public query format selects a typed scope and one response projection.
use crate::{
    ApiError, AppState, DirectoryBranchFilter, DirectoryBranchProjection, ReadDirectoryScopes,
    backend_directory_scopes::DirectoryScopesResponse,
    http_boundary::{ApiPath, ApiQuery, Authenticated},
};
use http_routing::{
    Json,
    extract::{FromRequestParts, State},
    http::request::Parts,
};
use std::collections::BTreeMap;

fn parse(
    mut query: BTreeMap<String, String>,
) -> Result<(DirectoryBranchFilter, DirectoryBranchProjection), ApiError> {
    if let Some(key) = query
        .keys()
        .find(|key| !["path", "summary", "owner_main"].contains(&key.as_str()))
    {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let projection = match query.remove("summary").as_deref() {
        None => DirectoryBranchProjection::Full,
        Some("true") => DirectoryBranchProjection::Summary,
        Some(_) => return Err(ApiError::invalid("summary must be true when supplied")),
    };
    let owning_main = match query.remove("owner_main").as_deref() {
        None => false,
        Some("true") if matches!(projection, DirectoryBranchProjection::Summary) => true,
        Some(_) => {
            return Err(ApiError::invalid(
                "owner_main requires true and summary=true",
            ));
        }
    };
    let path = query.remove("path");
    if owning_main {
        let path = path.ok_or_else(|| ApiError::invalid("owner_main requires an explicit path"))?;
        let path = if path.is_empty() {
            None
        } else {
            Some(name_policy::LookupPath::parse(&path).map_err(ApiError::invalid_source)?)
        };
        return Ok((DirectoryBranchFilter::OwningMain(path), projection));
    }
    let filter = match path {
        None => DirectoryBranchFilter::All,
        Some(path) if path.is_empty() => DirectoryBranchFilter::Root,
        Some(path) => DirectoryBranchFilter::Directory(
            name_policy::LookupPath::parse(&path).map_err(ApiError::invalid_source)?,
        ),
    };
    Ok((filter, projection))
}
pub(crate) struct Query(Result<(DirectoryBranchFilter, DirectoryBranchProjection), ApiError>);

impl<S: Send + Sync> FromRequestParts<S> for Query {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let ApiQuery(query) =
            ApiQuery::<BTreeMap<String, String>>::from_request_parts(parts, state).await?;
        Ok(Self(parse(query)))
    }
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(repository_id): ApiPath<String>,
    Query(query): Query,
) -> Result<Json<DirectoryScopesResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            let (filter, projection) = query?;
            backend
                .directory_scopes(
                    &identity,
                    ReadDirectoryScopes {
                        repository_id: &repository_id,
                        filter: &filter,
                        projection,
                    },
                )
                .map(DirectoryScopesResponse::from)
        })
        .await
        .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_directory_scopes.rs"]
mod tests;

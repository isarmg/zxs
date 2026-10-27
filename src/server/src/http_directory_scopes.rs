//! The public query format selects a typed scope and one response projection.
use crate::{
    ApiError, AppState, DirectoryBranchFilter, DirectoryBranchProjection, ReadDirectoryScopes,
    auth::Identity,
    backend_directory_scopes::DirectoryScopesResponse,
    http_boundary::{ApiPath, ApiQuery},
};
use http_routing::{Extension, Json, extract::State};
use std::collections::BTreeMap;

fn parse(
    mut query: BTreeMap<String, String>,
) -> Result<(DirectoryBranchFilter, DirectoryBranchProjection), ApiError> {
    if let Some(key) = query
        .keys()
        .find(|key| !["path", "summary"].contains(&key.as_str()))
    {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let projection = match query.remove("summary").as_deref() {
        None => DirectoryBranchProjection::Full,
        Some("true") => DirectoryBranchProjection::Summary,
        Some(_) => return Err(ApiError::invalid("summary must be true when supplied")),
    };
    let filter = match query.remove("path") {
        None => DirectoryBranchFilter::All,
        Some(path) if path.is_empty() => DirectoryBranchFilter::Root,
        Some(path) => DirectoryBranchFilter::Directory(
            name_policy::LookupPath::parse(&path).map_err(ApiError::invalid_source)?,
        ),
    };
    Ok((filter, projection))
}
pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository_id): ApiPath<String>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<DirectoryScopesResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let (filter, projection) = parse(query)?;
        state
            .backend
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
    .map_err(ApiError::join)?
    .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_directory_scopes.rs"]
mod tests;

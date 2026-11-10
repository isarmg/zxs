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
pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository_id): ApiPath<String>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<DirectoryScopesResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            let (filter, projection) = parse(query)?;
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

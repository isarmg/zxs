//! The public query format selects a typed scope and one response projection.
use crate::{
    ApiError, AppState, DirectoryBranchFilter, DirectoryBranchProjection, ReadDirectoryScopes,
    auth::Identity,
    backend_directory_scopes::DirectoryScopesResponse,
    http_boundary::{ApiPath, ApiQuery},
};
use http_routing::{Extension, extract::State, response::Response};
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
    connection: Option<Extension<crate::http_shutdown::ResponseConnection>>,
) -> Result<Response, ApiError> {
    crate::history_read::run(
        state,
        connection.map(|Extension(connection)| connection),
        move |backend, control| {
            let (filter, projection) = parse(query)?;
            backend.directory_scopes(
                &identity,
                ReadDirectoryScopes {
                    repository_id: &repository_id,
                    filter: &filter,
                    projection,
                },
                control,
            )
        },
        |snapshot| {
            crate::backend_file_history::encode_response_bounded(DirectoryScopesResponse::from(
                snapshot,
            ))
        },
    )
    .await
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_directory_scopes.rs"]
mod tests;

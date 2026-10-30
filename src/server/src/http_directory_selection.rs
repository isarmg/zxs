//! HTTP shape decoding precedes the reusable bounded-selection constructor.
use crate::{
    ApiError, AppState, DirectorySelectionEntryInput, DirectorySelectionSourceInput,
    ReadDirectorySelection, auth::Identity,
    backend_directory_selection::DirectorySelectionResponse, http_boundary::ApiPath,
};
use data_serialization::Deserialize;
use http_routing::{
    Extension,
    extract::{Request, State},
    response::Response,
};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct SelectionFields {
    repository_id: String,
    path: String,
    sources: Vec<SourceFields>,
    entries: Vec<EntryFields>,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct SourceFields {
    directory_branch_id: String,
    expected_tree_id: String,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct EntryFields {
    entry_id: String,
    path: String,
}
fn parse(body: json_codec::Value) -> Result<ReadDirectorySelection, ApiError> {
    let SelectionFields {
        repository_id,
        path,
        sources,
        entries,
    } = json_codec::from_value(body).map_err(ApiError::invalid_source)?;
    ReadDirectorySelection::parse(
        repository_id,
        &path,
        sources
            .into_iter()
            .map(
                |SourceFields {
                     directory_branch_id,
                     expected_tree_id,
                 }| DirectorySelectionSourceInput {
                    directory_branch_id,
                    expected_tree_id,
                },
            )
            .collect(),
        entries
            .into_iter()
            .map(|EntryFields { entry_id, path }| DirectorySelectionEntryInput { entry_id, path })
            .collect(),
    )
}
pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository_id): ApiPath<String>,
    connection: Option<Extension<crate::http_shutdown::ResponseConnection>>,
    request: Request,
) -> Result<Response, ApiError> {
    let body = crate::with_id(
        crate::json_body(request).await?,
        "repository_id",
        repository_id,
    )?;
    crate::history_read::run(
        state,
        connection.map(|Extension(connection)| connection),
        move |backend, control| {
            let selection = parse(body)?;
            backend.directory_selection(&identity, selection, control)
        },
        |snapshot| {
            crate::backend_file_history::encode_response_bounded(DirectorySelectionResponse::from(
                snapshot,
            ))
        },
    )
    .await
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_directory_selection.rs"]
mod tests;

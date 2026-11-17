//! Authenticated shared Draft commands and bounded descriptor-based BODY I/O.
use crate::{ApiError, AppState, ChangeFileWorking, FileWorkingSnapshot};
use crate::http_boundary::{ApiJson, ApiPath, ApiQuery, Authenticated, JsonBody};
use crate::transfer_limits::timeout_error;
use data_serialization::Deserialize;
use http_routing::{Json, extract::{Request, State}, response::Response};
use http::{HeaderMap, Method};
use storage_contract::{EntryId, FileId, RepositoryId};
use storage_contract::file_draft::{DraftCommand, DraftLocation};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct LocationQuery {
    repository_id: RepositoryId,
    path: name_policy::PortablePath,
    entry_id: EntryId,
}
impl LocationQuery {
    pub(super) fn target(self, file: FileId) -> DraftLocation {
        DraftLocation { repository: self.repository_id, path: self.path,
            entry: self.entry_id, file }
    }
}

pub(crate) struct CommandBody(pub(crate) ChangeFileWorking);
impl JsonBody for CommandBody {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        if !value.is_object()
            || !value.get("command").is_some_and(json_codec::Value::is_object)
            || !value.get("command").and_then(|value| value.get("target"))
                .is_some_and(json_codec::Value::is_object)
        {
            return Err(ApiError::invalid("draft command and target must be named objects"));
        }
        json_codec::from_value(value).map(Self).map_err(ApiError::invalid_source)
    }
}

pub(crate) async fn status(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FileId>, ApiQuery(query): ApiQuery<LocationQuery>,
) -> Result<Json<FileWorkingSnapshot>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.file_working(&identity, query.target(file)))
        .await.map(Json)
}

pub(crate) async fn change(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FileId>, ApiJson(body): ApiJson<CommandBody>,
) -> Result<Json<json_codec::Value>, ApiError> {
    if body.0.command.target().file != file {
        return Err(ApiError::invalid("draft command file differs from route"));
    }
    if matches!(body.0.command, DraftCommand::Write { .. }) {
        return Err(ApiError::invalid("draft writes require the streamed body endpoint"));
    }
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.change_file_working(&identity, body.0)
            .and_then(|result| crate::operation_wire::operation_wire(result.into())))
        .await.map(Json)
}

pub(crate) async fn write(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FileId>, request: Request,
) -> Result<Json<json_codec::Value>, ApiError> {
    // Preserve staging-readiness rejection before decoding the command.
    let budget = state.http_spool()?;
    let headers = request.headers();
    if headers.get_all("x-zxs-draft-command").iter().count() != 1 {
        return Err(ApiError::invalid("exactly one X-ZXS-Draft-Command is required"));
    }
    let encoded = headers.get("x-zxs-draft-command")
        .ok_or_else(|| ApiError::invalid("X-ZXS-Draft-Command is required"))?;
    if encoded.as_bytes().len() > 16_384 {
        return Err(ApiError::limit("draft command header exceeds its byte budget"));
    }
    let value = storage_contract::strict_json(encoded.as_bytes())
        .map_err(ApiError::contract_input)?;
    let CommandBody(command) = CommandBody::from_json(value)?;
    if command.command.target().file != file {
        return Err(ApiError::invalid("draft command file differs from route"));
    }
    let DraftCommand::Write { content_hash, logical_size, .. } = &command.command else {
        return Err(ApiError::invalid("body endpoint requires a draft write command"));
    };
    let upload = DraftUpload { hash: *content_hash, size: *logical_size, command };
    crate::http_complete_upload::receive(state, identity, upload, request, budget).await.map(Json)
}

struct DraftUpload {
    command: ChangeFileWorking,
    size: u64,
    hash: storage_contract::ContentHash,
}

impl crate::http_complete_upload::CompleteUpload for DraftUpload {
    type Output = json_codec::Value;

    fn size(&self) -> u64 { self.size }
    fn hash(&self) -> storage_contract::ContentHash { self.hash }
    fn claim_key(&self) -> String {
        format!("draft:{}:{}", self.command.command.target().repository,
            self.command.command.target().file)
    }
    fn authorize(&self, backend: &dyn crate::Backend, identity: &crate::Identity)
        -> Result<(), ApiError>
    {
        backend.authorize_file_draft_write(identity, self.command.command.target(), self.size)
    }
    fn publish(
        self,
        backend: &dyn crate::Backend,
        identity: &crate::Identity,
        source: &crate::UploadSource,
    ) -> Result<Self::Output, ApiError> {
        backend.write_file_draft(identity, self.command, source)
            .and_then(|result| crate::operation_wire::operation_wire(result.into()))
    }
}

pub(crate) async fn content(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FileId>, ApiQuery(query): ApiQuery<LocationQuery>,
    method: Method, headers: HeaderMap,
) -> Result<Response, ApiError> {
    let limits = state.config.http_transfer_limits.clone();
    let deadline = tokio::time::Instant::now() + limits.total_timeout;
    let permit = state.read_slots.acquire()?;
    let request = crate::ReadFileDraft {
        target: query.target(file), deadline: deadline.into_std(),
    };
    let content = tokio::time::timeout_at(deadline,
        crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
            .run_with_permit(move |backend, permit| {
                let mut content = backend.file_draft_content(&identity, request)?;
                content.lease = Box::new((content.lease, permit));
                Ok(content)
            })).await.map_err(|source| timeout_error().with_source(source))??;
    crate::transfer::content_response_with_limits(content, method, headers, &limits, deadline)
}
